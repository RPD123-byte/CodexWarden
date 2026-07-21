use codex_control::{ActionOutcome, CodexControl, Config, ReplayResult};
use std::time::Duration;
use transport::{
    TransportConfig,
    mock::{Fault, MockAppServer},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    codex_control::init_tracing("codex_control=info");
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("app-server-control.sock");
    let server = MockAppServer::start(socket.clone()).await?;
    let config = Config {
        manage_gui: false,
        lifecycle_replay_capacity: 2,
        transport: TransportConfig {
            socket_path: socket,
            connect_timeout: Duration::from_millis(300),
            request_timeout: Duration::from_secs(1),
            retry_initial: Duration::from_millis(20),
            retry_max: Duration::from_millis(50),
            ..TransportConfig::default()
        },
        ..Config::default()
    };
    let run_server = server.clone();
    CodexControl::run(config, move |handle| async move {
        for index in 0..3 {
            run_server.emit_notification("thread/status/changed", serde_json::json!({
                "threadId":"example", "status":{"type":if index % 2 == 0 {"active"} else {"idle"}}
            })).await;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        match handle.events_since(0).await {
            ReplayResult::Events(events) => println!("replayed {} lifecycle events", events.len()),
            ReplayResult::GapTooOld { snapshot, .. } => {
                println!("history gap; recovered at {}", snapshot.at_sequence)
            }
        }
        run_server
            .set_fault(Fault::DisconnectOnMethod("turn/steer".into()))
            .await;
        if let ActionOutcome::OutcomeUnknown { evidence, .. } =
            handle.steer("example", "turn", vec![]).await
        {
            println!("safe ambiguous outcome: {}", evidence.note);
        }
        println!(
            "degraded transport health: {:?}",
            handle.health().borrow().phase
        );
    })
    .await?;
    server.shutdown().await;
    Ok(())
}
