//! Watch API streaming for real-time resource updates.
//!
//! Provides a background watcher task that streams Kubernetes Watch API events
//! (ADDED/MODIFIED/DELETED) for the currently active dynamic resource view.
//!
//! Lifecycle:
//! - Watch starts when user switches to a dynamic resource view (`:deploy`, `:pods`, etc.)
//! - Watch stops when user leaves the view (switches to another resource or static view)
//! - Incremental reconciliation: ADDED inserts, MODIFIED updates, DELETED removes
//!
//! Uses kube-rs `watcher()` runtime which handles reconnection and bookmark tracking
//! automatically, giving us a reliable event stream over the Watch API.

use futures::StreamExt;
use kube::api::{ApiResource, DynamicObject};
use kube::runtime::watcher::{self, Event as WatcherEvent};
use kube::{Api, Client};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A single watch event delivered to the main loop for incremental reconciliation.
#[derive(Debug)]
pub enum WatchEvent {
    /// Initial list complete — replace all objects with this full set.
    InitialList(Vec<DynamicObject>),
    /// A resource was added or modified — upsert into the object list.
    Applied(DynamicObject),
    /// A resource was deleted — remove from the object list.
    Deleted(DynamicObject),
    /// Watch stream encountered an error (will auto-reconnect).
    Error(String),
}

/// Parameters for starting a watch.
#[derive(Clone)]
pub struct WatchParams {
    pub client: Client,
    pub api_resource: ApiResource,
    pub namespaced: bool,
    pub namespace: Option<String>,
    /// Generation counter for stale event detection.
    pub generation: u64,
}

impl std::fmt::Debug for WatchParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchParams")
            .field("api_resource", &self.api_resource)
            .field("namespaced", &self.namespaced)
            .field("namespace", &self.namespace)
            .field("generation", &self.generation)
            .finish()
    }
}

/// A watch event tagged with its generation for stale detection.
#[derive(Debug)]
pub struct TaggedWatchEvent {
    pub event: WatchEvent,
    pub generation: u64,
}

/// Start a background watcher task that streams events via the provided channel.
///
/// Returns a `CancellationToken` that can be used to stop the watcher.
/// The watcher will also stop if the channel receiver is dropped.
pub fn start_watcher(params: WatchParams, tx: mpsc::Sender<TaggedWatchEvent>) -> CancellationToken {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        run_watcher(params, tx, cancel_clone).await;
    });

    cancel
}

/// Internal watcher loop — runs until cancelled or channel closed.
async fn run_watcher(
    params: WatchParams,
    tx: mpsc::Sender<TaggedWatchEvent>,
    cancel: CancellationToken,
) {
    let api: Api<DynamicObject> = if params.namespaced {
        match &params.namespace {
            Some(ns) => Api::namespaced_with(params.client.clone(), ns, &params.api_resource),
            None => Api::all_with(params.client.clone(), &params.api_resource),
        }
    } else {
        Api::all_with(params.client.clone(), &params.api_resource)
    };

    let watcher_config = watcher::Config::default().any_semantic();
    let mut stream = watcher::watcher(api, watcher_config).boxed();

    // Accumulate InitApply events, then send as InitialList on InitDone
    let mut init_buffer: Vec<DynamicObject> = Vec::new();
    let mut in_init_phase = true;

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                break;
            }
            maybe_event = stream.next() => {
                match maybe_event {
                    Some(Ok(event)) => {
                        let watch_event = match event {
                            WatcherEvent::Apply(obj) if in_init_phase => {
                                // During init phase, buffer objects
                                init_buffer.push(obj);
                                continue;
                            }
                            WatcherEvent::Apply(obj) => {
                                // Post-init: this is a real-time ADDED/MODIFIED
                                WatchEvent::Applied(obj)
                            }
                            WatcherEvent::InitApply(obj) => {
                                // Explicit init-phase object
                                init_buffer.push(obj);
                                continue;
                            }
                            WatcherEvent::Init => {
                                // Start of a new init sequence (re-list)
                                init_buffer.clear();
                                in_init_phase = true;
                                continue;
                            }
                            WatcherEvent::InitDone => {
                                // Init complete — send full list
                                in_init_phase = false;
                                let objects = std::mem::take(&mut init_buffer);
                                WatchEvent::InitialList(objects)
                            }
                            WatcherEvent::Delete(obj) => {
                                WatchEvent::Deleted(obj)
                            }
                        };
                        let tagged = TaggedWatchEvent {
                            event: watch_event,
                            generation: params.generation,
                        };
                        if tx.send(tagged).await.is_err() {
                            // Receiver dropped — stop watcher
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        let tagged = TaggedWatchEvent {
                            event: WatchEvent::Error(format!("Watch error: {}", e)),
                            generation: params.generation,
                        };
                        // Send error but don't break — watcher() handles reconnection
                        let _ = tx.send(tagged).await;
                    }
                    None => {
                        // Stream ended (shouldn't happen with watcher() — it reconnects)
                        break;
                    }
                }
            }
        }
    }
}

/// Debounce window for batching watch events before sending to the TUI loop.
/// Events arriving within this window are accumulated and sent as a single batch.
const DEBOUNCE_MS: u64 = 200;

/// Maximum events to buffer before forcing a flush (prevents unbounded growth during storms).
const MAX_DEBOUNCE_BATCH: usize = 200;

/// Start a debounced background watcher that batches rapid-fire events.
///
/// Instead of sending every individual watch event, this buffers events within a
/// configurable debounce window and flushes them as a batch. This prevents the TUI
/// from redrawing on every single MODIFIED event during pod churn or event storms.
///
/// Returns a `CancellationToken` that can be used to stop the watcher.
pub fn start_debounced_watcher(
    params: WatchParams,
    tx: mpsc::Sender<TaggedWatchEvent>,
) -> CancellationToken {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        run_debounced_watcher(params, tx, cancel_clone).await;
    });

    cancel
}

/// Internal debounced watcher loop — buffers events and flushes after debounce window.
async fn run_debounced_watcher(
    params: WatchParams,
    tx: mpsc::Sender<TaggedWatchEvent>,
    cancel: CancellationToken,
) {
    let api: Api<DynamicObject> = if params.namespaced {
        match &params.namespace {
            Some(ns) => Api::namespaced_with(params.client.clone(), ns, &params.api_resource),
            None => Api::all_with(params.client.clone(), &params.api_resource),
        }
    } else {
        Api::all_with(params.client.clone(), &params.api_resource)
    };

    let watcher_config = watcher::Config::default().any_semantic();
    let mut stream = watcher::watcher(api, watcher_config).boxed();

    // Event buffer for debouncing
    let mut batch: Vec<WatchEvent> = Vec::new();
    let mut last_flush = Instant::now();
    // Accumulate InitApply events, then send as InitialList on InitDone
    let mut init_buffer: Vec<DynamicObject> = Vec::new();
    let mut in_init_phase = true;

    loop {
        // Determine timeout: if batch is non-empty, wait only until debounce window expires
        let timeout_dur = if batch.is_empty() {
            Duration::from_millis(500) // idle: just check cancellation periodically
        } else {
            let elapsed = last_flush.elapsed();
            Duration::from_millis(DEBOUNCE_MS).saturating_sub(elapsed)
        };

        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                // Flush remaining batch before exit
                flush_batch(&mut batch, &tx, params.generation).await;
                break;
            }
            maybe_event = stream.next() => {
                match maybe_event {
                    Some(Ok(event)) => {
                        let watch_event = match event {
                            WatcherEvent::Apply(obj) if in_init_phase => {
                                init_buffer.push(obj);
                                continue;
                            }
                            WatcherEvent::Apply(obj) => WatchEvent::Applied(obj),
                            WatcherEvent::InitApply(obj) => {
                                init_buffer.push(obj);
                                continue;
                            }
                            WatcherEvent::Init => {
                                init_buffer.clear();
                                in_init_phase = true;
                                continue;
                            }
                            WatcherEvent::InitDone => {
                                in_init_phase = false;
                                let objects = std::mem::take(&mut init_buffer);
                                // InitialList is sent immediately (no debounce) since it
                                // replaces all data and the user needs to see it right away.
                                let tagged = TaggedWatchEvent {
                                    event: WatchEvent::InitialList(objects),
                                    generation: params.generation,
                                };
                                if tx.send(tagged).await.is_err() {
                                    return;
                                }
                                last_flush = Instant::now();
                                continue;
                            }
                            WatcherEvent::Delete(obj) => WatchEvent::Deleted(obj),
                        };

                        if batch.is_empty() {
                            last_flush = Instant::now();
                        }
                        batch.push(watch_event);

                        // Force-flush on batch size limit
                        if batch.len() >= MAX_DEBOUNCE_BATCH {
                            flush_batch(&mut batch, &tx, params.generation).await;
                            last_flush = Instant::now();
                        }
                    }
                    Some(Err(e)) => {
                        // Flush any pending batch, then send error
                        flush_batch(&mut batch, &tx, params.generation).await;
                        let tagged = TaggedWatchEvent {
                            event: WatchEvent::Error(format!("Watch error: {}", e)),
                            generation: params.generation,
                        };
                        let _ = tx.send(tagged).await;
                        last_flush = Instant::now();
                    }
                    None => {
                        flush_batch(&mut batch, &tx, params.generation).await;
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(timeout_dur) => {
                // Debounce timeout expired — flush batch
                if !batch.is_empty() {
                    flush_batch(&mut batch, &tx, params.generation).await;
                    last_flush = Instant::now();
                }
            }
        }
    }
}

/// Flush accumulated watch events through the channel.
async fn flush_batch(
    batch: &mut Vec<WatchEvent>,
    tx: &mpsc::Sender<TaggedWatchEvent>,
    generation: u64,
) {
    for event in batch.drain(..) {
        let tagged = TaggedWatchEvent { event, generation };
        if tx.send(tagged).await.is_err() {
            return; // Receiver dropped
        }
    }
}

/// Apply incremental reconciliation to a DynamicObject list.
///
/// Returns `true` if the list was modified (caller should re-extract rows).
pub fn reconcile_objects(objects: &mut Vec<DynamicObject>, event: WatchEvent) -> bool {
    match event {
        WatchEvent::InitialList(new_objects) => {
            *objects = new_objects;
            true
        }
        WatchEvent::Applied(obj) => {
            // Upsert: find by (namespace, name) and replace, or insert
            let key = object_key(&obj);
            if let Some(pos) = objects.iter().position(|o| object_key(o) == key) {
                objects[pos] = obj;
            } else {
                objects.push(obj);
            }
            true
        }
        WatchEvent::Deleted(obj) => {
            let key = object_key(&obj);
            let before = objects.len();
            objects.retain(|o| object_key(o) != key);
            objects.len() != before
        }
        WatchEvent::Error(_) => {
            // Errors don't modify the object list
            false
        }
    }
}

/// Unique key for a DynamicObject: (namespace, name).
fn object_key(obj: &DynamicObject) -> (Option<&str>, Option<&str>) {
    (
        obj.metadata.namespace.as_deref(),
        obj.metadata.name.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn make_obj(name: &str, ns: Option<&str>) -> DynamicObject {
        DynamicObject {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: ns.map(|s| s.to_string()),
                ..Default::default()
            },
            types: None,
            data: serde_json::json!({}),
        }
    }

    #[test]
    fn reconcile_initial_list_replaces_all() {
        let mut objects = vec![make_obj("old", Some("default"))];
        let changed = reconcile_objects(
            &mut objects,
            WatchEvent::InitialList(vec![
                make_obj("new1", Some("default")),
                make_obj("new2", Some("default")),
            ]),
        );
        assert!(changed);
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].metadata.name.as_deref(), Some("new1"));
    }

    #[test]
    fn reconcile_applied_inserts_new() {
        let mut objects = vec![make_obj("existing", Some("default"))];
        let changed = reconcile_objects(
            &mut objects,
            WatchEvent::Applied(make_obj("new-pod", Some("default"))),
        );
        assert!(changed);
        assert_eq!(objects.len(), 2);
    }

    #[test]
    fn reconcile_applied_updates_existing() {
        let mut objects = vec![
            make_obj("pod-a", Some("default")),
            make_obj("pod-b", Some("default")),
        ];
        let mut updated = make_obj("pod-a", Some("default"));
        updated.data = serde_json::json!({"status": {"phase": "Succeeded"}});

        let changed = reconcile_objects(&mut objects, WatchEvent::Applied(updated));
        assert!(changed);
        assert_eq!(objects.len(), 2);
        // pod-a should be updated in-place
        assert_eq!(
            objects[0].data.get("status").unwrap().get("phase").unwrap(),
            "Succeeded"
        );
    }

    #[test]
    fn reconcile_deleted_removes() {
        let mut objects = vec![
            make_obj("pod-a", Some("default")),
            make_obj("pod-b", Some("default")),
        ];
        let changed = reconcile_objects(
            &mut objects,
            WatchEvent::Deleted(make_obj("pod-a", Some("default"))),
        );
        assert!(changed);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].metadata.name.as_deref(), Some("pod-b"));
    }

    #[test]
    fn reconcile_deleted_nonexistent_noop() {
        let mut objects = vec![make_obj("pod-a", Some("default"))];
        let changed = reconcile_objects(
            &mut objects,
            WatchEvent::Deleted(make_obj("nonexistent", Some("default"))),
        );
        assert!(!changed);
        assert_eq!(objects.len(), 1);
    }

    #[test]
    fn reconcile_error_noop() {
        let mut objects = vec![make_obj("pod-a", Some("default"))];
        let changed = reconcile_objects(&mut objects, WatchEvent::Error("some error".to_string()));
        assert!(!changed);
        assert_eq!(objects.len(), 1);
    }

    /// Simulates a batch of events applied in sequence (as the main loop does after
    /// draining the watch channel). Verifies that batched reconciliation produces
    /// the correct final state.
    #[test]
    fn reconcile_batch_events_produces_correct_state() {
        let mut objects = Vec::new();

        // 1. Initial list with 3 pods
        reconcile_objects(
            &mut objects,
            WatchEvent::InitialList(vec![
                make_obj("pod-a", Some("default")),
                make_obj("pod-b", Some("default")),
                make_obj("pod-c", Some("default")),
            ]),
        );
        assert_eq!(objects.len(), 3);

        // 2. Batch: modify pod-a, delete pod-b, add pod-d
        let events = vec![
            WatchEvent::Applied({
                let mut obj = make_obj("pod-a", Some("default"));
                obj.data = serde_json::json!({"status": {"phase": "Succeeded"}});
                obj
            }),
            WatchEvent::Deleted(make_obj("pod-b", Some("default"))),
            WatchEvent::Applied(make_obj("pod-d", Some("default"))),
        ];

        for event in events {
            reconcile_objects(&mut objects, event);
        }

        // Final state: pod-a (modified), pod-c, pod-d
        assert_eq!(objects.len(), 3);
        let names: Vec<&str> = objects
            .iter()
            .filter_map(|o| o.metadata.name.as_deref())
            .collect();
        assert!(names.contains(&"pod-a"));
        assert!(!names.contains(&"pod-b")); // deleted
        assert!(names.contains(&"pod-c"));
        assert!(names.contains(&"pod-d")); // added
                                           // pod-a should be the modified version
        let pod_a = objects
            .iter()
            .find(|o| o.metadata.name.as_deref() == Some("pod-a"))
            .unwrap();
        assert_eq!(
            pod_a.data.get("status").unwrap().get("phase").unwrap(),
            "Succeeded"
        );
    }

    /// Verifies that a re-list (Init → InitApply... → InitDone pattern)
    /// correctly replaces all data via InitialList event.
    #[test]
    fn reconcile_relist_replaces_stale_data() {
        let mut objects = vec![
            make_obj("stale-1", Some("default")),
            make_obj("stale-2", Some("default")),
        ];

        // Re-list delivers only the current set
        reconcile_objects(
            &mut objects,
            WatchEvent::InitialList(vec![make_obj("fresh-1", Some("default"))]),
        );
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].metadata.name.as_deref(), Some("fresh-1"));
    }

    /// Ensures debounce constants are reasonable values.
    #[test]
    fn debounce_constants_are_reasonable() {
        const { assert!(DEBOUNCE_MS >= 100, "debounce too aggressive (< 100ms)") };
        const { assert!(DEBOUNCE_MS <= 500, "debounce too slow (> 500ms)") };
        const { assert!(MAX_DEBOUNCE_BATCH >= 50, "batch too small") };
        const { assert!(MAX_DEBOUNCE_BATCH <= 1000, "batch too large") };
    }
}
