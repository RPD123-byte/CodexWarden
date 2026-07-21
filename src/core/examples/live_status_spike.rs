//! Bounded live diagnostic for OpenSpec task 5.1. It never starts or modifies a turn.

use std::time::Duration;
use transport::{RpcClient, TransportConfig, TransportHandle};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = TransportConfig {
        request_timeout: Duration::from_secs(3),
        connect_timeout: Duration::from_secs(2),
        ..TransportConfig::default()
    };
    let observer = TransportHandle::spawn(config.clone());
    let actor = TransportHandle::spawn(config);
    let list = observer
        .request(
            "thread/list",
            serde_json::json!({"limit":10,"sortKey":"updated_at","sortDirection":"desc"}),
        )
        .await?;
    let mut events = observer.subscribe();
    let threads = list["data"].as_array().cloned().unwrap_or_default();
    let mut tested = 0usize;
    let mut observed = None;
    for thread in threads {
        let Some(thread_id) = thread["id"].as_str() else {
            continue;
        };
        if actor
            .request("thread/resume", serde_json::json!({"threadId":thread_id}))
            .await
            .is_err()
        {
            continue;
        }
        tested += 1;
        let matching = tokio::time::timeout(Duration::from_millis(750), async {
            loop {
                let event = events.recv().await.ok()?;
                if event.method() == Some("thread/status/changed")
                    && event.thread_id.as_deref() == Some(thread_id)
                {
                    return Some(event.sequence);
                }
            }
        })
        .await
        .ok()
        .flatten();
        if let Some(sequence) = matching {
            observed = Some((thread_id.to_owned(), sequence));
            break;
        }
        if tested >= 3 {
            break;
        }
    }
    match observed {
        Some((thread_id, sequence)) => println!(
            "GLOBAL_STATUS_CHANGED=yes thread={thread_id} observer_sequence={sequence} tested={tested}"
        ),
        None => println!("GLOBAL_STATUS_CHANGED=no tested={tested}"),
    }
    actor.shutdown().await;
    observer.shutdown().await;
    Ok(())
}
