//! Session-scoped pacing for Codex model inference.
//!
//! Slow Mode decides when the next billable model request may start. It does
//! not change the model, reasoning effort, tools, or prompt. Callers gate
//! requests through [`before_model_request`] and drop the returned permit only
//! after that request's response stream finishes.

mod config;
mod controller;
mod latch;
mod pacer;
mod registry;
mod simulate;
mod status;

pub use config::SlowModeConfig;
pub use controller::RateSnapshot;
pub use controller::UsageWindow;
pub use latch::FastSlowLatch;
pub use latch::TierDirective;
pub use pacer::GateError;
pub use pacer::GateTrace;
pub use pacer::InferencePermit;
pub use pacer::SlowModePacer;
pub use registry::before_model_request;
pub use registry::cancel_waits;
pub use registry::disable;
pub use registry::enable;
pub use registry::note_parent;
pub use registry::record_rate_limits;
pub use registry::reset_for_tests;
pub use registry::shutdown_thread;
pub use registry::status_for;
pub use simulate::SimulationReport;
pub use simulate::render_simulation_report;
pub use status::SlowModeAction;
pub use status::SlowModeStatus;
pub use status::format_duration;
pub use status::parse_slow_mode_args;
pub use status::render_status;

/// Slow Mode never writes Codex config. The next process starts unpaced.
pub const PERSISTS_TO_GLOBAL_CONFIG: bool = false;

/// Flex Processing is not enabled by Slow Mode.
///
/// Codex can already send `service_tier=flex` when a caller configures it and
/// the model catalog advertises that tier. This crate does not set that value.
/// Nothing in the current client establishes that Flex reduces a ChatGPT
/// subscription's five-hour allowance, so subscription sessions stay on
/// standard routing plus pacing.
pub const ENABLES_FLEX_PROCESSING: bool = false;
