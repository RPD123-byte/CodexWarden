## Why

We can now observe and control the ChatGPT desktop app's live Codex sessions from an
independent local process — by running the app against Codex's managed app-server
daemon and joining that daemon as a second client (proven end-to-end in
`experimentation/`). We need a lightweight Rust control layer that an always-on
downstream consumer can embed: it ingests the shared daemon's live event stream and
exposes safely correlated Codex actions, so a facial-expression consumer can align
expressions with events and interrupt/steer an explicitly selected turn in real time.
This change establishes that library foundation; a standalone binary remains a later
packaging change.

## What Changes

- Introduce a Rust Cargo workspace (`crate-per-concern`, mirroring codex-rs) for the
  `codex-app-control` daemon; all prior JS spikes are frozen under `experimentation/`.
- Generate a typed `protocol` crate from Codex's JSON Schema (via `typify`), replacing
  the idea of vendor-copying codex-rs Rust types. Normal CI regenerates from the pin; a
  scheduled job checks the latest version; recorded inner messages test type fidelity.
- Add a WebSocket-over-unix-socket transport to the managed daemon with
  reconnect+backoff.
- Add thread ingestion using global creation/status pushes plus bounded periodic
  reconciliation, subscribe with rollout-write retry, and close the turn-start race with
  `thread/read`. Successful subscriptions are retained by default until global
  reactivation delivery is proven; capacity degradation is explicit rather than silent.
  The wire protocol has no reliable Desktop discriminator, so all active shared-daemon
  threads may be observed; actions always require an explicit thread target.
- Add an authoritative lifecycle reducer and bounded lifecycle replay, followed by two
  notification topics: lifecycle and explicitly lossy deltas. Every inbound frame gets
  a receipt sequence so consumers can recover gaps and reconstruct cross-plane order.
- Add an in-memory, time-indexed, hard-bounded event store with dual clocks, receipt
  sequence, idle eviction, and query/replay APIs (no persistence; jsonl owns history).
- Add the Codex action space (`turn/interrupt`, `turn/steer`, `turn/start`, …) with
  per-action confirmation, outcome states, and retry safety: only known-not-written
  requests are retried; ambiguous writes return/reconcile as `OutcomeUnknown` rather
  than being duplicated. **BREAKING invariant:** the client MUST NEVER respond to
  server→client approval/elicitation requests or override the GUI reviewer.
- Add a GUI supervisor: a conditional daemon-ownership state machine (attach vs. start,
  ongoing health/version checks), with supervisor-owned GUI launch setting
  `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1`; it does not rely on a LaunchAgent to inject an
  environment variable into Dock launches.
- Add an embeddable library API: `CodexControl::run(cfg, |handle| async { … })` with a
  concurrent lifecycle, explicit shutdown, separate lifecycle/delta streams, lifecycle
  replay/snapshot, event queries, and explicit action methods.

## Capabilities

### New Capabilities
- `protocol-types`: Typed Rust representation of the Codex app-server JSON-RPC protocol, generated from the JSON Schema and drift-checked in CI.
- `daemon-transport`: WebSocket-over-unix-socket JSON-RPC client to the managed daemon, with initialize handshake and reconnect+backoff.
- `thread-ingestion`: Discover and reconcile active shared-daemon threads, subscribe and release safely, and close rollout-write/turn-start races.
- `event-streaming`: Maintain authoritative lifecycle state plus bounded replay, then expose sequenced lifecycle and lossy-delta notification topics.
- `event-store`: In-memory, time-indexed, hard-bounded recent-event buffer with explicit clock domains and query/replay APIs.
- `action-control`: Programmatic Codex action space with confirmation, explicit unknown outcomes, safe retry classification, and strict reviewer-request isolation.
- `gui-supervisor`: Ensure the standalone install + managed daemon remain healthy and version-compatible, and own daemon-mode GUI launching; conditional daemon ownership.
- `library-api`: Embeddable concurrent `CodexControl` lifecycle and `Handle` (streams, replay/snapshot, queries, actions, explicit shutdown).

### Modified Capabilities
<!-- None — greenfield project, no existing specs. -->

## Impact

- New Cargo workspace and crates; new dev dependency on `typify` (build-time codegen),
  deterministic pinned-schema CI, a scheduled latest-version drift alarm, and replay of
  sanitized recorded inner messages in the protocol crate for type fidelity.
- Runtime coupling to Codex's managed daemon protocol version (must satisfy the GUI's
  `LB()` compatibility check) and to the `CODEX_APP_SERVER_USE_LOCAL_DAEMON` launch gate.
- macOS-only: depends on `~/.codex/...` paths, `ps eww`, `launchctl`, and the ChatGPT app
  bundle. No external services, brokers, or network egress.
