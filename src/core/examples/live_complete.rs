//! Diagnostic: does a short turn complete, and does the reducer clear active_turn_id?
use codex_control::{CodexControl, Config};
use serde_json::json;
use std::time::{Duration, Instant};
use transport::{RpcClient, TransportConfig, TransportHandle};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    codex_control::init_tracing("warn");
    let cwd = std::env::var("E2E_CWD").unwrap_or(std::env::current_dir()?.display().to_string());
    CodexControl::run(Config { manage_gui: false, ..Config::default() }, move |handle| async move {
        let boot = TransportHandle::spawn(TransportConfig::default());
        let created = boot.request("thread/start", json!({"cwd": cwd})).await?;
        let thread = created["thread"]["id"].as_str().unwrap().to_string();
        for _ in 0..40 { if handle.subscription_states().await.contains_key(&thread) { break; } tokio::time::sleep(Duration::from_millis(200)).await; }
        println!("thread={} subscribed", &thread[thread.len()-8..]);

        // Replicate TEST 2's exact condition: start a long turn, interrupt it, THEN start pong on the same thread.
        let _ = handle.start(&thread, vec![json!({"type":"text","text":"Write a 3000-word essay on TCP. Write slowly."})]).await;
        for _ in 0..40 { if let Some(t) = handle.snapshot().await.threads.get(&thread).and_then(|x| x.active_turn_id.clone()) {
            tokio::time::sleep(Duration::from_secs(2)).await;
            println!("interrupt long turn: {:?}", std::mem::discriminant(&handle.interrupt(&thread, &t).await)); break; }
            tokio::time::sleep(Duration::from_millis(200)).await; }

        let _ = handle.start(&thread, vec![json!({"type":"text","text":"Reply with exactly the word: pong"})]).await;
        let t0 = Instant::now();
        loop {
            let active = handle.snapshot().await.threads.get(&thread).and_then(|x| x.active_turn_id.clone());
            if active.is_none() { println!("active_turn cleared at +{}ms => COMPLETED ✅", t0.elapsed().as_millis()); break; }
            if t0.elapsed() > Duration::from_secs(45) { println!("still active after 45s: {:?} => STUCK", active.map(|s| s[s.len()-6..].to_string())); break; }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // Dump the lifecycle events the store received for this thread (reveals turn/started, turn/completed).
        let q = handle.query_sequence(Some(&thread), 0, None).await;
        println!("--- {} stored events for this thread ---", q.events.len());
        for e in &q.events {
            if let Some(m) = e.method()
                && (m.starts_with("turn/") || m.starts_with("thread/"))
            {
                println!("  {}  turn={:?}", m, e.turn_id.as_deref().map(|x| &x[x.len().saturating_sub(6)..]));
            }
        }
        boot.shutdown().await;
        Ok::<(), Box<dyn std::error::Error>>(())
    }).await??;
    Ok(())
}
