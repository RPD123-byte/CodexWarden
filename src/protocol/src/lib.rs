//! JSON-RPC wire types for the Codex app-server control connection.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[allow(clippy::all, dead_code, missing_docs)]
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}

pub type Sequence = u64;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcId {
    Number(u64),
    String(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Plane {
    Lifecycle,
    Delta,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum KnownMessage {
    ThreadStarted,
    ThreadStatusChanged,
    TurnStarted,
    TurnCompleted,
    TurnDiffUpdated,
    ItemLifecycle,
    ReviewerRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IncomingFrame {
    Response {
        id: RpcId,
        result: Option<Value>,
        error: Option<Value>,
        raw: Value,
    },
    Notification {
        method: String,
        params: Value,
        known: Option<KnownMessage>,
        raw: Value,
    },
    ServerRequest {
        id: RpcId,
        method: String,
        params: Value,
        known: Option<KnownMessage>,
        raw: Value,
    },
}

impl IncomingFrame {
    pub fn parse(raw: Value) -> Result<Self, &'static str> {
        let id = raw.get("id").cloned();
        let method = raw.get("method").and_then(Value::as_str).map(str::to_owned);
        match (id, method) {
            (Some(id), Some(method)) => Ok(Self::ServerRequest {
                id: serde_json::from_value(id).map_err(|_| "invalid request id")?,
                params: raw.get("params").cloned().unwrap_or(Value::Null),
                known: classify(&method),
                method,
                raw,
            }),
            (None, Some(method)) => Ok(Self::Notification {
                params: raw.get("params").cloned().unwrap_or(Value::Null),
                known: classify(&method),
                method,
                raw,
            }),
            (Some(id), None) => Ok(Self::Response {
                id: serde_json::from_value(id).map_err(|_| "invalid response id")?,
                result: raw.get("result").cloned(),
                error: raw.get("error").cloned(),
                raw,
            }),
            (None, None) => Err("frame has neither method nor id"),
        }
    }

    pub fn raw(&self) -> &Value {
        match self {
            Self::Response { raw, .. }
            | Self::Notification { raw, .. }
            | Self::ServerRequest { raw, .. } => raw,
        }
    }

    pub fn method(&self) -> Option<&str> {
        match self {
            Self::Notification { method, .. } | Self::ServerRequest { method, .. } => Some(method),
            Self::Response { .. } => None,
        }
    }

    pub fn params(&self) -> Option<&Value> {
        match self {
            Self::Notification { params, .. } | Self::ServerRequest { params, .. } => Some(params),
            Self::Response { .. } => None,
        }
    }

    pub fn is_server_request(&self) -> bool {
        matches!(self, Self::ServerRequest { .. })
    }
}

fn classify(method: &str) -> Option<KnownMessage> {
    match method {
        "thread/started" => Some(KnownMessage::ThreadStarted),
        "thread/status/changed" => Some(KnownMessage::ThreadStatusChanged),
        "turn/started" => Some(KnownMessage::TurnStarted),
        "turn/completed" => Some(KnownMessage::TurnCompleted),
        "turn/diff/updated" => Some(KnownMessage::TurnDiffUpdated),
        m if m.starts_with("item/") && !m.contains("delta") => Some(KnownMessage::ItemLifecycle),
        m if m.contains("approval") || m.contains("requestUserInput") => {
            Some(KnownMessage::ReviewerRequest)
        }
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequencedEvent {
    pub sequence: Sequence,
    pub unix_receipt_ms: u64,
    pub monotonic_ms: u64,
    pub emitted_at_ms: Option<u64>,
    pub plane: Plane,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub frame: IncomingFrame,
    /// True when this event was reconstructed by the daemon (e.g. a post-subscribe
    /// `thread/read` anchor for a turn whose wire `turn/started` was never received),
    /// NOT delivered as a genuine wire notification. Its timestamps are our receipt time.
    #[serde(default)]
    pub reconstructed: bool,
}

impl SequencedEvent {
    pub fn from_frame(sequence: Sequence, monotonic: Duration, frame: IncomingFrame) -> Self {
        let params = frame.params();
        let method = frame.method().unwrap_or_default();
        Self {
            sequence,
            unix_receipt_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            monotonic_ms: monotonic.as_millis() as u64,
            emitted_at_ms: params.and_then(extract_emitted_at_ms),
            plane: plane_for_method(method),
            thread_id: params.and_then(extract_thread_id).map(str::to_owned),
            turn_id: params.and_then(extract_turn_id).map(str::to_owned),
            frame,
            reconstructed: false,
        }
    }

    /// Builds a reconstructed lifecycle anchor for a turn observed active at subscribe
    /// time (its wire `turn/started` was missed). Carries our own receipt time and is
    /// flagged `reconstructed` so consumers never mistake it for a genuine notification.
    pub fn reconstructed_active_turn(
        sequence: Sequence,
        monotonic: Duration,
        thread_id: &str,
        turn_id: &str,
    ) -> Self {
        let raw = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "turn/started",
            "params": {"threadId": thread_id, "turn": {"id": turn_id}},
        });
        let frame = IncomingFrame::parse(raw).expect("static reconstructed frame is valid");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            sequence,
            unix_receipt_ms: now,
            monotonic_ms: monotonic.as_millis() as u64,
            emitted_at_ms: None,
            plane: Plane::Lifecycle,
            thread_id: Some(thread_id.to_owned()),
            turn_id: Some(turn_id.to_owned()),
            frame,
            reconstructed: true,
        }
    }

    pub fn method(&self) -> Option<&str> {
        self.frame.method()
    }

    pub fn encoded_len(&self) -> usize {
        serde_json::to_vec(self).map_or(0, |v| v.len())
    }
}

pub fn plane_for_method(method: &str) -> Plane {
    if method.contains("delta") || method.ends_with("/updated") || method.contains("output") {
        Plane::Delta
    } else {
        Plane::Lifecycle
    }
}

pub fn extract_thread_id(value: &Value) -> Option<&str> {
    value
        .get("threadId")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("thread")
                .and_then(|v| v.get("id"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            value
                .get("turn")
                .and_then(|v| v.get("threadId"))
                .and_then(Value::as_str)
        })
}

pub fn extract_turn_id(value: &Value) -> Option<&str> {
    value.get("turnId").and_then(Value::as_str).or_else(|| {
        value
            .get("turn")
            .and_then(|v| v.get("id"))
            .and_then(Value::as_str)
    })
}

pub fn extract_emitted_at_ms(value: &Value) -> Option<u64> {
    value
        .get("emittedAtMs")
        .or_else(|| value.get("emitted_at_ms"))
        .and_then(Value::as_u64)
}

pub fn request(id: RpcId, method: &str, params: Value) -> Value {
    serde_json::json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params})
}

pub fn notification(method: &str, params: Value) -> Value {
    serde_json::json!({"jsonrpc":"2.0", "method":method, "params":params})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::File,
        io::{BufRead, BufReader},
        path::PathBuf,
    };

    #[test]
    fn unknown_server_request_is_raw_and_answerless() {
        let raw =
            serde_json::json!({"jsonrpc":"2.0","id":7,"method":"future/review","params":{"x":1}});
        let frame = IncomingFrame::parse(raw.clone()).unwrap();
        assert!(frame.is_server_request());
        assert_eq!(frame.raw(), &raw);
    }

    #[test]
    fn sanitized_recorded_inner_messages_round_trip_without_loss() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/recorded-inner-messages.ndjson");
        let reader = BufReader::new(File::open(path).expect("protocol fidelity fixture"));
        let mut seen = 0usize;
        for line in reader.lines() {
            let line = line.expect("read fixture line");
            let message: Value = serde_json::from_str(&line).expect("parse inner message");
            let frame = IncomingFrame::parse(message.clone()).expect("parse incoming envelope");
            assert_eq!(frame.raw(), &message);
            let encoded = serde_json::to_vec(frame.raw()).expect("encode inner message");
            let decoded: Value = serde_json::from_slice(&encoded).expect("decode inner message");
            assert_eq!(decoded, message);
            seen += 1;
        }
        assert!(seen >= 18, "fidelity fixture unexpectedly small: {seen}");
    }
}
