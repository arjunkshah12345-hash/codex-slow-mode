use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

use tokio_util::sync::CancellationToken;

use crate::config::SlowModeConfig;
use crate::controller::RateSnapshot;
use crate::pacer::GateError;
use crate::pacer::InferencePermit;
use crate::pacer::SlowModePacer;
use crate::status::SlowModeStatus;

struct Node {
    parent: Option<String>,
    pacer: Option<std::sync::Arc<SlowModePacer>>,
    limits: Option<RateSnapshot>,
}

struct Registry {
    nodes: Mutex<HashMap<String, Node>>,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Registry {
        nodes: Mutex::new(HashMap::new()),
    })
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, Node>> {
    registry()
        .nodes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn disabled_pacer() -> std::sync::Arc<SlowModePacer> {
    SlowModePacer::new(SlowModeConfig::default())
}

/// Remember a child thread so its model requests share the parent's pacer.
pub fn note_parent(thread_id: &str, parent_thread_id: Option<&str>) {
    let Some(parent) = parent_thread_id else {
        return;
    };
    if parent.is_empty() || parent == thread_id {
        return;
    }
    let mut nodes = lock();
    let node = nodes.entry(thread_id.to_string()).or_insert(Node {
        parent: None,
        pacer: None,
        limits: None,
    });
    node.parent = Some(parent.to_string());
}

pub fn record_rate_limits(thread_id: &str, snapshot: RateSnapshot) {
    let pacer = {
        let mut nodes = lock();
        if let Some(node) = nodes.get_mut(thread_id) {
            node.limits = Some(snapshot.clone());
        } else {
            nodes.insert(
                thread_id.to_string(),
                Node {
                    parent: None,
                    pacer: None,
                    limits: Some(snapshot.clone()),
                },
            );
        }
        resolve_pacer(&nodes, thread_id)
    };
    if let Some(pacer) = pacer {
        pacer.observe(snapshot);
    }
}

pub fn enable(thread_id: &str) -> SlowModeStatus {
    let limits = {
        let nodes = lock();
        nodes.get(thread_id).and_then(|node| node.limits.clone())
    };
    let pacer = {
        let mut nodes = lock();
        let node = nodes.entry(thread_id.to_string()).or_insert(Node {
            parent: None,
            pacer: None,
            limits: None,
        });
        if node.pacer.is_none() {
            node.pacer = Some(SlowModePacer::new(SlowModeConfig::default()));
        }
        node.pacer.clone().unwrap_or_else(disabled_pacer)
    };
    pacer.enable(limits);
    pacer.status()
}

pub fn disable(thread_id: &str) -> SlowModeStatus {
    let pacer = {
        let nodes = lock();
        resolve_pacer(&nodes, thread_id)
    };
    if let Some(pacer) = pacer {
        pacer.disable();
        return pacer.status();
    }
    SlowModeStatus {
        enabled: false,
        model: None,
        fast_mode: None,
        primary_used_percent: None,
        primary_resets_in: None,
        secondary_used_percent: None,
        secondary_resets_in: None,
        estimated_request_cost_percent: None,
        next_eligible_in: None,
        queued_model_requests: 0,
        preserving_secondary: false,
        note: None,
    }
}

pub fn cancel_waits(thread_id: &str) {
    let pacer = {
        let nodes = lock();
        resolve_pacer(&nodes, thread_id)
    };
    if let Some(pacer) = pacer {
        pacer.cancel_inflight_waits();
    }
}

pub fn shutdown_thread(thread_id: &str) {
    let pacer = {
        let nodes = lock();
        resolve_pacer(&nodes, thread_id)
    };
    if let Some(pacer) = pacer {
        pacer.shutdown();
    }
}

pub fn status_for(thread_id: &str) -> SlowModeStatus {
    let nodes = lock();
    if let Some(pacer) = resolve_pacer(&nodes, thread_id) {
        return pacer.status();
    }
    disable_status()
}

pub async fn before_model_request(
    thread_id: &str,
    parent_thread_id: Option<&str>,
    ui: &(dyn Fn(&str) + Send + Sync),
) -> Result<InferencePermit, GateError> {
    note_parent(thread_id, parent_thread_id);
    let pacer = {
        let nodes = lock();
        resolve_pacer(&nodes, thread_id)
    };
    let Some(pacer) = pacer else {
        return Ok(InferencePermit::immediate());
    };
    if !pacer.enabled() {
        return Ok(InferencePermit::immediate());
    }
    pacer.acquire(&CancellationToken::new(), ui).await
}

pub fn reset_for_tests() {
    lock().clear();
}

fn resolve_pacer(
    nodes: &HashMap<String, Node>,
    thread_id: &str,
) -> Option<std::sync::Arc<SlowModePacer>> {
    let mut current = thread_id.to_string();
    for _ in 0..16 {
        let node = nodes.get(&current)?;
        if let Some(pacer) = &node.pacer {
            return Some(std::sync::Arc::clone(pacer));
        }
        current = node.parent.clone()?;
    }
    None
}

fn disable_status() -> SlowModeStatus {
    SlowModeStatus {
        enabled: false,
        model: None,
        fast_mode: None,
        primary_used_percent: None,
        primary_resets_in: None,
        secondary_used_percent: None,
        secondary_resets_in: None,
        estimated_request_cost_percent: None,
        next_eligible_in: None,
        queued_model_requests: 0,
        preserving_secondary: false,
        note: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PERSISTS_TO_GLOBAL_CONFIG;

    #[test]
    fn session_enable_does_not_persist_global_config() {
        assert!(!PERSISTS_TO_GLOBAL_CONFIG);
    }

    #[tokio::test]
    async fn child_thread_uses_the_parent_pacer() {
        reset_for_tests();
        enable("root-thread");
        note_parent("child-thread", Some("root-thread"));
        let parent = status_for("root-thread");
        let child_pacer_enabled = status_for("child-thread").enabled;
        assert!(parent.enabled);
        assert!(child_pacer_enabled);
        disable("root-thread");
        assert!(!status_for("child-thread").enabled);
        reset_for_tests();
    }
}
