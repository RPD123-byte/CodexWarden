# Codex App Control

Control layer for the facial-expression interrupt daemon: create/interrupt/steer Codex sessions, including the ChatGPT **desktop GUI's own** sessions.

This directory is an isolated research workspace, not a dependency of the Rust library. Run `npm ci` here before using the JavaScript WebSocket probes. Its generated `node_modules/`, captured logs, NDJSON traffic, and cloned app bundles remain local and are not published.

## ★★ BEST APPROACH — daemon mode, NO shim (2026-07-20)

The ChatGPT GUI can be pointed at the **managed app-server daemon** instead of spawning private engines, which makes its chat sessions live in a shared daemon we can join as a second client. Fully verified: interrupted a live GUI essay turn via the daemon's local socket — no bundle clone, no shim, no re-signing, no cloud.

Setup:
1. Install standalone Codex (once): `curl -fsSL https://chatgpt.com/codex/install.sh | sh` → installs `~/.codex/packages/standalone/current/codex` (needed by the daemon).
2. Start the managed daemon: `codex app-server daemon start` → exposes `~/.codex/app-server-control/app-server-control.sock` (WebSocket-framed, `ws://.../rpc`).
3. Launch the GUI with the daemon lever ON: `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT`
   - The exact gate (from app.asar): `hostConfig.kind==='local' && env.CODEX_APP_SERVER_USE_LOCAL_DAEMON==='1' && env.CODEX_APP_SERVER_FORCE_CLI!=='1' && !env.CODEX_CLI_PATH && await LB()` where `LB()` runs `codex app-server daemon version` and checks compatibility. True → GUI connects via `ws://localhost/rpc` to the daemon; false → spawns a private stdio engine.
   - GUI log confirms: `Transport start success ... transport=websocket`, `currentVersion=0.144.6` (the daemon).
4. Our daemon connects as a SECOND client and interrupts: `daemon-client.js` (uses `ws` over `ws+unix://<sock>:/rpc`), does `initialize`, `thread/list` (sees the GUI's chats!), `thread/read {includeTurns}` to find the `inProgress` turn, then `turn/interrupt {threadId, turnId}` → status `interrupted`, GUI stops mid-generation.

### Real-time autonomous pipeline (verified 2026-07-20) — `interrupt-daemon.js`

Fully hands-off: a new GUI chat is detected, subscribed, and its turn interruptible in real time, no polling.
1. `thread/started` is pushed **globally** to every connected client the instant a new chat is created (threadId at `params.thread.id`). No subscription needed to detect new threads.
2. Turn-level events (`turn/started`, items, `turn/completed`) are NOT pushed until you subscribe. Subscribe by `thread/resume {threadId}` (its docs: "if thread_id identifies a running thread, app-server rejoins that thread").
3. Race: right after `thread/started`, the rollout file may not be on disk yet → `thread/resume` returns "no rollout found". Retry ~8×250ms until it succeeds.
4. Second race: `turn/started` can fire during the subscribe-retry window (missed push). Close it by calling `thread/read {includeTurns}` immediately after subscribing and picking up any `inProgress` turn.
5. Track `threadId→turnId` from `turn/started` (or the read); on trigger, `turn/interrupt`.

Verified live: daemon auto-caught a new GUI chat, subscribed, caught the in-flight turn, and interrupted it → `turn/completed status=interrupted`, essay never rendered. The title-generator thread can't be subscribed ("no rollout found", it's ephemeral) — ignore it.

Notes:
- Daemon 0.144.6 was accepted by the GUI's 0.145-alpha bundle (version check passed).
- Files: `daemon-client.js` (one-shot second-client interruptor), `listen-all.js` (event explorer), `interrupt-daemon.js` (the production-shape real-time daemon — replace the `--interrupt-after` timer with your facial-expression trigger calling `interrupt(threadId)`). This SUPERSEDES the shim for GUI chats.

## Shim approach (superseded for GUI chats, kept for reference)

`node demo-interrupt.js` — spawns `codex app-server` over stdio, then:

1. `initialize` → handshake OK
2. `thread/start` → thread created (persists to `~/.codex`, non-ephemeral)
3. `turn/start` → long essay turn, `turn/started` event carries `turn.id`
4. `turn/interrupt {threadId, turnId}` after 5s → returns `{}` in ~10ms
5. `turn/completed` fires immediately with `status: "interrupted"` ✅

## Key protocol facts

- Framing: newline-delimited JSON-RPC 2.0 over stdio (no Content-Length headers).
- Method names come from `codex app-server generate-json-schema --out schema` (see `schema/`).
- `turn/interrupt` requires both `threadId` and `turnId`. Track `turnId` from the `turn/started` notification.
- `turn/steer` is the soft alternative: `{threadId, expectedTurnId, input:[{type:"text",text}]}` injects guidance without cancelling.
- Server-initiated requests (approvals) arrive as JSON-RPC requests with an `id`; you must respond or the turn stalls.
- Also useful: `thread/list`, `thread/resume`, `thread/loaded/list`, `thread/shellCommand`, `review/start`.

## ★ BREAKTHROUGH: interrupting the REAL desktop app via a MITM shim (2026-07-19)

We CAN interrupt an in-flight turn inside the actual ChatGPT desktop app — by owning the app's engine process from spawn, via a man-in-the-middle shim, without modifying the installed app.

How it works:
1. **APFS-clone the bundle** (`cp -cR /Applications/ChatGPT.app ChatGPT-shimmed.app`) — instant, near-zero disk, original untouched.
2. In the COPY, move `Contents/Resources/codex` → `codex.real` and drop a shim at that path. The app resolves the engine via `process.resourcesPath` (bundle-relative), so the copy execs OUR shim.
3. **Ad-hoc re-sign the copy** (`codesign --force --deep -s -`). Modifying a sealed resource breaks the notarized seal ("a sealed resource is missing or invalid"); ad-hoc re-signing makes it internally consistent and locally launchable. Auth carries over (lives in `~/.codex`, file-based) — the shimmed app launches fully logged in.
4. The shim (`shim-codex.js`) execs the real codex, tees the parent(GUI)<->engine JSON-RPC byte-for-byte, logs every frame, and exposes a per-PID control socket (`shim-<pid>.sock`, advertised in `shim-registry.ndjson`).
5. A daemon (`shim-daemon.js`, stand-in for the facial-expression monitor) discovers shims via the registry, watches traffic, and on `turn/started` captures the exact `{threadId, turn.id}` pair and injects `turn/interrupt` (high id range, no collision with GUI ids) into the engine's stdin.

Result: drove the app's GUI to start long essay turns; the daemon interrupted each → `turn/completed status: "interrupted"`, and the essay never streamed in the UI. Verified live 3x.

Architecture (measured 2026-07-20, PID-tagged + full-frame shim capture):
- The FIRST message in a new chat spawns TWO threads: (1) the persisted main conversation, and (2) an EPHEMERAL title-generator thread. The second thread is NOT a worker sub-agent — its turn/start input is literally "generate a concise UI title (up to 36 characters)... fill the structured title/description fields." Its output (e.g. `{"title":"Identify Japan's tallest mountain","description":"..."}`) becomes the sidebar chat name. It is NOT persisted to disk (main thread IS).
- The "you are the primary agent in a team of agents" text is the MAIN thread's system prompt (a capability description), NOT evidence a worker was spawned. For trivial queries no worker runs — the only companion thread is the titler.
- BOTH threads run in ONE engine process — the `code_mode_host` engine. "Two conversations" does NOT mean "two engine processes." threads = conversation data objects multiplexed inside an engine, not processes.
- The other `app-server --listen stdio://` processes the GUI spawns are near-inert: they only do initialize/getAuthStatus/account/read (probe/handshake connections), no turns.
- The code_mode_host engine is the workhorse: hosts the threads AND runs `process/spawn` (×27 in one message) for tool/child-process execution — those spawned children are the real subprocesses.
- Terminology: engine process (real, in `ps`) > thread (a conversation, data) > turn (one msg→reply) > item. Only the engine and tool-exec children are OS processes.

Gotchas learned:
- The GUI spawns the engine MULTIPLE times (startup probes + a `code_mode_host` engine + the chat engine `app-server --listen stdio://`). Use per-PID sockets + a registry, not one fixed socket path.
- Each user submission produced TWO turns on TWO threads (main chat + a second, likely code-mode/host thread). Interrupt each turn by its OWN `turn/started` pair — do NOT track a single global turnId (that crossed ids and the engine rejected the interrupt with a JSON-RPC error, `hasResult:false`).
- `turn/started` params contain both `threadId` and `turn.id` together — capture them atomically from that one event.
- Every app update overwrites the real bundle; the shimmed copy must be rebuilt (re-clone + re-sign) after updates.

Files: `shim-codex.js` (MITM), `shim-daemon.js` (interrupt daemon), `ChatGPT-shimmed.app` (the working shimmed copy), `shim-registry.ndjson` / `shim-log.ndjson` (runtime artifacts).

## Desktop-session interruption — tested end-to-end (2026-07-19)

Live experiment: drove the ChatGPT desktop app to start a long turn, then tried to interrupt it from our own app-server (desktop-bundled binary, shared `~/.codex` store):

- `thread/list` DOES show desktop threads (`originator: "Codex Desktop"`), and `thread/resume` on an idle desktop thread works (with the 0.145 binary — homebrew 0.133 can't parse 0.145 rollouts: "does not start with session metadata").
- While the desktop turn was actively running, our `thread/read` showed the persisted copy as **idle** — the in-flight turn exists only in the Desktop's private app-server process.
- `turn/interrupt` with the real in-flight `turn_id` (pulled from the rollout jsonl): without resume → `thread not found`; after `thread/resume` → `no active turn to interrupt`. Exact reproduction of openai/codex#25914. `thread/loaded/list` is empty.
- UI channel works: clicking the app's Stop button aborted the turn (`turn_aborted` in the rollout) — that's the only interrupt path into Desktop-owned turns today.

**Conclusion (matches Synapse's design, ChrisRoyse/Synapse#958): interruptibility must be arranged at spawn time.** Synapse launches `codex app-server --listen ws://127.0.0.1:<port>` per agent, records `(endpoint, threadId, turnId, pid)` in a `codex-control.json` artifact, and a *second* WS client later connects to the same endpoint and fires `turn/interrupt` — works because the turn is loaded in that same process. Copyable details: `capabilities.experimentalApi: true` on initialize, `expectedTurnId` precondition on steer, refresh turnId from `turn/started`/`turn/completed`, PID-liveness check before interrupting, approval-request forwarding bridge.

## Desktop app findings

- ChatGPT.app runs its own child: `/Applications/ChatGPT.app/Contents/Resources/codex -c features.code_mode_host=true app-server` — stdio transport only, not attachable.
- ChatGPT.app (main process) listens on `~/.codex/ipc/ipc.sock`, but this is the Electron **single-instance lock + CLI/deep-link handoff** socket (evidence: `requestSingleInstanceLock`, `second-instance`, dir chmod 0700, `handoff` ×227, `openInApp` ×65), NOT an engine endpoint. It routes "open project / focus / second-instance" to the GUI, never carries app-server JSON-RPC — which is why it silently closes on JSON-RPC. The engine has no listen socket by design (private stdio), hence the shim.
- No managed daemon running (`codex app-server daemon version` → no control socket). `codex app-server daemon start` could create a shared daemon we and other clients attach to — untested.
- Threads we create share `~/.codex` auth/config/sessions with the app, and you can pass `--desktop-binary` to use the app's bundled codex binary.

## Files

- `codex-controller.js` — reusable controller class (spawn, request/notify plumbing, `startThread`, `startTurn`, `interrupt`, `steer`, active-turn tracking)
- `demo-interrupt.js` — live interrupt test (`--delay ms`, `--desktop-binary`)
- `probe.js` — unix-socket probe used against `ipc.sock`
- `schema/` — generated protocol JSON schemas

## Next steps for the daemon integration

- Expose `interrupt(threadId)` over a tiny local HTTP/WS endpoint the facial-expression daemon can hit.
- Decide policy: `turn/steer` ("pause and check in") below threshold, `turn/interrupt` above it.
- Handle approvals properly (currently auto-denied in the demo).
