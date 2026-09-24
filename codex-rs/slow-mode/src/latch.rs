/// Session memory for the Fast/Slow conflict.
///
/// Enabling Slow Mode forces the effective service tier off Fast for this
/// session only. The saved tier is restored when Slow Mode is turned off.
/// Nothing here writes user config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastSlowLatch {
    enabled: bool,
    /// Effective tier before Slow Mode, when we had to change it.
    saved_tier: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TierDirective {
    /// Leave the effective service tier alone.
    Unchanged,
    /// Apply this tier to the session without persisting config.
    /// `None` means the standard / default tier.
    SetSessionTier(Option<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatchOutcome {
    pub directive: TierDirective,
    pub message: String,
    pub accepted: bool,
}

impl Default for FastSlowLatch {
    fn default() -> Self {
        Self {
            enabled: false,
            saved_tier: None,
        }
    }
}

impl FastSlowLatch {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn enable(&mut self, current_tier: Option<&str>) -> LatchOutcome {
        if self.enabled {
            return LatchOutcome {
                directive: TierDirective::Unchanged,
                message: "Slow mode is already enabled for this session.".to_string(),
                accepted: true,
            };
        }
        self.enabled = true;
        if is_fast_tier(current_tier) {
            self.saved_tier = Some(current_tier.map(str::to_string));
            LatchOutcome {
                directive: TierDirective::SetSessionTier(None),
                message: "Slow mode enabled for this session. Fast mode is paused until Slow mode is turned off."
                    .to_string(),
                accepted: true,
            }
        } else {
            self.saved_tier = None;
            LatchOutcome {
                directive: TierDirective::Unchanged,
                message: "Slow mode enabled for this session.".to_string(),
                accepted: true,
            }
        }
    }

    pub fn disable(&mut self) -> LatchOutcome {
        if !self.enabled {
            return LatchOutcome {
                directive: TierDirective::Unchanged,
                message: "Slow mode is already off.".to_string(),
                accepted: true,
            };
        }
        self.enabled = false;
        let directive = match self.saved_tier.take() {
            Some(tier) => TierDirective::SetSessionTier(tier),
            None => TierDirective::Unchanged,
        };
        LatchOutcome {
            directive,
            message: "Slow mode disabled for this session.".to_string(),
            accepted: true,
        }
    }

    /// `/fast` while Slow Mode is on. This does not call a model.
    pub fn reject_fast(&self) -> Option<LatchOutcome> {
        if !self.enabled {
            return None;
        }
        Some(LatchOutcome {
            directive: TierDirective::Unchanged,
            message: "Fast mode stays off while Slow mode is on. Run /slow-mode off first."
                .to_string(),
            accepted: false,
        })
    }
}

pub fn is_fast_tier(tier: Option<&str>) -> bool {
    matches!(
        tier.map(str::to_ascii_lowercase).as_deref(),
        Some("fast" | "priority")
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn fast_is_paused_and_restored_without_a_second_mode() {
        let mut latch = FastSlowLatch::default();
        let enabled = latch.enable(Some("priority"));
        assert!(enabled.accepted);
        assert_eq!(enabled.directive, TierDirective::SetSessionTier(None));
        let rejected = latch.reject_fast().expect("fast conflicts");
        assert!(!rejected.accepted);
        assert_eq!(rejected.directive, TierDirective::Unchanged);
        assert!(latch.enabled());
        let disabled = latch.disable();
        assert_eq!(
            disabled.directive,
            TierDirective::SetSessionTier(Some("priority".to_string()))
        );
        assert!(!latch.enabled());
        assert!(latch.reject_fast().is_none());
    }

    #[test]
    fn standard_tier_is_left_alone() {
        let mut latch = FastSlowLatch::default();
        let enabled = latch.enable(Some("default"));
        assert_eq!(enabled.directive, TierDirective::Unchanged);
        let disabled = latch.disable();
        assert_eq!(disabled.directive, TierDirective::Unchanged);
    }
}
