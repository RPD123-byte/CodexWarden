## Context

Codex's ChatGPT desktop app normally spawns private per-window `codex app-server`
engines over stdio, unreachable by other processes. But when the standalone Codex is
installed and its **managed app-server daemon** is running, launching the GUI with
`CODEX_APP_SERVER_USE_LOCAL_DAEMON=1` makes it connect to that daemon over a local
WebSocket-framed unix socket (`~/.codex/app-server-control/app-server-control.sock`,
path `/rpc`) instead. Its chat sessions then live in the shared daemon, and any second
client on the socket can observe threads and interrupt/steer turns. All of this is
verified in `experimentation/` (see its README), including the failure modes.

This control layer is that second client. It is packaged as a lightweight Rust library
embedded by the always-on downstream facial-expression consumer; a standalone binary is
a later packaging change. Its job is to (a) keep the GUI in daemon mode while the host
process is running, (b) ingest the live event stream, and (c) expose safely correlated
actions against an explicitly selected thread/turn.

## Goals / Non-Goals

**Goals:**
- Lightweight, single-user, embeddable in an always-on host: low idle overhead, no GC,
  no external services.
- Faithful, self-maintaining protocol types (generated from the JSON Schema).
- Real-time ingestion of active shared-daemon threads with authoritative lifecycle state,
  bounded transition replay, and explicit degradation when replay history is exhausted.
- Safe coexistence with the GUI user (never touch approvals; never steal reviewer role).
- Embeddable-library-first, with a standalone daemon binary as a trivial later add.
- Resilient to daemon/GUI restarts and laptop sleep (reconnect + resubscribe).

**Non-Goals:**
- Multi-user / horizontal scaling / high availability.
- Durable event storage (jsonl already persists everything; our store is ephemeral).
- Message brokers (Kafka/NATS) or a dead-letter queue.
- Cross-platform support (macOS-only initially).
- Reimplementing the GUI or answering approval/elicitation prompts.
- The facial-expression logic itself (that is the downstream consumer).

## Decisions

### D1. Protocol types: generate from JSON Schema, not vendor-copy
`codex-rs/app-server-protocol` references `codex_protocol::` ~450 times across 15
submodules and carries 4 derives per type (`Serialize/Deserialize/JsonSchema/TS`).
Vendor-copying drags in the whole `codex-protocol` crate and forces stripping ~1,260
tooling annotations. Instead, generate a `serde`-only `protocol` crate from the 39
self-contained JSON Schema files (via `typify`), pinned to a Codex version.
- **Alternatives:** vendor-copy Rust (messy, heavy re-sync); git-dependency on their
  crate (pulls the whole tree, may not build standalone).
- **Fidelity guardrails (CI):** (1) deterministically regenerate from the pinned Codex
  version and diff against the committed snapshot; (2) run a separate scheduled
  latest-version drift alarm; (3) round-trip the protocol crate's sanitized, representative
  inner-message fixture derived from captured traffic. The fixture exercises envelope
  fidelity only, not managed-daemon behavior, and keeps the Rust workspace independent
  from `experimentation/`.
- **Unknown-method resilience (review-fix G5):** the reader uses a known-message envelope
  with a raw fallback, so a new upstream method is captured as a raw/unknown event and
  logged rather than failing the reader before CI drift detection runs. Generated types
  must NOT be a fail-closed exhaustive enum at the wire boundary.

### D2. Transport: one WS in, WebSocket-over-UnixStream
Connect `tokio::net::UnixStream` → `tokio-tungstenite` client handshake to
`ws://.../rpc`. A single connection task owns the split reader/writer. JSON-RPC framing:
newline is not used; each WS text frame is one JSON-RPC message. The transport records
whether each request was queued, written, or answered, and classifies disconnects as
`NotWritten` versus `WrittenOutcomeUnknown`; pending calls complete with that
classification rather than hanging across reconnect. Ping/pong or bounded read-idle
health detection covers half-open connections and laptop sleep. Reconnect uses
exponential backoff, repeats `initialize`/`initialized`, then invokes ingestion
reconciliation before normal action traffic resumes. Request ids need only be unique on
this connection; a high range is useful for logs but is not a cross-client guarantee.
- **Alternatives:** raw ndjson to the control socket (rejected — the socket speaks the
  WebSocket handshake, proven; raw JSON is dropped).

### D3. Authoritative ingress + bounded replay + two-plane notification
A bounded `broadcast` channel is LOSSY — it overwrites on lag and `Lagged(n)` only
reports the loss. So lifecycle correctness must NOT live in a channel. The single reader
task assigns a **monotonic receipt sequence to every inbound frame** at read time. Known
event-bearing messages (notifications and server-initiated requests) then flow through
the authoritative internal path before consumer fan-out:

`reader → sequence → reducer + event store + action waiters → broadcast topics`

The reducer owns current thread/turn state; the event store records retained notifications
and reviewer requests; action completion waiters are notified directly from ingress. A bounded lifecycle
transition log supports `events_since(sequence)`. A lagging consumer first replays retained
transitions and then snapshots current state. If its requested sequence predates the log,
the API returns `GapTooOld` plus a snapshot: current state is restored, but unavailable
historical transitions are honestly reported rather than claimed recovered. Only after
authoritative processing does ingress publish lifecycle and lossy-delta
`tokio::sync::broadcast` topics. The receipt sequence reconstructs cross-plane order.

### D4. Ingestion lifecycle (multi-signal discovery + bounded reconciliation)
Discovery uses BOTH global pushes: `thread/started` (new) and `thread/status/changed`
(activation, incl. resumed conversations), but global delivery of the latter to a second
client remains a required live-spike check. On connect/reconnect/interval, **reconcile**
against `thread/list` sorted by `updatedAt` descending, using pagination and a watermark;
only recent/active candidates are passed to `thread/read`. This catches threads activated
while disconnected without rescanning all history.

Subscribe via `thread/resume` with bounded retry (~8×250ms) for the rollout-write race,
then immediately `thread/read {includeTurns}` to catch a turn that started during the
subscribe window. Concurrent discovery signals are deduplicated through a per-thread
subscription state machine. Until global reactivation delivery is proven live, the safe
default is **not to release successful non-ephemeral subscriptions**; store eviction is
independent. A configured subscription cap surfaces `CapacityDegraded` rather than
silently releasing a thread and claiming zero blind spots. Idle release may be enabled
only after the reactivation signal is verified; periodic reconciliation remains a
backstop but is not claimed to recover a short turn that begins and ends between polls.

**Desktop-only filtering is dropped:** the wire `Thread` has no `originator`; live
Desktop threads report `source: "vscode"`/`threadSource: null`; the working spike
subscribed to everything. Any narrower selection (by `cwd`, or by reading rollout
`session_meta.originator`) is an explicit later filter. Observation does not authorize
action: every action requires an explicit thread/turn target, and no API means "act on
all active threads."

### D5. Action control + approval safety
Actions are JSON-RPC requests with **per-action completion contracts**, an explicit
`Confirmed | Rejected | OutcomeUnknown` result, and **per-action retry safety**. Retry
only transport-classified `NotWritten` failures. For a written request with no response,
reconcile where a unique observation exists; otherwise return `OutcomeUnknown` without
resending. `turn/steer` generally has no unique observable marker, and a new turn could
also have been started concurrently by the GUI, so plausible state is not promoted to
`Confirmed`.

Actions are serialized per thread within this client; this does not claim serialization
against the GUI. Completion waiters are registered on the authoritative ingress path
before the request is sent. Interrupt success is a **correlated** outcome (we issued an
interrupt for this pair and saw it end interrupted), not proof of causation. Failures are
logged (no DLQ). **Hard invariant:** read-only on the server→client request channel — we
receive `*/requestApproval` and `elicitation_request` (fan-out, proven) but MUST NEVER
respond, and MUST NOT set or override `approvalsReviewer` on `thread/resume`,
`turn/start`, or any other request. Where `initialize` allows opting out we declare it;
never-answer remains the backstop.

### D6. Storage: in-memory, hard-bounded, sequence-preserving
The authoritative ingress writes a per-active-thread buffer carrying Unix receipt time
in milliseconds, monotonic time since this run began, `emittedAtMs`, and receipt sequence.
Queries from external sensors use Unix milliseconds plus an explicit before/after
tolerance; sequence is the ordering authority when wall time moves. Defaults are a
10-minute raw correlation window, 8 MiB per thread, and 64 MiB globally, all configurable.
Time alone is not treated as a memory bound. On a byte-cap conflict, oldest deltas are
summarized with start/end time and sequence spans before lifecycle records. Lifecycle
replay has its own bounded log and exposes `GapTooOld` when exhausted. Evict event content
on idle while retaining reducer/subscription state independently. No filesystem metadata;
purpose is immediate correlation, not history.

### D7. Library-first, concurrent lifecycle
`core` is a library exposing `CodexControl::run(cfg, |handle| async move { … }).await`.
`run` performs shared init, **spawns all background loops (reader, reducer, ingestion,
streaming) so they are running, then invokes the consumer closure CONCURRENTLY** with
them — NOT sequentially before an event loop, which would deadlock a closure that awaits
events forever. Explicit `handle.shutdown().await` and normal closure return perform
graceful shutdown: stop loops, unsubscribe active threads, and close the socket. Dropping
the `run` future signals cancellation and closes owned resources promptly, but async
unsubscribe is documented as best effort because a dropped future cannot await cleanup.

The consumer obtains capabilities only through `Handle`, which exposes two explicit
pulled streams (`lifecycle()`, `deltas()`), `events_since(sequence)`, a reducer snapshot,
time-window event queries, health/degradation state, explicit action methods
(`interrupt`, `steer`, start-turn), and shutdown. Init-before-closure ordering is enforced
by data dependency. A standalone daemon binary is a later change.

### D8. GUI supervisor: conditional daemon ownership
Init is a state machine, not a blind `daemon start` (which collided with the GUI's own
remote-control management, observed live):
1. Ensure the standalone Codex install exists (else guide the one-time installer).
2. Detect a running managed daemon. If none, we start it. If the GUI's remote-control
   owns one, attach to it — do not start a competitor.
3. Verify the daemon version satisfies the GUI's `LB()` compatibility expectation.
4. Ensure the GUI runs in daemon mode using one committed default: **supervisor-owned
   launch**. Detect an already-correct instance via `ps eww` plus a successful shared
   socket connection and leave it alone. For a non-daemon-mode instance, request graceful
   quit and relaunch ChatGPT directly with `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1`; if it
   does not exit before a timeout, return `UserActionRequired` and never force-kill by
   default. A LaunchAgent does not inject into Dock launches, `launchctl setenv` is not
   used, and `ipc.sock` cannot mutate a running process environment.
5. Continue supervising after init: detect GUI/daemon restart, repeat compatibility and
   connection checks, and re-enter attach/start/launch/reconcile as needed. Preflight uses
   `codex app-server daemon version`; successful GUI attachment to the shared socket is
   the final compatibility proof.

Daemon ownership grants permission to start, not permission to disrupt other clients.
Whether this library started or attached to the managed daemon, shutdown leaves it
running by default because the GUI may still depend on it.

## Risks / Trade-offs

- **Protocol drift on Codex updates** → deterministic pinned regeneration protects normal
  CI; a scheduled latest-version alarm reports upstream drift; raw unknown fallback keeps
  runtime observation alive until types are regenerated.
- **Version-compat break (`LB()`) after a GUI auto-update** → ongoing supervisor health
  checks repeat preflight and verify actual GUI attachment to the shared socket.
- **Daemon-ownership collision with the GUI's remote-control** → conditional init
  (D8) attaches instead of starting when the GUI already owns the daemon.
- **Restarting the user's running GUI is disruptive** → preserve a correct instance;
  otherwise request graceful quit with timeout and never force-kill by default.
- **Approval misrouting stalls turns** → strict read-only rule on server requests
  (D5), enforced in code and covered by a replay test.
- **Consumer backpressure** → broadcasts are notification-only; authoritative ingress
  updates reducer/store/action waiters first. Lifecycle gaps use bounded replay and then
  an explicit `GapTooOld` + snapshot; deltas remain lossy for consumers.
- **User launches GUI from Dock without the env var** → ongoing supervision detects the
  wrong mode, requests graceful quit, and relaunches through the supervisor-owned path.
- **Ingestion goes blind on resumed/disconnect-window threads (review-fix #1)** →
  multi-signal discovery (`thread/status/changed`) + connect/reconnect/interval
  bounded reconciliation via `thread/list`/`thread/read`; idle release stays disabled
  until the global reactivation signal is proven.
- **"Lossless channel" is a contradiction** → authoritative ingress holds current truth;
  bounded replay recovers retained transitions, while `GapTooOld` honestly marks history
  that a snapshot cannot reconstruct.
- **Lifecycle deadlock (review-fix #3)** → background loops spawned and running before the
  consumer closure; closure runs concurrently; explicit cancellation.
- **Retry duplicates non-idempotent actions** → retry only `NotWritten`; reconcile only
  unique evidence; otherwise return `OutcomeUnknown`; serialize locally and register the
  authoritative waiter before send.
- **No Desktop discriminator on the wire (review-fix #5)** → drop Desktop-only gating;
  subscribe to all managed-daemon threads; add a narrower explicit filter only if needed.
- **Unknown upstream method breaks the reader (review-fix G5)** → known-envelope + raw
  fallback, logged, never fail-closed at the wire boundary.

## Migration Plan

Greenfield — no migration. Rollout is: scaffold the workspace, generate `protocol`,
build bottom-up (transport → reducer/streaming/ingestion/store → control → supervisor →
library-api), validate each crate against a **scripted stateful mock app-server** before
touching a live app. The mock serves the WebSocket `/rpc` handshake over a unix socket
and must exercise behaviors the stdio replay corpus cannot: request write-state failure,
half-open reconnect, bounded discovery/reconciliation, capacity degradation, lifecycle
replay gaps, store cap pressure, approval silence, and interrupt-confirmation ordering.
The protocol crate owns a sanitized **type-fidelity** fixture of representative inner
messages; it is not behavioral input. The original JS spikes and raw captures stay
isolated in `experimentation/` for reference and are never imported by the Rust workspace.

## Open Questions

- Whether `turn/steer` needs a distinct confirmation signal beyond its response
  (verify live; until then a written request without a response is `OutcomeUnknown`).
- Reconciliation interval and whether `thread/status/changed` reliably fires for resumed
  conversations (verify before enabling idle subscription release; reconciliation is only
  a bounded backstop, not a claim to recover already-finished short turns).
- Whether `initialize` exposes a capability to opt out of server→client requests, or
  never-answer is the only lever (verify during `action-control`; never-answer is the
  backstop either way).
