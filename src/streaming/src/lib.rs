//! Lifecycle and delta event fan-out.

use protocol::{Plane, Sequence, SequencedEvent};
use reducer::{Reducer, ReplayResult, Snapshot};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct EventHub {
    lifecycle: broadcast::Sender<Arc<SequencedEvent>>,
    delta: broadcast::Sender<Arc<SequencedEvent>>,
}

impl EventHub {
    pub fn new(lifecycle_capacity: usize, delta_capacity: usize) -> Self {
        let (lifecycle, _) = broadcast::channel(lifecycle_capacity);
        let (delta, _) = broadcast::channel(delta_capacity);
        Self { lifecycle, delta }
    }

    pub fn publish(&self, event: Arc<SequencedEvent>) {
        match event.plane {
            Plane::Lifecycle => {
                let _ = self.lifecycle.send(event);
            }
            Plane::Delta => {
                let _ = self.delta.send(event);
            }
        }
    }

    pub fn lifecycle(&self, reducer: Reducer, after: Sequence) -> LifecycleStream {
        LifecycleStream {
            receiver: self.lifecycle.subscribe(),
            reducer,
            cursor: after,
        }
    }

    pub fn deltas(&self) -> DeltaStream {
        DeltaStream {
            receiver: self.delta.subscribe(),
            dropped: 0,
        }
    }
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new(1024, 2048)
    }
}

#[derive(Debug)]
pub enum LifecycleItem {
    Event(Arc<SequencedEvent>),
    Replay(Vec<Arc<SequencedEvent>>),
    GapTooOld {
        snapshot: Snapshot,
        oldest_available: Option<Sequence>,
    },
    Closed,
}

pub struct LifecycleStream {
    receiver: broadcast::Receiver<Arc<SequencedEvent>>,
    reducer: Reducer,
    cursor: Sequence,
}

impl LifecycleStream {
    pub async fn recv(&mut self) -> LifecycleItem {
        match self.receiver.recv().await {
            Ok(event) => {
                self.cursor = event.sequence;
                LifecycleItem::Event(event)
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                match self.reducer.events_since(self.cursor).await {
                    ReplayResult::Events(events) => {
                        if let Some(last) = events.last() {
                            self.cursor = last.sequence;
                        }
                        LifecycleItem::Replay(events)
                    }
                    ReplayResult::GapTooOld {
                        oldest_available,
                        snapshot,
                        ..
                    } => {
                        self.cursor = snapshot.at_sequence;
                        LifecycleItem::GapTooOld {
                            snapshot,
                            oldest_available,
                        }
                    }
                }
            }
            Err(broadcast::error::RecvError::Closed) => LifecycleItem::Closed,
        }
    }
}

pub struct DeltaStream {
    receiver: broadcast::Receiver<Arc<SequencedEvent>>,
    dropped: u64,
}

impl DeltaStream {
    pub async fn recv(&mut self) -> Option<Arc<SequencedEvent>> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(count)) => self.dropped += count,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::IncomingFrame;
    use std::time::Duration;

    fn event(seq: u64, method: &str) -> Arc<SequencedEvent> {
        let raw = serde_json::json!({"method":method,"params":{"threadId":"t"}});
        Arc::new(SequencedEvent::from_frame(
            seq,
            Duration::from_millis(seq),
            IncomingFrame::parse(raw).unwrap(),
        ))
    }

    #[tokio::test]
    async fn lifecycle_lag_recovers_from_authoritative_log() {
        let reducer = Reducer::new(8);
        let hub = EventHub::new(1, 1);
        let mut stream = hub.lifecycle(reducer.clone(), 0);
        for seq in 1..=3 {
            let e = event(seq, "turn/completed");
            reducer.apply(e.clone()).await;
            hub.publish(e);
        }
        assert!(matches!(stream.recv().await, LifecycleItem::Replay(v) if v.len() == 3));
    }
}
