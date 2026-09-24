use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::Notify;
use tokio::sync::OwnedMutexGuard;
use tokio_util::sync::CancellationToken;

use crate::config::SlowModeConfig;
use crate::controller::PaceDecision;
use crate::controller::RateSnapshot;
use crate::controller::SlowModeRateController;
use crate::status::SlowModeStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateTrace {
    pub wait_started_unix_ms: i64,
    pub granted_unix_ms: i64,
    pub waited_ms: u64,
}

#[derive(Debug)]
pub enum GateError {
    Cancelled,
}

pub struct InferencePermit {
    pub waited: Duration,
    pub granted_unix_ms: i64,
    /// Held until the response stream is dropped so the next model request waits.
    #[allow(dead_code)]
    guard: Option<OwnedMutexGuard<()>>,
    _inflight: InflightGuard,
}

impl InferencePermit {
    /// No concurrency slot and no wait. Used when Slow Mode is off.
    pub fn immediate() -> Self {
        Self {
            waited: Duration::ZERO,
            granted_unix_ms: 0,
            guard: None,
            _inflight: InflightGuard { inflight: None },
        }
    }
}

struct InflightGuard {
    inflight: Option<Arc<AtomicUsize>>,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        if let Some(inflight) = self.inflight.take() {
            inflight.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

enum Clock {
    System,
    Manual(ManualClock),
}

#[derive(Clone)]
pub struct ManualClock {
    inner: Arc<ManualInner>,
}

struct ManualInner {
    ms: Mutex<i64>,
    notify: Notify,
}

impl ManualClock {
    pub fn new(start_ms: i64) -> Self {
        Self {
            inner: Arc::new(ManualInner {
                ms: Mutex::new(start_ms),
                notify: Notify::new(),
            }),
        }
    }

    pub fn unix_ms(&self) -> i64 {
        *self
            .inner
            .ms
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn advance(&self, by: Duration) {
        let mut ms = self
            .inner
            .ms
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *ms += by.as_millis() as i64;
        drop(ms);
        self.inner.notify.notify_waiters();
    }

    async fn sleep(&self, duration: Duration) {
        if duration.is_zero() {
            return;
        }
        let deadline = self.unix_ms().saturating_add(duration.as_millis() as i64);
        loop {
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            if self.unix_ms() >= deadline {
                return;
            }
            notified.await;
        }
    }
}

/// Serializes model requests and waits only until the next sustainable time.
pub struct SlowModePacer {
    controller: Mutex<SlowModeRateController>,
    gate: Arc<AsyncMutex<()>>,
    notify: Notify,
    inflight: Arc<AtomicUsize>,
    shutdown: CancellationToken,
    /// Cancelled by a turn interrupt. Replaced immediately so the next request
    /// can wait again. Callers clone the current token before they sleep.
    turn_cancel: Mutex<CancellationToken>,
    traces: Mutex<Vec<GateTrace>>,
    clock: Clock,
}

impl SlowModePacer {
    pub fn new(config: SlowModeConfig) -> Arc<Self> {
        Arc::new(Self::from_clock(config, Clock::System))
    }

    pub fn manual(config: SlowModeConfig, clock: ManualClock) -> Arc<Self> {
        Arc::new(Self::from_clock(config, Clock::Manual(clock)))
    }

    fn from_clock(config: SlowModeConfig, clock: Clock) -> Self {
        Self {
            controller: Mutex::new(SlowModeRateController::new(config)),
            gate: Arc::new(AsyncMutex::new(())),
            notify: Notify::new(),
            inflight: Arc::new(AtomicUsize::new(0)),
            shutdown: CancellationToken::new(),
            turn_cancel: Mutex::new(CancellationToken::new()),
            traces: Mutex::new(Vec::new()),
            clock,
        }
    }

    pub fn enable(&self, limits: Option<RateSnapshot>) {
        let mut controller = self
            .controller
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        controller.enable(self.unix_ms(), limits);
        drop(controller);
        self.notify.notify_waiters();
    }

    pub fn disable(&self) {
        self.controller
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .disable();
        self.notify.notify_waiters();
    }

    pub fn shutdown(&self) {
        self.shutdown.cancel();
        self.notify.notify_waiters();
    }

    /// Abort pacing waits that are already queued. The following request uses a
    /// fresh token, so Slow Mode stays enabled after Ctrl-C.
    pub fn cancel_inflight_waits(&self) {
        let mut token = self
            .turn_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        token.cancel();
        *token = CancellationToken::new();
        drop(token);
        self.notify.notify_waiters();
    }

    fn turn_token(&self) -> CancellationToken {
        self.turn_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn observe(&self, snapshot: RateSnapshot) {
        self.controller
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .observe(snapshot);
        self.notify.notify_waiters();
    }

    pub fn enabled(&self) -> bool {
        self.controller
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_enabled()
    }

    pub fn status(&self) -> SlowModeStatus {
        let queued = self.inflight.load(Ordering::SeqCst).saturating_sub(1);
        self.controller
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .decide(self.unix_ms(), queued)
            .status
    }

    pub fn traces(&self) -> Vec<GateTrace> {
        self.traces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn current_banner(&self) -> Option<String> {
        let queued = self.inflight.load(Ordering::SeqCst).saturating_sub(1);
        let decision = self
            .controller
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .decide(self.unix_ms(), queued);
        if !self.enabled() || decision.wait.is_zero() {
            None
        } else {
            Some(decision.message)
        }
    }

    /// Block until this request may be sent. The permit is held until drop,
    /// which is when the response stream finishes.
    pub async fn acquire(
        &self,
        external_cancel: &CancellationToken,
        ui: &(dyn Fn(&str) + Send + Sync),
    ) -> Result<InferencePermit, GateError> {
        let inflight = InflightGuard {
            inflight: Some(Arc::clone(&self.inflight)),
        };
        self.inflight.fetch_add(1, Ordering::SeqCst);
        if !self.enabled() {
            return Ok(self.noop_permit(inflight));
        }
        let turn_cancel = self.turn_token();
        let guard = tokio::select! {
            biased;
            _ = external_cancel.cancelled() => return Err(GateError::Cancelled),
            _ = turn_cancel.cancelled() => return Err(GateError::Cancelled),
            _ = self.shutdown.cancelled() => return Err(GateError::Cancelled),
            guard = self.gate.clone().lock_owned() => guard,
        };
        if external_cancel.is_cancelled()
            || turn_cancel.is_cancelled()
            || self.shutdown.is_cancelled()
        {
            return Err(GateError::Cancelled);
        }
        if !self.enabled() {
            drop(guard);
            return Ok(self.noop_permit(inflight));
        }
        let started = self.unix_ms();
        let waited = self
            .wait_until_ready(external_cancel, &turn_cancel, ui)
            .await?;
        if !self.enabled() {
            drop(guard);
            return Ok(self.noop_permit(inflight));
        }
        let granted = self.unix_ms();
        self.controller
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .mark_granted(granted);
        self.traces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(GateTrace {
                wait_started_unix_ms: started,
                granted_unix_ms: granted,
                waited_ms: waited.as_millis() as u64,
            });
        Ok(InferencePermit {
            waited,
            granted_unix_ms: granted,
            guard: Some(guard),
            _inflight: inflight,
        })
    }

    fn noop_permit(&self, inflight: InflightGuard) -> InferencePermit {
        let now = self.unix_ms();
        InferencePermit {
            waited: Duration::ZERO,
            granted_unix_ms: now,
            guard: None,
            _inflight: inflight,
        }
    }

    async fn wait_until_ready(
        &self,
        external_cancel: &CancellationToken,
        turn_cancel: &CancellationToken,
        ui: &(dyn Fn(&str) + Send + Sync),
    ) -> Result<Duration, GateError> {
        let started = self.unix_ms();
        loop {
            if external_cancel.is_cancelled()
                || turn_cancel.is_cancelled()
                || self.shutdown.is_cancelled()
            {
                return Err(GateError::Cancelled);
            }
            if !self.enabled() {
                return Ok(Duration::ZERO);
            }
            let decision = self.decision();
            if decision.wait.is_zero() {
                return Ok(Duration::from_millis(
                    self.unix_ms().saturating_sub(started).max(0) as u64,
                ));
            }
            ui(&decision.message);
            let slice = if decision.wait > Duration::from_secs(60) {
                Duration::from_secs(15).min(decision.wait)
            } else {
                decision.wait.min(Duration::from_secs(1))
            };
            match self
                .sleep_slice(slice, external_cancel, turn_cancel)
                .await?
            {
                SleepEnd::Elapsed | SleepEnd::Woken => continue,
            }
        }
    }

    fn decision(&self) -> PaceDecision {
        let queued = self.inflight.load(Ordering::SeqCst).saturating_sub(1);
        self.controller
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .decide(self.unix_ms(), queued)
    }

    async fn sleep_slice(
        &self,
        duration: Duration,
        external_cancel: &CancellationToken,
        turn_cancel: &CancellationToken,
    ) -> Result<SleepEnd, GateError> {
        tokio::select! {
            biased;
            _ = external_cancel.cancelled() => Err(GateError::Cancelled),
            _ = turn_cancel.cancelled() => Err(GateError::Cancelled),
            _ = self.shutdown.cancelled() => Err(GateError::Cancelled),
            _ = self.notify.notified() => Ok(SleepEnd::Woken),
            _ = self.sleep_clock(duration) => Ok(SleepEnd::Elapsed),
        }
    }

    async fn sleep_clock(&self, duration: Duration) {
        match &self.clock {
            Clock::System => tokio::time::sleep(duration).await,
            Clock::Manual(clock) => clock.sleep(duration).await,
        }
    }

    fn unix_ms(&self) -> i64 {
        match &self.clock {
            Clock::System => system_unix_ms(),
            Clock::Manual(clock) => clock.unix_ms(),
        }
    }
}

enum SleepEnd {
    Elapsed,
    Woken,
}

fn system_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use crate::controller::UsageWindow;

    fn limits(now_ms: i64, used: f64, reset_secs: i64) -> RateSnapshot {
        RateSnapshot {
            primary: Some(UsageWindow {
                used_percent: used,
                window_minutes: Some(300),
                resets_at_unix_secs: Some(reset_secs),
            }),
            secondary: None,
            observed_unix_ms: now_ms,
        }
    }

    #[tokio::test]
    async fn wait_finishes_before_the_permit_is_granted() {
        let clock = ManualClock::new(1_000_000);
        let pacer = SlowModePacer::manual(SlowModeConfig::default(), clock.clone());
        let reset = clock.unix_ms() / 1000 + 3 * 60 * 60;
        pacer.enable(Some(limits(clock.unix_ms(), 40.0, reset)));
        pacer.observe(limits(clock.unix_ms(), 41.0, reset));
        let pacer_task = Arc::clone(&pacer);
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            pacer_task
                .acquire(&task_cancel, &|_| {})
                .await
                .expect("grant")
        });
        // First request is immediate when allowance remains.
        let first = handle.await.expect("task");
        assert!(first.granted_unix_ms >= clock.unix_ms() - 1);
        drop(first);

        let pacer_task = Arc::clone(&pacer);
        let task_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            pacer_task
                .acquire(&task_cancel, &|_| {})
                .await
                .expect("second grant")
        });
        let second = drive_manual_clock(clock.clone(), handle).await;
        let trace = pacer.traces();
        let last = trace.last().expect("trace");
        assert!(last.granted_unix_ms >= last.wait_started_unix_ms);
        assert!(last.waited_ms > 0);
        assert!(second.granted_unix_ms >= last.wait_started_unix_ms);
        let mut inference_started = false;
        if second.granted_unix_ms >= last.wait_started_unix_ms {
            inference_started = true;
        }
        assert!(inference_started);
    }

    #[tokio::test]
    async fn parallel_acquires_run_serially_and_are_not_dropped() {
        let clock = ManualClock::new(0);
        let mut config = SlowModeConfig::default();
        config.fallback_interval = Duration::ZERO;
        let pacer = SlowModePacer::manual(config, clock);
        pacer.enable(None);
        let order = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancellationToken::new();
        let first = pacer.acquire(&cancel, &|_| {}).await.expect("first");
        let second_pacer = Arc::clone(&pacer);
        let second_cancel = cancel.clone();
        let second_order = Arc::clone(&order);
        let second = tokio::spawn(async move {
            let permit = second_pacer
                .acquire(&second_cancel, &|_| {})
                .await
                .expect("second");
            second_order.lock().expect("order").push(2);
            drop(permit);
        });
        tokio::task::yield_now().await;
        order.lock().expect("order").push(1);
        drop(first);
        tokio::time::timeout(Duration::from_secs(2), second)
            .await
            .expect("queued subagent request should run, not be dropped")
            .expect("second joined");
        assert_eq!(order.lock().expect("order").as_slice(), &[1, 2]);
    }

    async fn drive_manual_clock<T>(
        clock: ManualClock,
        mut handle: tokio::task::JoinHandle<T>,
    ) -> T {
        for _ in 0..600 {
            match tokio::time::timeout(Duration::from_millis(5), &mut handle).await {
                Ok(result) => return result.expect("paced task"),
                Err(_) => clock.advance(Duration::from_secs(5)),
            }
        }
        panic!("manual clock did not reach the next grant");
    }

    #[tokio::test]
    async fn cancellation_interrupts_the_wait() {
        let clock = ManualClock::new(0);
        let pacer = SlowModePacer::manual(SlowModeConfig::default(), clock.clone());
        pacer.enable(Some(limits(0, 96.0, 3_600)));
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task_pacer = Arc::clone(&pacer);
        let handle = tokio::spawn(async move { task_pacer.acquire(&task_cancel, &|_| {}).await });
        tokio::task::yield_now().await;
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("cancel should not hang")
            .expect("task");
        assert!(matches!(result, Err(GateError::Cancelled)));
    }

    #[tokio::test]
    async fn turn_interrupt_cancels_the_wait_without_disabling_slow_mode() {
        let clock = ManualClock::new(0);
        let pacer = SlowModePacer::manual(SlowModeConfig::default(), clock);
        pacer.enable(Some(limits(0, 96.0, 3_600)));
        let task_pacer = Arc::clone(&pacer);
        let handle =
            tokio::spawn(
                async move { task_pacer.acquire(&CancellationToken::new(), &|_| {}).await },
            );
        tokio::task::yield_now().await;
        pacer.cancel_inflight_waits();
        let result = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("interrupt should not hang")
            .expect("task");
        assert!(matches!(result, Err(GateError::Cancelled)));
        assert!(pacer.enabled());
    }

    #[tokio::test]
    async fn disabling_interrupts_the_wait_and_allows_the_request() {
        let clock = ManualClock::new(0);
        let pacer = SlowModePacer::manual(SlowModeConfig::default(), clock);
        pacer.enable(Some(limits(0, 96.0, 3_600)));
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task_pacer = Arc::clone(&pacer);
        let handle = tokio::spawn(async move { task_pacer.acquire(&task_cancel, &|_| {}).await });
        tokio::task::yield_now().await;
        pacer.disable();
        let permit = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("disable should wake the wait")
            .expect("task")
            .expect("proceed");
        assert!(permit.guard.is_none());
    }

    #[tokio::test]
    async fn disabled_pacer_does_not_serialize() {
        let clock = ManualClock::new(0);
        let pacer = SlowModePacer::manual(SlowModeConfig::default(), clock);
        let cancel = CancellationToken::new();
        let a = {
            let pacer = Arc::clone(&pacer);
            let cancel = cancel.clone();
            tokio::spawn(async move { pacer.acquire(&cancel, &|_| {}).await.expect("a") })
        };
        let b = {
            let pacer = Arc::clone(&pacer);
            let cancel = cancel.clone();
            tokio::spawn(async move { pacer.acquire(&cancel, &|_| {}).await.expect("b") })
        };
        let (a, b) = tokio::time::timeout(Duration::from_secs(2), async { tokio::join!(a, b) })
            .await
            .expect("disabled slow mode should not block");
        assert!(a.expect("a").guard.is_none());
        assert!(b.expect("b").guard.is_none());
    }
}
