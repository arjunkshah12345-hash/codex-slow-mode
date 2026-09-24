use std::time::Duration;

use crate::config::SlowModeConfig;
use crate::status::SlowModeStatus;
use crate::status::format_duration;

/// One usage window from Codex rate-limit telemetry.
///
/// `resets_at_unix_secs` is the server reset timestamp. `window_minutes` is
/// recorded for status and diagnostics; pacing uses the reset timestamp
/// because the window length is not the time remaining.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    pub resets_at_unix_secs: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RateSnapshot {
    pub primary: Option<UsageWindow>,
    pub secondary: Option<UsageWindow>,
    /// When this snapshot was observed, unix milliseconds.
    pub observed_unix_ms: i64,
}

#[derive(Debug, Clone)]
struct Anchor {
    t0_ms: i64,
    u0: f64,
    reset_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DelayKind {
    None,
    Interval,
    UntilReset,
    Fallback,
}

#[derive(Debug, Clone)]
struct WindowPlan {
    wait: Duration,
    kind: DelayKind,
    used_percent: Option<f64>,
    resets_in: Option<Duration>,
    usable: Option<f64>,
}

/// Pure pacing math. Time is injected so tests do not sleep.
#[derive(Debug, Clone)]
pub(crate) struct SlowModeRateController {
    config: SlowModeConfig,
    enabled: bool,
    anchor: Option<Anchor>,
    last: Option<RateSnapshot>,
    ewma_cost: Option<f64>,
    smoothed_interval: Option<Duration>,
    last_grant_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct PaceDecision {
    pub wait: Duration,
    pub message: String,
    /// True when the secondary window, not the five-hour window, set the wait.
    #[allow(dead_code)]
    pub preserving_secondary: bool,
    pub status: SlowModeStatus,
}

impl SlowModeRateController {
    pub(crate) fn new(config: SlowModeConfig) -> Self {
        Self {
            config,
            enabled: false,
            anchor: None,
            last: None,
            ewma_cost: None,
            smoothed_interval: None,
            last_grant_ms: None,
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn enable(&mut self, now_ms: i64, limits: Option<RateSnapshot>) {
        if self.enabled {
            if let Some(limits) = limits {
                self.observe(limits);
            }
            return;
        }
        self.enabled = true;
        self.last_grant_ms = None;
        self.smoothed_interval = None;
        self.anchor = limits.as_ref().and_then(|snapshot| {
            snapshot.primary.as_ref().map(|window| Anchor {
                t0_ms: now_ms,
                u0: finite_percent(window.used_percent).unwrap_or(0.0),
                reset_ms: window.resets_at_unix_secs.and_then(secs_to_ms),
            })
        });
        if let Some(limits) = limits {
            self.last = Some(limits);
        }
    }

    pub(crate) fn disable(&mut self) {
        self.enabled = false;
        self.smoothed_interval = None;
    }

    pub(crate) fn observe(&mut self, snapshot: RateSnapshot) {
        if let (Some(previous), Some(current)) = (
            self.last
                .as_ref()
                .and_then(|previous| previous.primary.clone()),
            snapshot.primary.clone(),
        ) {
            if let (Some(before), Some(after)) = (
                finite_percent(previous.used_percent),
                finite_percent(current.used_percent),
            ) {
                let delta = after - before;
                if delta < -1.0 {
                    self.reanchor(snapshot.observed_unix_ms, &snapshot);
                } else if delta > self.config.min_sample_percent
                    && delta <= self.config.max_sample_percent
                    && delta.is_finite()
                {
                    self.ewma_cost = Some(match self.ewma_cost {
                        Some(previous_cost) => {
                            self.config.ewma_alpha * delta
                                + (1.0 - self.config.ewma_alpha) * previous_cost
                        }
                        None => delta,
                    });
                }
            }
        }
        self.last = Some(snapshot);
    }

    pub(crate) fn mark_granted(&mut self, now_ms: i64) {
        self.last_grant_ms = Some(now_ms);
    }

    pub(crate) fn decide(&mut self, now_ms: i64, queued: usize) -> PaceDecision {
        if !self.enabled {
            return PaceDecision {
                wait: Duration::ZERO,
                message: String::new(),
                preserving_secondary: false,
                status: self.status(now_ms, queued, Duration::ZERO, false),
            };
        }

        let stale = self.telemetry_is_stale(now_ms);
        let primary = self.plan_window(now_ms, true, stale);
        let secondary = self.plan_window(now_ms, false, stale);
        let mut preserving_secondary = false;
        let mut secondary_wait = secondary.wait;
        if matches!(secondary.kind, DelayKind::UntilReset | DelayKind::Interval)
            && secondary.wait > self.config.secondary_enforced_cap
            && primary.usable.unwrap_or(1.0) > 0.05
            && !matches!(primary.kind, DelayKind::UntilReset)
        {
            preserving_secondary = true;
            secondary_wait = self.config.secondary_enforced_cap;
        }

        let raw_wait = primary.wait.max(secondary_wait);
        let wait = self.account_for_elapsed(now_ms, raw_wait, &primary);
        if !wait.is_zero() && !matches!(primary.kind, DelayKind::UntilReset) {
            self.smoothed_interval = Some(match self.smoothed_interval {
                Some(previous) => smooth(previous, raw_wait),
                None => raw_wait,
            });
        }

        let message = pacing_message(wait, preserving_secondary, &primary, &secondary);
        PaceDecision {
            preserving_secondary,
            status: self.status(now_ms, queued, wait, preserving_secondary),
            wait,
            message,
        }
    }

    fn reanchor(&mut self, now_ms: i64, snapshot: &RateSnapshot) {
        self.ewma_cost = None;
        self.smoothed_interval = None;
        self.anchor = snapshot.primary.as_ref().map(|window| Anchor {
            t0_ms: now_ms,
            u0: finite_percent(window.used_percent).unwrap_or(0.0),
            reset_ms: window.resets_at_unix_secs.and_then(secs_to_ms),
        });
    }

    fn telemetry_is_stale(&self, now_ms: i64) -> bool {
        let Some(snapshot) = &self.last else {
            return true;
        };
        let age_ms = now_ms.saturating_sub(snapshot.observed_unix_ms);
        age_ms < 0
            || Duration::from_millis(age_ms.max(0) as u64) > self.config.telemetry_stale_after
    }

    fn plan_window(&self, now_ms: i64, primary: bool, stale: bool) -> WindowPlan {
        if stale {
            return WindowPlan {
                wait: if primary {
                    self.config.fallback_interval
                } else {
                    Duration::ZERO
                },
                kind: if primary {
                    DelayKind::Fallback
                } else {
                    DelayKind::None
                },
                used_percent: None,
                resets_in: None,
                usable: None,
            };
        }
        let window = self.last.as_ref().and_then(|snapshot| {
            if primary {
                snapshot.primary.clone()
            } else {
                snapshot.secondary.clone()
            }
        });
        let Some(window) = window else {
            return WindowPlan {
                wait: if primary {
                    self.config.fallback_interval
                } else {
                    Duration::ZERO
                },
                kind: if primary {
                    DelayKind::Fallback
                } else {
                    DelayKind::None
                },
                used_percent: None,
                resets_in: None,
                usable: None,
            };
        };
        let used = finite_percent(window.used_percent);
        let Some(used) = used else {
            return WindowPlan {
                wait: self.config.fallback_interval,
                kind: DelayKind::Fallback,
                used_percent: None,
                resets_in: None,
                usable: None,
            };
        };
        let reset_ms = window.resets_at_unix_secs.and_then(secs_to_ms);
        let Some(reset_ms) = reset_ms else {
            return WindowPlan {
                wait: if primary {
                    self.config.fallback_interval
                } else {
                    Duration::ZERO
                },
                kind: if primary {
                    DelayKind::Fallback
                } else {
                    DelayKind::None
                },
                used_percent: Some(used),
                resets_in: None,
                usable: None,
            };
        };
        if now_ms >= reset_ms {
            return WindowPlan {
                wait: Duration::ZERO,
                kind: DelayKind::None,
                used_percent: Some(0.0),
                resets_in: Some(Duration::ZERO),
                usable: Some((100.0 - self.config.reserve_percent).max(0.0)),
            };
        }
        let until_reset = Duration::from_millis((reset_ms - now_ms) as u64);
        let usable = (100.0 - self.config.reserve_percent - used).max(0.0);
        if usable <= 0.05 {
            return WindowPlan {
                wait: until_reset,
                kind: DelayKind::UntilReset,
                used_percent: Some(used),
                resets_in: Some(until_reset),
                usable: Some(0.0),
            };
        }
        let Some(cost) = self.ewma_cost else {
            return WindowPlan {
                wait: if primary {
                    self.config.fallback_interval
                } else {
                    Duration::ZERO
                },
                kind: if primary {
                    DelayKind::Fallback
                } else {
                    DelayKind::None
                },
                used_percent: Some(used),
                resets_in: Some(until_reset),
                usable: Some(usable),
            };
        };
        let cost = self.config.sanitized_cost(cost);
        let requests_left = usable / cost;
        if !requests_left.is_finite() || requests_left <= f64::EPSILON {
            return WindowPlan {
                wait: until_reset,
                kind: DelayKind::UntilReset,
                used_percent: Some(used),
                resets_in: Some(until_reset),
                usable: Some(usable),
            };
        }
        let mut interval_ms = until_reset.as_millis() as f64 / requests_left;
        if primary && let Some(target) = self.target_used(now_ms) {
            let ahead = used - target;
            let multiplier = (1.0 + ahead / 8.0).clamp(0.3, 3.5);
            interval_ms *= multiplier;
        }
        if !interval_ms.is_finite() || interval_ms < 0.0 {
            interval_ms = self.config.fallback_interval.as_millis() as f64;
        }
        interval_ms = interval_ms.min(until_reset.as_millis() as f64);
        WindowPlan {
            wait: Duration::from_millis(interval_ms.round().max(0.0) as u64),
            kind: DelayKind::Interval,
            used_percent: Some(used),
            resets_in: Some(until_reset),
            usable: Some(usable),
        }
    }

    fn target_used(&self, now_ms: i64) -> Option<f64> {
        let anchor = self.anchor.as_ref()?;
        let reset_ms = anchor.reset_ms?;
        let window = reset_ms.saturating_sub(anchor.t0_ms);
        if window <= 0 {
            return None;
        }
        let elapsed = (now_ms - anchor.t0_ms).clamp(0, window);
        let usable = (100.0 - self.config.reserve_percent - anchor.u0).max(0.0);
        Some(anchor.u0 + usable * (elapsed as f64 / window as f64))
    }

    fn account_for_elapsed(
        &self,
        now_ms: i64,
        raw_wait: Duration,
        primary: &WindowPlan,
    ) -> Duration {
        if matches!(primary.kind, DelayKind::UntilReset) && self.last_grant_ms.is_none() {
            return raw_wait;
        }
        let Some(last_grant) = self.last_grant_ms else {
            return if matches!(primary.kind, DelayKind::UntilReset) {
                raw_wait
            } else {
                Duration::ZERO
            };
        };
        if now_ms < last_grant {
            return raw_wait;
        }
        let elapsed = Duration::from_millis((now_ms - last_grant) as u64);
        let interval = self.smoothed_interval.unwrap_or(raw_wait);
        let target = interval.max(raw_wait);
        target.saturating_sub(elapsed)
    }

    fn status(
        &self,
        now_ms: i64,
        queued: usize,
        next_wait: Duration,
        preserving_secondary: bool,
    ) -> SlowModeStatus {
        let primary = self
            .last
            .as_ref()
            .and_then(|snapshot| snapshot.primary.as_ref());
        let secondary = self
            .last
            .as_ref()
            .and_then(|snapshot| snapshot.secondary.as_ref());
        SlowModeStatus {
            enabled: self.enabled,
            model: None,
            fast_mode: None,
            primary_used_percent: primary.and_then(|window| finite_percent(window.used_percent)),
            primary_resets_in: primary.and_then(|window| reset_in(now_ms, window)),
            secondary_used_percent: secondary
                .and_then(|window| finite_percent(window.used_percent)),
            secondary_resets_in: secondary.and_then(|window| reset_in(now_ms, window)),
            estimated_request_cost_percent: self
                .ewma_cost
                .filter(|cost| cost.is_finite() && *cost > 0.0),
            next_eligible_in: self.enabled.then_some(next_wait),
            queued_model_requests: queued,
            preserving_secondary,
            note: None,
        }
    }
}

fn pacing_message(
    wait: Duration,
    preserving_secondary: bool,
    primary: &WindowPlan,
    secondary: &WindowPlan,
) -> String {
    let mut message = format!(
        "Slow mode · pacing usage · next model request in ~{}",
        format_duration(wait)
    );
    if preserving_secondary {
        message.push_str("\nSlow mode is preserving your longer-term allowance.");
        if let Some(used) = primary.used_percent {
            message.push_str(&format!("\nPrimary: {used:.0}% used"));
            if let Some(reset) = primary.resets_in {
                message.push_str(&format!(", resets in {}", format_duration(reset)));
            }
        }
        if let Some(used) = secondary.used_percent {
            message.push_str(&format!("\nSecondary: {used:.0}% used"));
            if let Some(reset) = secondary.resets_in {
                message.push_str(&format!(", resets in {}", format_duration(reset)));
            }
        }
    }
    message
}

fn smooth(previous: Duration, raw: Duration) -> Duration {
    let mixed = 0.65 * previous.as_secs_f64() + 0.35 * raw.as_secs_f64();
    if !mixed.is_finite() || mixed < 0.0 {
        raw
    } else {
        Duration::from_secs_f64(mixed)
    }
}

fn finite_percent(value: f64) -> Option<f64> {
    value.is_finite().then_some(value.clamp(0.0, 100.0))
}

fn secs_to_ms(secs: i64) -> Option<i64> {
    secs.checked_mul(1000)
}

fn reset_in(now_ms: i64, window: &UsageWindow) -> Option<Duration> {
    let reset_ms = secs_to_ms(window.resets_at_unix_secs?)?;
    if reset_ms <= now_ms {
        return Some(Duration::ZERO);
    }
    Some(Duration::from_millis((reset_ms - now_ms) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(used: f64, reset_secs: i64) -> UsageWindow {
        UsageWindow {
            used_percent: used,
            window_minutes: Some(300),
            resets_at_unix_secs: Some(reset_secs),
        }
    }

    fn snapshot(now_ms: i64, primary: f64, reset_secs: i64) -> RateSnapshot {
        RateSnapshot {
            primary: Some(window(primary, reset_secs)),
            secondary: None,
            observed_unix_ms: now_ms,
        }
    }

    #[test]
    fn ahead_of_trajectory_increases_delay() {
        let mut calm = SlowModeRateController::new(SlowModeConfig::default());
        let mut hot = SlowModeRateController::new(SlowModeConfig::default());
        let now = 1_000_000_i64;
        let reset = now / 1000 + 180 * 60;
        calm.enable(now, Some(snapshot(now, 40.0, reset)));
        hot.enable(now, Some(snapshot(now, 40.0, reset)));
        calm.observe(snapshot(now + 60_000, 41.0, reset));
        hot.observe(snapshot(now + 60_000, 55.0, reset));
        calm.mark_granted(now + 60_000);
        hot.mark_granted(now + 60_000);
        let calm_wait = calm.decide(now + 61_000, 0).wait;
        let hot_wait = hot.decide(now + 61_000, 0).wait;
        assert!(
            hot_wait > calm_wait,
            "hot {hot_wait:?} should exceed calm {calm_wait:?}"
        );
    }

    #[test]
    fn natural_elapsed_time_shrinks_the_wait() {
        let mut controller = SlowModeRateController::new(SlowModeConfig::default());
        let now = 5_000_000_i64;
        let reset = now / 1000 + 180 * 60;
        controller.enable(now, Some(snapshot(now, 40.0, reset)));
        controller.observe(snapshot(now + 1_000, 41.0, reset));
        controller.mark_granted(now + 1_000);
        let immediate = controller.decide(now + 1_000, 0).wait;
        let after_shell = controller.decide(now + 1_000 + 10 * 60 * 1000, 0).wait;
        assert!(immediate > Duration::from_secs(30));
        assert!(after_shell < immediate);
        assert_eq!(after_shell, Duration::ZERO);
    }

    #[test]
    fn quantized_zero_delta_does_not_create_infinite_throughput() {
        let mut controller = SlowModeRateController::new(SlowModeConfig::default());
        let now = 9_000_i64;
        let reset = now / 1000 + 3 * 60 * 60;
        controller.enable(now, Some(snapshot(now, 40.0, reset)));
        controller.observe(snapshot(now + 1_000, 40.0, reset));
        controller.observe(snapshot(now + 2_000, 40.0, reset));
        assert!(controller.ewma_cost.is_none());
        controller.mark_granted(now + 2_000);
        let decision = controller.decide(now + 2_000, 0);
        assert!(decision.wait > Duration::ZERO);
        assert!(decision.wait < Duration::from_secs(10 * 60));
    }

    #[test]
    fn zero_cost_floor_stays_finite() {
        let config = SlowModeConfig::default();
        assert!(config.sanitized_cost(0.0) > 0.0);
        assert!(config.sanitized_cost(f64::NAN) > 0.0);
        assert!(config.sanitized_cost(f64::INFINITY).is_finite());
    }

    #[test]
    fn missing_telemetry_uses_fallback() {
        let mut controller = SlowModeRateController::new(SlowModeConfig::default());
        controller.enable(0, None);
        controller.mark_granted(0);
        let wait = controller.decide(0, 0).wait;
        assert_eq!(wait, SlowModeConfig::default().fallback_interval);
    }

    #[test]
    fn stale_telemetry_uses_fallback() {
        let mut controller = SlowModeRateController::new(SlowModeConfig::default());
        let now = 0_i64;
        let reset = 5 * 60 * 60;
        controller.enable(now, Some(snapshot(now, 10.0, reset)));
        controller.observe(snapshot(now, 11.0, reset));
        let later = SlowModeConfig::default().telemetry_stale_after.as_millis() as i64 + 5_000;
        controller.mark_granted(later);
        let wait = controller.decide(later, 0).wait;
        assert_eq!(wait, SlowModeConfig::default().fallback_interval);
    }

    #[test]
    fn reset_timestamp_in_the_past_does_not_keep_blocking() {
        let mut controller = SlowModeRateController::new(SlowModeConfig::default());
        let now = 50_000_i64;
        controller.enable(now, Some(snapshot(now, 99.0, 10)));
        controller.mark_granted(now);
        let wait = controller.decide(now + 1_000, 0).wait;
        assert_eq!(wait, Duration::ZERO);
    }

    #[test]
    fn exhausted_primary_waits_until_reset() {
        let mut controller = SlowModeRateController::new(SlowModeConfig::default());
        let now = 0_i64;
        let reset_secs = 30 * 60;
        controller.enable(now, Some(snapshot(now, 95.0, reset_secs)));
        let wait = controller.decide(now, 0).wait;
        assert!(wait > Duration::from_secs(29 * 60));
        assert!(wait <= Duration::from_secs(30 * 60));
    }

    #[test]
    fn secondary_pressure_increases_wait_but_caps_the_sleep() {
        let mut controller = SlowModeRateController::new(SlowModeConfig::default());
        let now = 1_000_000_i64;
        let primary_reset = now / 1000 + 3 * 60 * 60;
        let secondary_reset = now / 1000 + 3 * 24 * 60 * 60;
        controller.enable(
            now,
            Some(RateSnapshot {
                primary: Some(window(20.0, primary_reset)),
                secondary: Some(UsageWindow {
                    used_percent: 91.0,
                    window_minutes: Some(10_080),
                    resets_at_unix_secs: Some(secondary_reset),
                }),
                observed_unix_ms: now,
            }),
        );
        controller.mark_granted(now);
        let decision = controller.decide(now, 0);
        assert!(decision.preserving_secondary);
        assert!(decision.wait <= SlowModeConfig::default().secondary_enforced_cap);
        assert!(decision.wait > Duration::from_secs(60));
        assert!(decision.message.contains("longer-term allowance"));
    }

    #[test]
    fn disabled_controller_does_not_wait() {
        let mut controller = SlowModeRateController::new(SlowModeConfig::default());
        let now = 0;
        controller.enable(now, Some(snapshot(now, 95.0, 3_600)));
        controller.disable();
        assert_eq!(controller.decide(now, 0).wait, Duration::ZERO);
    }
}
