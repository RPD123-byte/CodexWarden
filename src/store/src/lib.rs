//! Bounded in-memory event store fed by authoritative ingress.

use protocol::{Plane, Sequence, SequencedEvent};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;

const MIB: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub retention: Duration,
    pub per_thread_bytes: usize,
    pub global_bytes: usize,
    pub emitted_at_tolerance: Duration,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            retention: Duration::from_secs(10 * 60),
            per_thread_bytes: 8 * MIB,
            global_bytes: 64 * MIB,
            emitted_at_tolerance: Duration::from_secs(2),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionGap {
    pub before_sequence: Option<Sequence>,
    pub summarized_delta_events: u64,
    pub evicted_lifecycle_events: u64,
}

#[derive(Clone, Debug)]
pub struct QueryResult {
    pub events: Vec<Arc<SequencedEvent>>,
    pub gap: Option<RetentionGap>,
}

struct Inner {
    events: VecDeque<Arc<SequencedEvent>>,
    bytes: usize,
    thread_bytes: HashMap<String, usize>,
    gaps: HashMap<Option<String>, RetentionGap>,
}

#[derive(Clone)]
pub struct EventStore {
    config: StoreConfig,
    inner: Arc<RwLock<Inner>>,
    started_at: Instant,
}

impl EventStore {
    pub fn new(config: StoreConfig) -> Self {
        Self {
            config,
            inner: Arc::new(RwLock::new(Inner {
                events: VecDeque::new(),
                bytes: 0,
                thread_bytes: HashMap::new(),
                gaps: HashMap::new(),
            })),
            started_at: Instant::now(),
        }
    }

    pub async fn record(&self, event: Arc<SequencedEvent>) {
        let mut inner = self.inner.write().await;
        self.record_locked(&mut inner, event);
    }

    /// Records a subscribe-boundary anchor only when no genuine wire `turn/started`
    /// for the same turn is already retained. The check and insert share one store lock.
    pub async fn record_reconstructed_active_turn(
        &self,
        sequence: Sequence,
        thread_id: &str,
        turn_id: &str,
    ) -> bool {
        let mut inner = self.inner.write().await;
        if inner.events.iter().any(|event| {
            !event.reconstructed
                && event.method() == Some("turn/started")
                && event.thread_id.as_deref() == Some(thread_id)
                && event.turn_id.as_deref() == Some(turn_id)
        }) {
            return false;
        }
        let event = Arc::new(SequencedEvent::reconstructed_active_turn(
            sequence,
            self.started_at.elapsed(),
            thread_id,
            turn_id,
        ));
        self.record_locked(&mut inner, event);
        true
    }

    fn record_locked(&self, inner: &mut Inner, event: Arc<SequencedEvent>) {
        let size = event.encoded_len();
        inner.bytes += size;
        if let Some(thread) = &event.thread_id {
            *inner.thread_bytes.entry(thread.clone()).or_default() += size;
        }
        inner.events.push_back(event);
        self.prune_locked(inner);
    }

    fn prune_locked(&self, inner: &mut Inner) {
        let newest_ms = inner.events.back().map_or(0, |e| e.monotonic_ms);
        let cutoff = newest_ms.saturating_sub(self.config.retention.as_millis() as u64);
        while inner
            .events
            .front()
            .is_some_and(|e| e.monotonic_ms < cutoff)
        {
            Self::remove_at(inner, 0);
        }
        loop {
            let over_global = inner.bytes > self.config.global_bytes;
            let over_thread = inner
                .thread_bytes
                .iter()
                .find(|(_, bytes)| **bytes > self.config.per_thread_bytes)
                .map(|(thread, _)| thread.clone());
            if !over_global && over_thread.is_none() {
                break;
            }
            let position = inner
                .events
                .iter()
                .position(|event| {
                    event.plane == Plane::Delta
                        && over_thread
                            .as_ref()
                            .is_none_or(|t| event.thread_id.as_ref() == Some(t))
                })
                .unwrap_or(0);
            Self::remove_at(inner, position);
        }
    }

    fn remove_at(inner: &mut Inner, position: usize) {
        let Some(event) = inner.events.remove(position) else {
            return;
        };
        let size = event.encoded_len();
        inner.bytes = inner.bytes.saturating_sub(size);
        if let Some(thread) = &event.thread_id {
            let bytes = inner.thread_bytes.entry(thread.clone()).or_default();
            *bytes = bytes.saturating_sub(size);
        }
        let gap = inner.gaps.entry(event.thread_id.clone()).or_default();
        gap.before_sequence = Some(
            gap.before_sequence
                .map_or(event.sequence, |s| s.max(event.sequence)),
        );
        if event.plane == Plane::Delta {
            gap.summarized_delta_events += 1;
        } else {
            gap.evicted_lifecycle_events += 1;
        }
    }

    pub async fn query_sequence(
        &self,
        thread_id: Option<&str>,
        after: Sequence,
        through: Option<Sequence>,
    ) -> QueryResult {
        let inner = self.inner.read().await;
        let events = inner
            .events
            .iter()
            .filter(|event| {
                event.sequence > after
                    && through.is_none_or(|end| event.sequence <= end)
                    && thread_id.is_none_or(|thread| event.thread_id.as_deref() == Some(thread))
            })
            .cloned()
            .collect();
        QueryResult {
            events,
            gap: inner.gaps.get(&thread_id.map(str::to_owned)).cloned(),
        }
    }

    pub async fn query_time(
        &self,
        thread_id: Option<&str>,
        start_unix_ms: u64,
        end_unix_ms: u64,
    ) -> QueryResult {
        let tolerance = self.config.emitted_at_tolerance.as_millis() as u64;
        let inner = self.inner.read().await;
        let events = inner
            .events
            .iter()
            .filter(|event| {
                let timestamp = event.emitted_at_ms.unwrap_or(event.unix_receipt_ms);
                timestamp.saturating_add(tolerance) >= start_unix_ms
                    && timestamp <= end_unix_ms.saturating_add(tolerance)
                    && thread_id.is_none_or(|thread| event.thread_id.as_deref() == Some(thread))
            })
            .cloned()
            .collect();
        QueryResult {
            events,
            gap: inner.gaps.get(&thread_id.map(str::to_owned)).cloned(),
        }
    }

    pub async fn evict_idle_content(&self, idle_threads: &[String]) {
        let mut inner = self.inner.write().await;
        let mut position = inner.events.len();
        while position > 0 {
            position -= 1;
            let remove = inner.events[position].plane == Plane::Delta
                && inner.events[position]
                    .thread_id
                    .as_ref()
                    .is_some_and(|id| idle_threads.contains(id));
            if remove {
                Self::remove_at(&mut inner, position);
            }
        }
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.events.len()
    }
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.events.is_empty()
    }
    pub async fn bytes(&self) -> usize {
        self.inner.read().await.bytes
    }
}

impl Default for EventStore {
    fn default() -> Self {
        Self::new(StoreConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::IncomingFrame;

    fn event(seq: u64, method: &str, data: &str) -> Arc<SequencedEvent> {
        let raw = serde_json::json!({"method":method,"params":{"threadId":"t","data":data}});
        Arc::new(SequencedEvent::from_frame(
            seq,
            Duration::from_millis(seq),
            IncomingFrame::parse(raw).unwrap(),
        ))
    }

    #[tokio::test]
    async fn hard_cap_prefers_delta_eviction_and_reports_gap() {
        let sample = event(1, "item/output/delta", &"x".repeat(500));
        let size = sample.encoded_len();
        let store = EventStore::new(StoreConfig {
            global_bytes: size * 2,
            per_thread_bytes: size * 2,
            ..StoreConfig::default()
        });
        store.record(sample).await;
        store
            .record(event(2, "turn/completed", &"y".repeat(500)))
            .await;
        store
            .record(event(3, "turn/completed", &"z".repeat(500)))
            .await;
        assert!(store.bytes().await <= size * 2);
        let query = store.query_sequence(Some("t"), 0, None).await;
        assert!(query.gap.is_some());
        assert!(query.gap.unwrap().summarized_delta_events >= 1);
    }
}
