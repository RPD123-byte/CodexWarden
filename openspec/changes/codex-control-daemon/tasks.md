## 1. Workspace scaffold

- [x] 1.1 Create the Cargo workspace at repo root with a root `Cargo.toml` and shared lints/deps (tokio, serde, serde_json, tokio-tungstenite, tracing, anyhow/thiserror)
- [x] 1.2 Create empty member crates: `protocol`, `transport`, `reducer`, `ingest`, `streaming`, `store`, `control`, `supervisor`, `core`
- [x] 1.3 Add `tracing-subscriber` init and config (socket/health timeouts, retry limits, reconcile watermark/page bound, subscription cap, correlation window, per-thread/global byte caps) with documented defaults
- [x] 1.4 Add a `justfile`/`Makefile` with build, test, lint, and schema-regen targets

## 2. protocol-types

- [x] 2.1 Vendor the committed JSON Schema snapshot into `src/protocol/schema/` and record the pinned Codex version
- [x] 2.2 Add a `typify`-based build step generating `serde`-only Rust types from the schema
- [x] 2.3 Wrap method-bearing server messages (notifications and server requests) in a known-message envelope with raw fallback; unknown requests remain never-answer
- [x] 2.4 Fidelity test: round-trip a sanitized, crate-owned representative inner-message fixture derived from captured traffic, with no Rust dependency on `experimentation/` — review-fix G7
- [x] 2.5 Normal CI: regenerate from the exact pinned Codex version/commit and fail if the committed snapshot is not reproducible
- [x] 2.6 Scheduled/manual latest-version job: report upstream schema drift separately from normal CI

## 3. daemon-transport

- [x] 3.1 WebSocket-over-`UnixStream` connect to `app-server-control.sock` at path `/rpc`
- [x] 3.2 `initialize` + `initialized` handshake gate; declare no server-request handling if the capability exists — review-fix G6
- [x] 3.3 Request/response correlation with ids unique on this connection and pending write state (`NotWritten` / `WrittenOutcomeUnknown` / answered)
- [x] 3.4 Resolve every pending request on disconnect; never carry or automatically resend an ambiguous write across reconnect
- [x] 3.5 Reconnect with exponential backoff, re-handshake, and a reconcile gate before normal action traffic
- [x] 3.6 Ping/pong or bounded read-idle half-open detection for daemon failure and laptop sleep
- [x] 3.7 Assign a monotonic receipt sequence to every inbound JSON-RPC frame at read time
- [x] 3.8 Unit-test transport write-state classification, half-open detection, and reconnect against the stateful mock (see 9)

## 4. reducer + event-streaming

- [x] 4.1 Authoritative ingress: update reducer/store/action waiters as applicable before fan-out; sequence and record reviewer requests without response
- [x] 4.2 Add a bounded lifecycle transition log, `events_since(sequence)`, and `GapTooOld + snapshot` behavior
- [x] 4.3 Reducer snapshot API for current-state recovery
- [x] 4.4 Two `broadcast` topics (lifecycle, delta) published after authoritative ingress, each event carrying receipt sequence
- [x] 4.5 Delta topic drop-on-lag; lifecycle lag triggers retained replay then snapshot, with honest history-gap reporting
- [x] 4.6 Tests: authoritative components survive broadcast lag; retained replay; `GapTooOld`; cross-plane ordering

## 5. thread-ingestion

- [x] 5.1 Live spike: verify whether a second client globally receives `thread/status/changed` for resumed existing conversations
- [x] 5.2 Multi-signal discovery from `thread/started`, received activation status, and reconciliation
- [x] 5.3 Bounded reconciliation: `thread/list` by `updatedAt` descending with pagination, candidate limit, watermark, then selective `thread/read`
- [x] 5.4 Subscribe to all active shared-daemon threads without a Desktop-only wire gate; require explicit thread/turn targets for actions
- [x] 5.5 Per-thread subscription state machine deduplicating concurrent creation/status/reconcile triggers
- [x] 5.6 `thread/resume` with bounded rollout-write retry; give up gracefully on ephemeral threads
- [x] 5.7 Post-subscribe `thread/read {includeTurns}` to catch in-flight turns
- [x] 5.8 Default to retaining successful non-ephemeral subscriptions until global reactivation is proven; report `CapacityDegraded` at the cap
- [x] 5.9 Enable/test idle release only behind verified reactivation support; never claim polling recovers already-finished short turns
- [x] 5.10 Integration test create, resume, duplicate discovery, disconnect-window activation, capacity degradation, and verified idle reactivation

## 6. event-store

- [x] 6.1 Feed the event store directly from authoritative ingress, never from lossy consumer broadcasts
- [x] 6.2 Store Unix receipt ms, monotonic run time, `emittedAtMs`, and receipt sequence per event
- [x] 6.3 Enforce time, 8 MiB/thread, and 64 MiB global defaults with configurable hard caps and delta-first span summarization
- [x] 6.4 Idle content eviction driven by reducer state without discarding subscription/current lifecycle state
- [x] 6.5 Time-window and sequence query APIs with tolerance, ordering, and retention-gap metadata; tests including high-volume cap pressure

## 7. action-control

- [x] 7.1 Implement explicit-target `turn/interrupt`, `turn/steer` (`expectedTurnId`), and `turn/start`; no implicit act-on-all API
- [x] 7.2 Return `Confirmed | Rejected | OutcomeUnknown` with target and evidence metadata
- [x] 7.3 Register completion waiters on authoritative ingress BEFORE send; never depend on lifecycle broadcast delivery
- [x] 7.4 Retry only transport `NotWritten`; reconcile only unique evidence; never resend or falsely confirm ambiguous steer/start writes
- [x] 7.5 Serialize actions per thread within this client and document that GUI actions remain concurrent
- [x] 7.6 Never respond to reviewer requests and never set/override `approvalsReviewer` on resume, turn/start, or any request
- [x] 7.7 Report interrupt as correlated outcome, not proof of causation; terminal-failure logging with no DLQ
- [x] 7.8 Tests: completion-before-response, broadcast lag, ambiguous steer/start, concurrent GUI start, and approval silence

## 8. gui-supervisor

- [x] 8.1 Detect standalone install; actionable error if missing
- [x] 8.2 Conditional daemon ownership: detect running daemon → attach; else start (no blind start)
- [x] 8.3 Preflight daemon version and verify actual GUI attachment to the shared socket as final compatibility proof
- [x] 8.4 Detect GUI daemon-mode via `ps eww`; leave a correct instance running
- [x] 8.5 Implement supervisor-owned ChatGPT launch with `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1`; no LaunchAgent, session env, Dock, or ipc handoff
- [x] 8.6 Graceful quit with timeout; return `UserActionRequired` and never force-kill by default
- [x] 8.7 Ongoing GUI/daemon restart and compatibility monitoring after initialization
- [x] 8.8 On library shutdown leave the managed daemon running whether started or attached
- [x] 8.9 Tests for ownership, stubborn GUI, restart, auto-update incompatibility, and shutdown state

## 9. Test harness (stateful mock app-server)

- [x] 9.1 Mock that serves the WebSocket `/rpc` handshake over a unix socket
- [x] 9.2 Scripted stateful scenarios: reconnect discovery, bounded reconciliation, duplicate discovery, verified idle reactivation, rollout timing, capacity degradation, and two-plane fan-out
- [x] 9.3 Script transport write-state failures, half-open sockets, server requests, lifecycle replay gaps, and interrupt-confirmation ordering
- [x] 9.4 Wire the mock into CI so transport/reducer/ingest/control tests run with zero live-app dependency
- [ ] 9.5 (Optional) capture fresh managed-daemon WS traffic to enrich the behavioral corpus

## 10. core library-api

- [x] 10.1 Compose transport + reducer + streaming + ingest + store + control + supervisor into `CodexControl`
- [x] 10.2 `CodexControl::run(cfg, |handle| async { … })`: init → SPAWN background loops → invoke closure CONCURRENTLY (no deadlock) — review-fix #3
- [x] 10.3 Graceful `handle.shutdown().await` and closure-return cleanup; dropped-run cancellation closes resources with best-effort remote unsubscribe
- [x] 10.4 `Handle` with lifecycle/delta streams, `events_since`, snapshot, time/sequence queries, health, explicit actions, and shutdown
- [x] 10.5 End-to-end embedded example handling lifecycle replay, `GapTooOld`, health degradation, and `OutcomeUnknown` against the mock

## 11. Post-live-test hardening (folded from live E2E observations)

- [x] 11.1 Bound the authoritative reducer to relevant threads: prune idle, non-subscribed threads on `apply`, and only `reconcile_thread` for active candidates so the snapshot no longer accumulates all `thread/list` history — live-fix (reducer tracked ~201 idle threads)
- [x] 11.2 Reducer/ingest tests: many idle non-subscribed threads stay bounded; subscribed and active threads are retained
- [x] 11.3 Reconstructed active-turn anchor: when the post-resume `thread/read` finds an in-flight turn with no prior wire `turn/started`, record an honest reconstructed store anchor (our receipt time, `reconstructed=true`) so subscribe-boundary turns are correlatable — live-fix (store missed `turn/started` at the subscribe boundary)
- [x] 11.4 Ingest test: a turn active before subscribe yields a `reconstructed` store anchor, distinct from wire events
- [x] 11.5 Re-run the live E2E and confirm the reducer snapshot is bounded and the started thread has a turn anchor
