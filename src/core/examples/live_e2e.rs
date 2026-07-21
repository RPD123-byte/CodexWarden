//! Live end-to-end exercise against the REAL managed Codex daemon.
//! Boots the full service, spins up a real session, and tests the action space:
//! start a turn, interrupt it, verify it stopped, start+complete, steer, and query.
//! Then watches for ~45s so a human/GUI-initiated turn can be caught and interrupted.
//!
//! Requires: managed daemon running + a GUI in daemon mode (or any client on the socket).

use codex_control::{ActionOutcome, CodexControl, Config};
use control::ActionTarget;
use serde_json::json;
use std::time::Duration;
use transport::{RpcClient, TransportConfig, TransportHandle};

async fn wait_subscribed(handle: &codex_control::Handle, thread: &str, secs: u64) -> bool {
    for _ in 0..(secs * 5) {
        if handle.subscription_states().await.contains_key(thread) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

async fn active_turn(handle: &codex_control::Handle, thread: &str, secs: u64) -> Option<String> {
    for _ in 0..(secs * 5) {
        if let Some(t) = handle.snapshot().await.threads.get(thread)
            && let Some(turn) = &t.active_turn_id
        {
            return Some(turn.clone());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    None
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    codex_control::init_tracing("codex_control=warn,ingest=warn,control=warn");
    let cwd = std::env::var("E2E_CWD").unwrap_or(std::env::current_dir()?.display().to_string());
    let long = || {
        vec![
            json!({"type":"text","text":"Write an extremely detailed 3000-word essay on the history of the TCP/IP protocol suite. No tools, write slowly, section by section."}),
        ]
    };

    CodexControl::run(Config { manage_gui: false, ..Config::default() }, move |handle| async move {
        // A separate bootstrap client creates the session (as the GUI/consumer would).
        let boot = TransportHandle::spawn(TransportConfig::default());

        // ---- TEST 1: service-initiated turn, then interrupt ----
        println!("\n=== TEST 1: start a long turn via the service, then interrupt it ===");
        let created = boot.request("thread/start", json!({"cwd": cwd})).await?;
        let thread = created["thread"]["id"].as_str().ok_or("no thread id")?.to_string();
        println!("[1] created session thread={}", &thread[thread.len()-12..]);
        println!("[1] service subscribed: {}", wait_subscribed(&handle, &thread, 8).await);

        let start = handle.start(&thread, long()).await;
        println!("[1] start outcome: {}", outcome_str(&start));
        let turn = active_turn(&handle, &thread, 8).await.ok_or("no active turn observed")?;
        println!("[1] active turn={}", &turn[turn.len()-12..]);
        tokio::time::sleep(Duration::from_secs(3)).await; // let it stream

        let interrupt = handle.interrupt(&thread, &turn).await;
        println!("[1] interrupt outcome: {}", outcome_str(&interrupt));
        let still_active = handle.snapshot().await.threads.get(&thread).and_then(|t| t.active_turn_id.clone());
        println!("[1] active turn after interrupt: {:?}  => {}",
            still_active.as_deref().map(|s| &s[s.len()-8..]),
            if matches!(interrupt, ActionOutcome::Confirmed{..}) && still_active.is_none() {"PASS ✅"} else {"CHECK"});

        // ---- TEST 2: start a turn and let it complete ----
        println!("\n=== TEST 2: start a short turn and let it complete ===");
        let short = vec![json!({"type":"text","text":"Reply with exactly the word: pong"})];
        let start2 = handle.start(&thread, short).await;
        println!("[2] start outcome: {}", outcome_str(&start2));
        // wait until no active turn (completed)
        let mut completed = false;
        for _ in 0..50 {
            if handle.snapshot().await.threads.get(&thread).and_then(|t| t.active_turn_id.clone()).is_none() {
                completed = true; break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        println!("[2] turn completed: {}  => {}", completed, if completed {"PASS ✅"} else {"CHECK"});

        // ---- TEST 3: steer semantics on a fresh long turn, then interrupt ----
        println!("\n=== TEST 3: start a long turn, steer it, then interrupt ===");
        let _ = handle.start(&thread, long()).await;
        if let Some(turn3) = active_turn(&handle, &thread, 8).await {
            let steer = handle.steer(&thread, &turn3, vec![json!({"type":"text","text":"Also mention IPv6 briefly."})]).await;
            println!("[3] steer outcome: {}", outcome_str(&steer));
            let intr = handle.interrupt(&thread, &turn3).await;
            println!("[3] interrupt outcome: {}", outcome_str(&intr));
        } else {
            println!("[3] no active turn to steer");
        }

        // ---- TEST 4: event store + streams sanity ----
        println!("\n=== TEST 4: observability surfaces ===");
        let snap = handle.snapshot().await;
        println!("[4] snapshot: {} threads tracked, sequence={}", snap.threads.len(), snap.at_sequence);
        let q = handle.query_sequence(Some(&thread), 0, None).await;
        println!("[4] stored events for this thread: {}", query_len(&q));
        println!("[4] transport health: {:?}", handle.health().borrow().phase);

        // ---- TEST 5: watch window for a GUI/human-initiated turn, and interrupt it ----
        println!("\n=== TEST 5: watching 45s — start a chat in the ChatGPT app now; I'll interrupt it ===");
        let mut caught = false;
        for _ in 0..225 {
            let snap = handle.snapshot().await;
            for (tid, st) in snap.threads.iter() {
                if tid == &thread { continue; }
                if let Some(turn) = &st.active_turn_id {
                    println!("[5] caught GUI turn thread={} turn={} — interrupting", &tid[tid.len()-8..], &turn[turn.len()-8..]);
                    let out = handle.interrupt(tid.clone(), turn.clone()).await;
                    println!("[5] interrupt outcome: {}  => {}", outcome_str(&out), if matches!(out, ActionOutcome::Confirmed{..}) {"PASS ✅"} else {"CHECK"});
                    caught = true;
                }
            }
            if caught { break; }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if !caught { println!("[5] no GUI turn seen in the window (skipped)"); }

        boot.shutdown().await;
        println!("\n=== done ===");
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await??;
    Ok(())
}

fn target_thread(t: &ActionTarget) -> &str {
    match t {
        ActionTarget::Turn { thread_id, .. } | ActionTarget::Thread { thread_id } => thread_id,
    }
}
fn outcome_str(o: &ActionOutcome) -> String {
    match o {
        ActionOutcome::Confirmed { target, evidence } => format!(
            "Confirmed (thread={}, note='{}')",
            short(target_thread(target)),
            evidence.note
        ),
        ActionOutcome::Rejected { reason, .. } => format!("Rejected ({reason})"),
        ActionOutcome::OutcomeUnknown { evidence, .. } => {
            format!("OutcomeUnknown (note='{}')", evidence.note)
        }
    }
}
fn short(s: &str) -> &str {
    if s.len() > 8 { &s[s.len() - 8..] } else { s }
}
fn query_len(q: &store::QueryResult) -> usize {
    q.events.len()
}
