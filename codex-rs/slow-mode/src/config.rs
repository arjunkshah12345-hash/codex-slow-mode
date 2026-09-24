use std::time::Duration;

/// Tunables for the rate controller. These are internal defaults, not a
/// user-facing delay picker.
#[derive(Debug, Clone)]
pub struct SlowModeConfig {
    /// Percent of each window left unused on purpose.
    pub reserve_percent: f64,
    /// EWMA weight for a new positive usage sample.
    pub ewma_alpha: f64,
    /// Delay used when rate-limit telemetry is missing or stale.
    ///
    /// 45s is the measured fallback: a burst of tool-follow-ups no longer
    /// lands inside a few seconds, and the first live usage sample replaces
    /// it. See the simulation report.
    pub fallback_interval: Duration,
    /// Ignore telemetry older than this and use the fallback interval.
    pub telemetry_stale_after: Duration,
    /// Samples at or below this percent are quantization noise, not "free".
    pub min_sample_percent: f64,
    /// Ignore jumps larger than this; they are usually a window change.
    pub max_sample_percent: f64,
    /// Floor used when dividing remaining allowance by estimated request cost.
    pub min_cost_percent: f64,
    /// Longest secondary-window delay that is actually slept while the
    /// primary window still has usable allowance. Longer secondary pressure
    /// is explained in status instead of becoming a multi-hour sleep.
    pub secondary_enforced_cap: Duration,
}

impl Default for SlowModeConfig {
    fn default() -> Self {
        Self {
            reserve_percent: 10.0,
            ewma_alpha: 0.3,
            fallback_interval: Duration::from_secs(45),
            telemetry_stale_after: Duration::from_secs(180),
            min_sample_percent: 0.0,
            max_sample_percent: 25.0,
            min_cost_percent: 0.05,
            secondary_enforced_cap: Duration::from_secs(15 * 60),
        }
    }
}

impl SlowModeConfig {
    pub(crate) fn sanitized_cost(&self, cost: f64) -> f64 {
        if !cost.is_finite() || cost <= 0.0 {
            self.min_cost_percent
        } else {
            cost.max(self.min_cost_percent)
        }
    }
}
