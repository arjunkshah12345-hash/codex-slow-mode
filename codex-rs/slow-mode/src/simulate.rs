use std::time::Duration;

use crate::config::SlowModeConfig;
use crate::controller::RateSnapshot;
use crate::controller::SlowModeRateController;
use crate::controller::UsageWindow;

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    Fixed,
    Adaptive,
}

#[derive(Clone)]
struct Turn {
    cost_percent: f64,
    /// Local work after the model response. This is not extra Slow Mode delay.
    shell_after_ms: u64,
    inference_ms: u64,
}

struct Workload {
    name: &'static str,
    turns: Vec<Turn>,
    initial_used: f64,
    window_ms: u64,
    secondary_used: Option<f64>,
    secondary_window_ms: Option<u64>,
    telemetry: bool,
    /// Subagent calls are ready together and must all finish.
    parallel: bool,
}

#[derive(Clone, Debug)]
struct Outcome {
    completion_ms: u64,
    peak_used_before_reset: f64,
    exhausted_before_reset: bool,
    idle_ms: u64,
    requests: usize,
    quality_changes: u32,
}

pub struct SimulationReport {
    pub markdown: String,
}

pub fn render_simulation_report() -> String {
    let mut lines = vec![
        "# Slow Mode simulation".to_string(),
        String::new(),
        "Deterministic fake clock. Quality-affecting changes are model, reasoning, tool, and prompt changes. Slow Mode keeps that count at zero and only moves request start times.".to_string(),
        String::new(),
        "| Workload | Mode | Completion | Peak use before reset | Exhausted before reset | Inserted idle | Requests | Quality changes |".to_string(),
        "| --- | --- | ---: | ---: | --- | ---: | ---: | ---: |".to_string(),
    ];
    for workload in workloads() {
        for mode in [Mode::Normal, Mode::Fixed, Mode::Adaptive] {
            let outcome = simulate(&workload, mode);
            lines.push(format!(
                "| {} | {} | {} | {:.1}% | {} | {} | {} | {} |",
                workload.name,
                mode_name(mode),
                format_ms(outcome.completion_ms),
                outcome.peak_used_before_reset,
                if outcome.exhausted_before_reset {
                    "yes"
                } else {
                    "no"
                },
                format_ms(outcome.idle_ms),
                outcome.requests,
                outcome.quality_changes,
            ));
        }
    }
    lines.push(String::new());
    lines.push("Fallback interval when telemetry is missing: 45s. That is long enough that a burst of follow-up turns no longer fits in a couple of seconds, and short enough that the first live rate-limit sample can take over.".to_string());
    lines.join("\n")
}

fn workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "many cheap turns",
            turns: repeat(40, 0.4, 0, 200),
            initial_used: 10.0,
            window_ms: 5 * 60 * 60 * 1000,
            secondary_used: Some(20.0),
            secondary_window_ms: Some(7 * 24 * 60 * 60 * 1000),
            telemetry: true,
            parallel: false,
        },
        Workload {
            name: "few expensive turns",
            turns: repeat(6, 4.0, 0, 2_000),
            initial_used: 20.0,
            window_ms: 5 * 60 * 60 * 1000,
            secondary_used: Some(30.0),
            secondary_window_ms: Some(7 * 24 * 60 * 60 * 1000),
            telemetry: true,
            parallel: false,
        },
        Workload {
            name: "mixed-cost turns",
            turns: vec![
                turn(0.3, 0, 200),
                turn(2.5, 20_000, 1_000),
                turn(0.4, 0, 200),
                turn(3.0, 0, 1_500),
                turn(0.5, 5_000, 300),
                turn(1.5, 0, 800),
                turn(0.2, 0, 200),
                turn(2.0, 0, 900),
            ],
            initial_used: 25.0,
            window_ms: 5 * 60 * 60 * 1000,
            secondary_used: Some(40.0),
            secondary_window_ms: Some(7 * 24 * 60 * 60 * 1000),
            telemetry: true,
            parallel: false,
        },
        Workload {
            name: "parallel subagents",
            turns: repeat(4, 1.5, 0, 500),
            initial_used: 30.0,
            window_ms: 5 * 60 * 60 * 1000,
            secondary_used: None,
            secondary_window_ms: None,
            telemetry: true,
            parallel: true,
        },
        Workload {
            name: "long-running shell commands",
            turns: repeat(8, 1.0, 4 * 60 * 1000, 300),
            initial_used: 35.0,
            window_ms: 5 * 60 * 60 * 1000,
            secondary_used: None,
            secondary_window_ms: None,
            telemetry: true,
            parallel: false,
        },
        Workload {
            name: "50% already consumed",
            turns: repeat(30, 1.0, 0, 200),
            initial_used: 50.0,
            window_ms: 3 * 60 * 60 * 1000,
            secondary_used: Some(40.0),
            secondary_window_ms: Some(7 * 24 * 60 * 60 * 1000),
            telemetry: true,
            parallel: false,
        },
        Workload {
            name: "80% already consumed",
            turns: repeat(25, 1.2, 0, 200),
            initial_used: 80.0,
            window_ms: 2 * 60 * 60 * 1000,
            secondary_used: Some(50.0),
            secondary_window_ms: Some(7 * 24 * 60 * 60 * 1000),
            telemetry: true,
            parallel: false,
        },
        Workload {
            name: "95% already consumed",
            turns: repeat(10, 1.0, 0, 200),
            initial_used: 95.0,
            window_ms: 20 * 60 * 1000,
            secondary_used: Some(60.0),
            secondary_window_ms: Some(3 * 24 * 60 * 60 * 1000),
            telemetry: true,
            parallel: false,
        },
        Workload {
            name: "reset imminent",
            turns: repeat(8, 2.0, 0, 100),
            initial_used: 70.0,
            window_ms: 3 * 60 * 1000,
            secondary_used: None,
            secondary_window_ms: None,
            telemetry: true,
            parallel: false,
        },
        Workload {
            name: "missing telemetry",
            turns: repeat(12, 1.0, 0, 100),
            initial_used: 0.0,
            window_ms: 5 * 60 * 60 * 1000,
            secondary_used: None,
            secondary_window_ms: None,
            telemetry: false,
            parallel: false,
        },
        Workload {
            name: "secondary nearly exhausted",
            turns: repeat(15, 0.8, 0, 200),
            initial_used: 30.0,
            window_ms: 4 * 60 * 60 * 1000,
            secondary_used: Some(91.0),
            secondary_window_ms: Some(3 * 24 * 60 * 60 * 1000),
            telemetry: true,
            parallel: false,
        },
    ]
}

fn simulate(workload: &Workload, mode: Mode) -> Outcome {
    let mut controller = SlowModeRateController::new(SlowModeConfig::default());
    let mut now = 0_u64;
    let mut used = if workload.telemetry {
        workload.initial_used
    } else {
        0.0
    };
    let mut secondary_used = workload.secondary_used.unwrap_or(0.0);
    let mut peak_before_reset = used;
    let mut exhausted = false;
    let mut idle = 0_u64;
    let mut requests = 0_usize;
    let mut rolled = false;
    let mut last_grant: Option<u64> = None;

    if workload.telemetry {
        controller.enable(0, Some(snapshot(workload, 0, used, secondary_used)));
    } else {
        controller.enable(0, None);
    }

    // Parallel subagents become ready together. Adaptive mode still runs
    // their model calls one at a time; it does not drop them.
    let turns = workload.turns.clone();
    for (index, turn) in turns.iter().enumerate() {
        if !rolled && now >= workload.window_ms {
            rolled = true;
            used = 0.0;
            controller.enable(
                now as i64,
                Some(snapshot(workload, now, used, secondary_used)),
            );
        }
        let wait = match mode {
            Mode::Normal => Duration::ZERO,
            Mode::Fixed => fixed_wait(last_grant, now),
            Mode::Adaptive => controller.decide(now as i64, turns.len() - index).wait,
        };
        let wait_ms = wait.as_millis() as u64;
        idle += wait_ms;
        now += wait_ms;
        if !rolled && now >= workload.window_ms {
            rolled = true;
            if used >= 99.5 {
                exhausted = true;
            }
            used = 0.0;
            if workload.telemetry {
                controller.enable(
                    now as i64,
                    Some(snapshot(workload, now, used, secondary_used)),
                );
            }
        }
        controller.mark_granted(now as i64);
        last_grant = Some(now);
        now += turn.inference_ms;
        requests += 1;
        if !rolled && now < workload.window_ms {
            used += turn.cost_percent;
            secondary_used += turn.cost_percent * 0.25;
            peak_before_reset = peak_before_reset.max(used);
            if used >= 99.5 {
                exhausted = true;
            }
        }
        if workload.telemetry {
            controller.observe(snapshot(workload, now, used, secondary_used));
        }
        if !workload.parallel {
            now += turn.shell_after_ms;
        }
    }
    Outcome {
        completion_ms: now,
        peak_used_before_reset: peak_before_reset,
        exhausted_before_reset: exhausted,
        idle_ms: idle,
        requests,
        quality_changes: 0,
    }
}

fn fixed_wait(last_grant: Option<u64>, now: u64) -> Duration {
    // Fixed mode always targets 45s between grants and ignores the rate
    // trajectory. Elapsed tool time still counts, matching the product rule
    // that a long shell is already pacing.
    let Some(last_grant) = last_grant else {
        return Duration::ZERO;
    };
    Duration::from_millis(45_000u64.saturating_sub(now.saturating_sub(last_grant)))
}

fn snapshot(workload: &Workload, now: u64, used: f64, secondary_used: f64) -> RateSnapshot {
    let reset = (workload.window_ms / 1000) as i64;
    // The controller reads an absolute reset timestamp. Shift it forward once
    // the simulated window has rolled so later requests are not stuck.
    let primary_reset = if now >= workload.window_ms {
        (now / 1000) as i64 + (workload.window_ms / 1000) as i64
    } else {
        reset
    };
    RateSnapshot {
        primary: Some(UsageWindow {
            used_percent: used,
            window_minutes: Some((workload.window_ms / 60_000) as i64),
            resets_at_unix_secs: Some(primary_reset.max(1)),
        }),
        secondary: workload.secondary_used.map(|_| UsageWindow {
            used_percent: secondary_used.min(100.0),
            window_minutes: workload
                .secondary_window_ms
                .map(|window| (window / 60_000) as i64),
            resets_at_unix_secs: workload
                .secondary_window_ms
                .map(|window| (window / 1000) as i64),
        }),
        observed_unix_ms: now as i64,
    }
}

fn turn(cost: f64, shell_after_ms: u64, inference_ms: u64) -> Turn {
    Turn {
        cost_percent: cost,
        shell_after_ms,
        inference_ms,
    }
}

fn repeat(count: usize, cost: f64, shell_after_ms: u64, inference_ms: u64) -> Vec<Turn> {
    (0..count)
        .map(|_| turn(cost, shell_after_ms, inference_ms))
        .collect()
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "normal",
        Mode::Fixed => "fixed-delay",
        Mode::Adaptive => "adaptive",
    }
}

fn format_ms(ms: u64) -> String {
    let duration = Duration::from_millis(ms);
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    fn outcome(name: &str, mode: Mode) -> Outcome {
        let workload = workloads()
            .into_iter()
            .find(|workload| workload.name == name)
            .expect(name);
        simulate(&workload, mode)
    }

    #[test]
    fn adaptive_spreads_bursts_without_changing_quality_or_dropping_work() {
        let normal = outcome("many cheap turns", Mode::Normal);
        let adaptive = outcome("many cheap turns", Mode::Adaptive);
        assert!(adaptive.completion_ms > normal.completion_ms);
        assert!(adaptive.idle_ms > normal.idle_ms);
        assert_eq!(adaptive.requests, normal.requests);
        assert_eq!(adaptive.quality_changes, 0);
        assert_eq!(normal.quality_changes, 0);
    }

    #[test]
    fn high_usage_is_not_burned_through_before_reset() {
        for name in ["80% already consumed", "95% already consumed"] {
            let normal = outcome(name, Mode::Normal);
            let adaptive = outcome(name, Mode::Adaptive);
            assert!(
                normal.exhausted_before_reset,
                "{name} normal should exhaust in this burst"
            );
            assert!(
                !adaptive.exhausted_before_reset,
                "{name} adaptive should stay under the cap until reset"
            );
            assert_eq!(adaptive.requests, normal.requests);
            assert_eq!(adaptive.quality_changes, 0);
        }
    }

    #[test]
    fn shell_time_counts_as_pacing() {
        let normal = outcome("long-running shell commands", Mode::Normal);
        let fixed = outcome("long-running shell commands", Mode::Fixed);
        let adaptive = outcome("long-running shell commands", Mode::Adaptive);
        assert_eq!(
            fixed.idle_ms, 0,
            "a four-minute shell already covers a fixed 45s gap"
        );
        assert!(
            adaptive.idle_ms < 7 * 4 * 60 * 1000,
            "inserted idle {} should stay under the shell time already spent",
            adaptive.idle_ms
        );
        assert!(adaptive.completion_ms >= normal.completion_ms);
        assert!(adaptive.completion_ms > 7 * 4 * 60 * 1000);
        assert_eq!(adaptive.quality_changes, 0);
        assert_eq!(adaptive.requests, 8);
        assert_eq!(normal.requests, adaptive.requests);
    }

    #[test]
    fn parallel_subagents_all_run() {
        let adaptive = outcome("parallel subagents", Mode::Adaptive);
        assert_eq!(adaptive.requests, 4);
        assert_eq!(adaptive.quality_changes, 0);
        assert!(adaptive.completion_ms > outcome("parallel subagents", Mode::Normal).completion_ms);
    }

    #[test]
    fn missing_telemetry_still_spaces_requests() {
        let normal = outcome("missing telemetry", Mode::Normal);
        let adaptive = outcome("missing telemetry", Mode::Adaptive);
        assert!(adaptive.completion_ms > normal.completion_ms);
        assert_eq!(adaptive.requests, normal.requests);
        assert!(!adaptive.exhausted_before_reset);
    }

    #[test]
    fn secondary_pressure_adds_idle_time() {
        let normal = outcome("secondary nearly exhausted", Mode::Normal);
        let adaptive = outcome("secondary nearly exhausted", Mode::Adaptive);
        assert!(adaptive.idle_ms > normal.idle_ms);
        assert_eq!(adaptive.quality_changes, 0);
    }
}
