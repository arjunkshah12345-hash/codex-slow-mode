//! Process-local footer badge for Slow Mode.
//!
//! The pacer runs in the app-server process. The TUI only needs a compact
//! indicator, and the footer is redrawn from composer state that does not own
//! the session latch. One Codex TUI process has one active session badge.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

static SLOW_MODE_BADGE: AtomicBool = AtomicBool::new(false);

pub(crate) fn set(enabled: bool) {
    SLOW_MODE_BADGE.store(enabled, Ordering::Relaxed);
}

pub(crate) fn is_on() -> bool {
    SLOW_MODE_BADGE.load(Ordering::Relaxed)
}
