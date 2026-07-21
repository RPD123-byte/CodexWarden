//! Authoritative lifecycle reducer and retained lifecycle replay.

use protocol::{Plane, Sequence, SequencedEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::sync::{RwLock, oneshot};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ThreadState {
    pub thread_id: String,
    pub status: String,
    pub active_turn_id: Option<String>,
    pub subscribed: bool,
    pub ephemeral: bool,
    pub last_sequence: Sequence,
    pub raw_thread: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub at_sequence: Sequence,
    pub threads: HashMap<String, ThreadState>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReplayResult {
    Events(Vec<Arc<SequencedEvent>>),
    GapTooOld {
        requested_after: Sequence,
        oldest_available: Option<Sequence>,
        snapshot: Snapshot,
    },
}

type Predicate = Box<dyn Fn(&SequencedEvent) -> bool + Send + Sync>;

struct Waiter {
    predicate: Predicate,
    sender: oneshot::Sender<Arc<SequencedEvent>>,
}

#[derive(Default)]
struct ReducerState {
    snapshot: Snapshot,
    subscribing: HashSet<String>,
}

#[derive(Clone)]
pub struct Reducer {
    state: Arc<RwLock<ReducerState>>,
    lifecycle: Arc<RwLock<VecDeque<Arc<SequencedEvent>>>>,
    lifecycle_capacity: usize,
    waiters: Arc<Mutex<Vec<Waiter>>>,
}

impl Reducer {
    pub fn new(lifecycle_capacity: usize) -> Self {
        assert!(lifecycle_capacity > 0);
        Self {
            state: Arc::new(RwLock::new(ReducerState::default())),
            lifecycle: Arc::new(RwLock::new(VecDeque::with_capacity(lifecycle_capacity))),
            lifecycle_capacity,
            waiters: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn apply(&self, event: Arc<SequencedEvent>) {
        {
            let mut state = self.state.write().await;
            state.snapshot.at_sequence = state.snapshot.at_sequence.max(event.sequence);
            if let Some(thread_id) = &event.thread_id {
                let method = event.method().unwrap_or_default();
                let subscribing = state.subscribing.contains(thread_id);
                let prune = {
                    let thread = state
                        .snapshot
                        .threads
                        .entry(thread_id.clone())
                        .or_insert_with(|| ThreadState {
                            thread_id: thread_id.clone(),
                            status: "unknown".into(),
                            ..ThreadState::default()
                        });
                    thread.last_sequence = event.sequence;
                    match method {
                        "thread/started" => {
                            thread.status = "idle".into();
                            thread.raw_thread =
                                event.frame.params().and_then(|p| p.get("thread")).cloned();
                            thread.ephemeral = thread
                                .raw_thread
                                .as_ref()
                                .and_then(|v| v.get("ephemeral"))
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                        }
                        "thread/status/changed" => {
                            thread.status = event
                                .frame
                                .params()
                                .and_then(|p| p.get("status"))
                                .and_then(|s| {
                                    s.get("type").and_then(Value::as_str).or_else(|| s.as_str())
                                })
                                .unwrap_or("unknown")
                                .to_owned();
                        }
                        "turn/started" => {
                            thread.status = "active".into();
                            thread.active_turn_id.clone_from(&event.turn_id);
                        }
                        "turn/completed" => {
                            if thread.active_turn_id.as_ref() == event.turn_id.as_ref()
                                || event.turn_id.is_none()
                            {
                                thread.active_turn_id = None;
                            }
                            thread.status = "idle".into();
                        }
                        _ => {}
                    }
                    !subscribing && Self::is_prunable(thread)
                };
                if prune {
                    state.snapshot.threads.remove(thread_id);
                }
            }
        }

        if event.plane == Plane::Lifecycle {
            let mut lifecycle = self.lifecycle.write().await;
            lifecycle.push_back(event.clone());
            while lifecycle.len() > self.lifecycle_capacity {
                lifecycle.pop_front();
            }
        }

        let mut ready = Vec::new();
        {
            let mut waiters = self.waiters.lock().expect("waiter lock poisoned");
            let mut index = 0;
            while index < waiters.len() {
                if (waiters[index].predicate)(&event) {
                    ready.push(waiters.swap_remove(index));
                } else {
                    index += 1;
                }
            }
        }
        for waiter in ready {
            let _ = waiter.sender.send(event.clone());
        }
    }

    pub fn register_waiter<F>(&self, predicate: F) -> oneshot::Receiver<Arc<SequencedEvent>>
    where
        F: Fn(&SequencedEvent) -> bool + Send + Sync + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        self.waiters
            .lock()
            .expect("waiter lock poisoned")
            .push(Waiter {
                predicate: Box::new(predicate),
                sender,
            });
        receiver
    }

    pub async fn snapshot(&self) -> Snapshot {
        self.state.read().await.snapshot.clone()
    }

    pub async fn current_sequence(&self) -> Sequence {
        self.state.read().await.snapshot.at_sequence
    }

    pub async fn events_since(&self, sequence: Sequence) -> ReplayResult {
        let log = self.lifecycle.read().await;
        let oldest = log.front().map(|event| event.sequence);
        if oldest.is_some_and(|oldest| sequence.saturating_add(1) < oldest) {
            drop(log);
            return ReplayResult::GapTooOld {
                requested_after: sequence,
                oldest_available: oldest,
                snapshot: self.snapshot().await,
            };
        }
        ReplayResult::Events(
            log.iter()
                .filter(|event| event.sequence > sequence)
                .cloned()
                .collect(),
        )
    }

    pub async fn set_subscribed(&self, thread_id: &str, subscribed: bool) {
        if subscribed {
            self.finish_subscription(thread_id).await;
            return;
        }
        let mut state = self.state.write().await;
        state.subscribing.remove(thread_id);
        let prune = state
            .snapshot
            .threads
            .get_mut(thread_id)
            .is_some_and(|thread| {
                thread.subscribed = false;
                Self::is_prunable(thread)
            });
        if prune {
            state.snapshot.threads.remove(thread_id);
        }
    }

    /// Marks a thread relevant before the async resume/read subscription sequence starts.
    /// This protection lives under the same lock as eviction, so it cannot be missed by a
    /// stale external snapshot of subscription state.
    pub async fn begin_subscription(&self, thread_id: &str) {
        let mut state = self.state.write().await;
        state.subscribing.insert(thread_id.to_owned());
        state
            .snapshot
            .threads
            .entry(thread_id.to_owned())
            .or_insert_with(|| ThreadState {
                thread_id: thread_id.to_owned(),
                status: "unknown".into(),
                ..ThreadState::default()
            });
    }

    /// Atomically promotes pending subscription relevance to a retained subscription.
    /// The entry is upserted so a prior prune can never turn this into a no-op.
    pub async fn finish_subscription(&self, thread_id: &str) {
        let mut state = self.state.write().await;
        state.subscribing.remove(thread_id);
        state
            .snapshot
            .threads
            .entry(thread_id.to_owned())
            .or_insert_with(|| ThreadState {
                thread_id: thread_id.to_owned(),
                status: "unknown".into(),
                ..ThreadState::default()
            })
            .subscribed = true;
    }

    /// Releases pending relevance after a failed subscription attempt.
    pub async fn cancel_subscription(&self, thread_id: &str) {
        let mut state = self.state.write().await;
        state.subscribing.remove(thread_id);
        if state
            .snapshot
            .threads
            .get(thread_id)
            .is_some_and(Self::is_prunable)
        {
            state.snapshot.threads.remove(thread_id);
        }
    }

    /// Removes any legacy idle entries while respecting subscription setup atomically.
    /// Normal lifecycle ingest also applies the same pruning rule inline.
    pub async fn evict_idle(&self) -> usize {
        let mut state = self.state.write().await;
        let subscribing = state.subscribing.clone();
        let before = state.snapshot.threads.len();
        state
            .snapshot
            .threads
            .retain(|id, thread| subscribing.contains(id) || !Self::is_prunable(thread));
        before - state.snapshot.threads.len()
    }

    fn is_prunable(thread: &ThreadState) -> bool {
        !thread.subscribed && thread.active_turn_id.is_none()
    }

    /// Replaces current state from a `thread/read` reconciliation response without inventing
    /// a wire receipt sequence or pretending the recovered state was a live notification.
    pub async fn reconcile_thread(&self, raw_thread: Value) {
        let Some(thread_id) = raw_thread
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        let status = raw_thread
            .get("status")
            .and_then(|status| {
                status
                    .get("type")
                    .and_then(Value::as_str)
                    .or_else(|| status.as_str())
            })
            .unwrap_or("unknown")
            .to_owned();
        let active_turn_id = raw_thread
            .get("turns")
            .and_then(Value::as_array)
            .and_then(|turns| {
                turns
                    .iter()
                    .rev()
                    .find(|turn| {
                        !matches!(
                            turn.get("status").and_then(Value::as_str),
                            Some("completed" | "failed" | "interrupted")
                        )
                    })
                    .and_then(|turn| turn.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let mut state = self.state.write().await;
        let sequence = state.snapshot.at_sequence;
        let previous_subscribed = state
            .snapshot
            .threads
            .get(&thread_id)
            .is_some_and(|thread| thread.subscribed);
        state.snapshot.threads.insert(
            thread_id.clone(),
            ThreadState {
                thread_id,
                status: if active_turn_id.is_some() {
                    "active".into()
                } else {
                    status
                },
                active_turn_id,
                subscribed: previous_subscribed,
                ephemeral: raw_thread
                    .get("ephemeral")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                last_sequence: sequence,
                raw_thread: Some(raw_thread),
            },
        );
    }
}

impl Default for Reducer {
    fn default() -> Self {
        Self::new(4096)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{IncomingFrame, RpcId};
    use std::time::Duration;

    fn event(sequence: u64, method: &str, thread_id: &str) -> Arc<SequencedEvent> {
        let raw =
            serde_json::json!({"jsonrpc":"2.0","method":method,"params":{"threadId":thread_id}});
        Arc::new(SequencedEvent::from_frame(
            sequence,
            Duration::from_millis(sequence),
            IncomingFrame::parse(raw).unwrap(),
        ))
    }

    #[tokio::test]
    async fn retained_replay_and_gap_are_honest() {
        let reducer = Reducer::new(2);
        for seq in 1..=3 {
            reducer
                .apply(event(seq, "thread/status/changed", "t"))
                .await;
        }
        assert!(matches!(reducer.events_since(1).await, ReplayResult::Events(v) if v.len() == 2));
        assert!(matches!(
            reducer.events_since(0).await,
            ReplayResult::GapTooOld {
                oldest_available: Some(2),
                ..
            }
        ));
    }

    fn event_turn(
        sequence: u64,
        method: &str,
        thread_id: &str,
        turn_id: &str,
    ) -> Arc<SequencedEvent> {
        let raw = serde_json::json!({"jsonrpc":"2.0","method":method,
            "params":{"threadId":thread_id,"turn":{"id":turn_id}}});
        Arc::new(SequencedEvent::from_frame(
            1,
            Duration::from_millis(sequence),
            IncomingFrame::parse(raw).unwrap(),
        ))
    }

    #[tokio::test]
    async fn idle_global_noise_is_pruned_inline() {
        let reducer = Reducer::default();
        for seq in 1..=50 {
            reducer
                .apply(event(seq, "thread/status/changed", &format!("idle{seq}")))
                .await;
        }

        assert!(
            reducer.snapshot().await.threads.is_empty(),
            "idle non-subscribed lifecycle noise must not accumulate between eviction ticks"
        );
    }

    #[tokio::test]
    async fn subscription_setup_and_active_turns_survive_inline_pruning() {
        let reducer = Reducer::default();

        reducer.begin_subscription("subscribing").await;
        reducer
            .apply(event(100, "thread/status/changed", "subscribing"))
            .await;
        assert!(
            reducer.snapshot().await.threads.contains_key("subscribing"),
            "a pending subscription must be protected under the reducer lock"
        );

        reducer.finish_subscription("subscribing").await;
        reducer
            .apply(event(101, "thread/status/changed", "subscribing"))
            .await;
        reducer
            .apply(event_turn(200, "turn/started", "act", "t9"))
            .await;

        let snap = reducer.snapshot().await;
        assert!(
            snap.threads
                .get("subscribing")
                .is_some_and(|thread| thread.subscribed),
            "subscribed thread retained"
        );
        assert!(snap.threads.contains_key("act"), "active-turn retained");
    }

    #[tokio::test]
    async fn authoritative_waiter_fires_without_broadcast() {
        let reducer = Reducer::default();
        let rx = reducer.register_waiter(|event| event.method() == Some("turn/completed"));
        reducer.apply(event(1, "turn/completed", "t")).await;
        assert_eq!(rx.await.unwrap().sequence, 1);
        let _ = RpcId::Number(1);
    }
}
