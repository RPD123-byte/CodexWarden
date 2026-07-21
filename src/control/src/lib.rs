//! Explicit-target control actions with conservative outcome classification.

use protocol::SequencedEvent;
use reducer::Reducer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;
use transport::{RequestError, RpcClient};

#[derive(Clone, Debug)]
pub struct ControlConfig {
    pub correlation_window: Duration,
    pub not_written_retries: usize,
    pub retry_delay: Duration,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            correlation_window: Duration::from_secs(3),
            not_written_retries: 2,
            retry_delay: Duration::from_millis(100),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionTarget {
    Turn { thread_id: String, turn_id: String },
    Thread { thread_id: String },
}

impl ActionTarget {
    fn thread_id(&self) -> &str {
        match self {
            Self::Turn { thread_id, .. } | Self::Thread { thread_id } => thread_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub action_correlation: u64,
    pub rpc_accepted: bool,
    pub observed_sequence: Option<u64>,
    pub observed_method: Option<String>,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ActionOutcome {
    Confirmed {
        target: ActionTarget,
        evidence: Evidence,
    },
    Rejected {
        target: ActionTarget,
        reason: String,
        rpc_error: Option<Value>,
    },
    OutcomeUnknown {
        target: ActionTarget,
        evidence: Evidence,
    },
}

#[derive(Clone)]
pub struct Controller<C: RpcClient> {
    client: C,
    reducer: Reducer,
    config: ControlConfig,
    thread_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    next_correlation: Arc<AtomicU64>,
}

impl<C: RpcClient> Controller<C> {
    pub fn new(client: C, reducer: Reducer, config: ControlConfig) -> Self {
        Self {
            client,
            reducer,
            config,
            thread_locks: Arc::new(Mutex::new(HashMap::new())),
            next_correlation: Arc::new(AtomicU64::new(1)),
        }
    }

    async fn lock_for(&self, thread_id: &str) -> Arc<Mutex<()>> {
        self.thread_locks
            .lock()
            .await
            .entry(thread_id.into())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn interrupt(
        &self,
        thread_id: impl Into<String>,
        turn_id: impl Into<String>,
    ) -> ActionOutcome {
        let target = ActionTarget::Turn {
            thread_id: thread_id.into(),
            turn_id: turn_id.into(),
        };
        let lock = self.lock_for(target.thread_id()).await;
        let _guard = lock.lock().await;
        let correlation = self.next_correlation.fetch_add(1, Ordering::Relaxed);
        let (expected_thread, expected_turn) = match &target {
            ActionTarget::Turn { thread_id, turn_id } => (thread_id.clone(), turn_id.clone()),
            _ => unreachable!(),
        };
        // Registered before send, so completion-before-response is still observed.
        let completion = self.reducer.register_waiter(move |event| {
            event.method() == Some("turn/completed")
                && event.thread_id.as_deref() == Some(&expected_thread)
                && event.turn_id.as_deref() == Some(&expected_turn)
        });
        let params = match &target {
            ActionTarget::Turn { thread_id, turn_id } => {
                serde_json::json!({"threadId":thread_id,"turnId":turn_id})
            }
            _ => unreachable!(),
        };
        match self.send_not_written_safe("turn/interrupt", params).await {
            Ok(_) => match tokio::time::timeout(self.config.correlation_window, completion).await {
                Ok(Ok(event)) => ActionOutcome::Confirmed {
                    target,
                    evidence: observed(
                        correlation,
                        true,
                        &event,
                        "matching completion observed after interrupt; correlation is not proof of causation",
                    ),
                },
                _ => self.reconcile_interrupt(target, correlation, true).await,
            },
            Err(RequestError::Rejected { error, .. }) => ActionOutcome::Rejected {
                target,
                reason: "daemon rejected interrupt".into(),
                rpc_error: Some(error),
            },
            Err(
                RequestError::WrittenOutcomeUnknown { detail, .. }
                | RequestError::TimedOut { method: detail },
            ) => {
                let result = self.reconcile_interrupt(target, correlation, false).await;
                if matches!(result, ActionOutcome::OutcomeUnknown { .. }) {
                    tracing::error!(correlation, %detail, "interrupt terminal outcome unknown; no DLQ configured");
                }
                result
            }
            Err(error) => ActionOutcome::Rejected {
                target,
                reason: error.to_string(),
                rpc_error: None,
            },
        }
    }

    async fn reconcile_interrupt(
        &self,
        target: ActionTarget,
        correlation: u64,
        rpc_accepted: bool,
    ) -> ActionOutcome {
        let (thread_id, turn_id) = match &target {
            ActionTarget::Turn { thread_id, turn_id } => (thread_id, turn_id),
            _ => unreachable!(),
        };
        let read = self
            .client
            .request(
                "thread/read",
                serde_json::json!({"threadId":thread_id,"includeTurns":true}),
            )
            .await;
        let terminal = read
            .ok()
            .and_then(|value| value.get("thread").cloned())
            .and_then(|thread| thread.get("turns").cloned())
            .and_then(|turns| turns.as_array().cloned())
            .and_then(|turns| turns.into_iter().find(|turn| turn["id"] == *turn_id))
            .and_then(|turn| {
                turn.get("status")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .is_some_and(|status| {
                matches!(status.as_str(), "completed" | "failed" | "interrupted")
            });
        let evidence = Evidence {
            action_correlation: correlation,
            rpc_accepted,
            observed_sequence: None,
            observed_method: None,
            note: if terminal {
                "target turn is terminal on reconciliation; correlation is not causation"
            } else {
                "no unique terminal evidence for target turn"
            }
            .into(),
        };
        if terminal {
            ActionOutcome::Confirmed { target, evidence }
        } else {
            ActionOutcome::OutcomeUnknown { target, evidence }
        }
    }

    pub async fn steer(
        &self,
        thread_id: impl Into<String>,
        expected_turn_id: impl Into<String>,
        input: Vec<Value>,
    ) -> ActionOutcome {
        let target = ActionTarget::Turn {
            thread_id: thread_id.into(),
            turn_id: expected_turn_id.into(),
        };
        self.rpc_response_action("turn/steer", target, input, true)
            .await
    }

    pub async fn start(&self, thread_id: impl Into<String>, input: Vec<Value>) -> ActionOutcome {
        let target = ActionTarget::Thread {
            thread_id: thread_id.into(),
        };
        let lock = self.lock_for(target.thread_id()).await;
        let _guard = lock.lock().await;
        let correlation = self.next_correlation.fetch_add(1, Ordering::Relaxed);
        let expected_thread = target.thread_id().to_owned();
        let started = self.reducer.register_waiter(move |event| {
            event.method() == Some("turn/started")
                && event.thread_id.as_deref() == Some(&expected_thread)
        });
        let params = serde_json::json!({"threadId":target.thread_id(),"input":input});
        match self.send_not_written_safe("turn/start", params).await {
            Ok(response) => {
                let response_turn = response
                    .get("turn")
                    .and_then(|v| v.get("id"))
                    .or_else(|| response.get("turnId"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let observed_event = tokio::time::timeout(self.config.correlation_window, started)
                    .await
                    .ok()
                    .and_then(Result::ok);
                if let (Some(expected), Some(event)) =
                    (response_turn.as_deref(), observed_event.as_ref())
                    && event.turn_id.as_deref() == Some(expected)
                {
                    return ActionOutcome::Confirmed {
                        target,
                        evidence: observed(
                            correlation,
                            true,
                            event,
                            "turn/started id matches the id returned by turn/start",
                        ),
                    };
                }
                if response_turn.is_some() {
                    return ActionOutcome::Confirmed { target, evidence: Evidence { action_correlation: correlation,
                        rpc_accepted: true, observed_sequence: None, observed_method: None,
                        note: "daemon returned a unique created turn id; concurrent GUI starts are not attributed".into() } };
                }
                ActionOutcome::OutcomeUnknown {
                    target,
                    evidence: unknown(correlation, true, "response lacked a unique turn id"),
                }
            }
            Err(RequestError::Rejected { error, .. }) => ActionOutcome::Rejected {
                target,
                reason: "daemon rejected turn/start".into(),
                rpc_error: Some(error),
            },
            Err(
                RequestError::WrittenOutcomeUnknown { detail, .. }
                | RequestError::TimedOut { method: detail },
            ) => {
                tracing::error!(correlation, %detail, "turn/start outcome unknown; ambiguous write will not be resent");
                ActionOutcome::OutcomeUnknown {
                    target,
                    evidence: unknown(
                        correlation,
                        false,
                        "write may have succeeded; reconciliation cannot distinguish a concurrent GUI start",
                    ),
                }
            }
            Err(error) => ActionOutcome::Rejected {
                target,
                reason: error.to_string(),
                rpc_error: None,
            },
        }
    }

    async fn rpc_response_action(
        &self,
        method: &str,
        target: ActionTarget,
        input: Vec<Value>,
        expected_turn: bool,
    ) -> ActionOutcome {
        let lock = self.lock_for(target.thread_id()).await;
        let _guard = lock.lock().await;
        let correlation = self.next_correlation.fetch_add(1, Ordering::Relaxed);
        let params = match &target {
            ActionTarget::Turn { thread_id, turn_id } if expected_turn => serde_json::json!({
                "threadId":thread_id,"expectedTurnId":turn_id,"input":input
            }),
            _ => unreachable!(),
        };
        match self.send_not_written_safe(method, params).await {
            Ok(_) => ActionOutcome::Confirmed {
                target,
                evidence: Evidence {
                    action_correlation: correlation,
                    rpc_accepted: true,
                    observed_sequence: None,
                    observed_method: None,
                    note: "daemon accepted the explicitly targeted request".into(),
                },
            },
            Err(RequestError::Rejected { error, .. }) => ActionOutcome::Rejected {
                target,
                reason: format!("daemon rejected {method}"),
                rpc_error: Some(error),
            },
            Err(
                RequestError::WrittenOutcomeUnknown { detail, .. }
                | RequestError::TimedOut { method: detail },
            ) => {
                tracing::error!(correlation, %detail, action=method, "action outcome unknown; ambiguous write will not be resent");
                ActionOutcome::OutcomeUnknown {
                    target,
                    evidence: unknown(
                        correlation,
                        false,
                        "ambiguous steer cannot be uniquely confirmed and was not resent",
                    ),
                }
            }
            Err(error) => ActionOutcome::Rejected {
                target,
                reason: error.to_string(),
                rpc_error: None,
            },
        }
    }

    async fn send_not_written_safe(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, RequestError> {
        let mut retries = 0;
        loop {
            match self.client.request_action(method, params.clone()).await {
                Err(RequestError::NotWritten(_)) if retries < self.config.not_written_retries => {
                    retries += 1;
                    tokio::time::sleep(self.config.retry_delay).await;
                }
                result => return result,
            }
        }
    }
}

fn observed(correlation: u64, rpc_accepted: bool, event: &SequencedEvent, note: &str) -> Evidence {
    Evidence {
        action_correlation: correlation,
        rpc_accepted,
        observed_sequence: Some(event.sequence),
        observed_method: event.method().map(str::to_owned),
        note: note.into(),
    }
}

fn unknown(correlation: u64, rpc_accepted: bool, note: &str) -> Evidence {
    Evidence {
        action_correlation: correlation,
        rpc_accepted,
        observed_sequence: None,
        observed_method: None,
        note: note.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use protocol::{IncomingFrame, RpcId};
    use std::time::Instant;

    #[derive(Clone)]
    struct CompletingClient {
        reducer: Reducer,
    }

    #[async_trait]
    impl RpcClient for CompletingClient {
        async fn request(&self, method: &str, _params: Value) -> Result<Value, RequestError> {
            if method == "thread/read" {
                return Ok(serde_json::json!({"thread":{"turns":[]}}));
            }
            Ok(Value::Null)
        }
        async fn request_action(&self, method: &str, params: Value) -> Result<Value, RequestError> {
            if method == "turn/interrupt" {
                let raw = serde_json::json!({"method":"turn/completed","params":{"threadId":params["threadId"],"turn":{"id":params["turnId"]}}});
                let event = Arc::new(SequencedEvent::from_frame(
                    4,
                    Instant::now().elapsed(),
                    IncomingFrame::parse(raw).unwrap(),
                ));
                self.reducer.apply(event).await;
            }
            Ok(Value::Null)
        }
        async fn notify(&self, _method: &str, _params: Value) -> Result<(), RequestError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn completion_before_response_confirms_without_broadcast() {
        let reducer = Reducer::default();
        let controller = Controller::new(
            CompletingClient {
                reducer: reducer.clone(),
            },
            reducer,
            ControlConfig {
                correlation_window: Duration::from_millis(50),
                ..ControlConfig::default()
            },
        );
        let outcome = controller.interrupt("thread", "turn").await;
        assert!(matches!(
            outcome,
            ActionOutcome::Confirmed {
                evidence: Evidence {
                    observed_sequence: Some(4),
                    ..
                },
                ..
            }
        ));
        let _ = RpcId::Number(2);
    }

    #[derive(Clone)]
    struct AmbiguousClient;
    #[async_trait]
    impl RpcClient for AmbiguousClient {
        async fn request(&self, _method: &str, _params: Value) -> Result<Value, RequestError> {
            Ok(Value::Null)
        }
        async fn request_action(
            &self,
            method: &str,
            _params: Value,
        ) -> Result<Value, RequestError> {
            Err(RequestError::WrittenOutcomeUnknown {
                id: RpcId::Number(9),
                method: method.into(),
                detail: "closed".into(),
            })
        }
        async fn notify(&self, _method: &str, _params: Value) -> Result<(), RequestError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn ambiguous_start_and_steer_are_never_falsely_confirmed() {
        let controller = Controller::new(
            AmbiguousClient,
            Reducer::default(),
            ControlConfig::default(),
        );
        assert!(matches!(
            controller.start("t", vec![]).await,
            ActionOutcome::OutcomeUnknown { .. }
        ));
        assert!(matches!(
            controller.steer("t", "turn", vec![]).await,
            ActionOutcome::OutcomeUnknown { .. }
        ));
    }
}
