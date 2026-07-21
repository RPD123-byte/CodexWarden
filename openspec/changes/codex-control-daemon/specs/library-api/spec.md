## ADDED Requirements

### Requirement: Embeddable core library
The system's core capabilities SHALL be provided as a library crate that a consumer can
embed in its own process, so the consumer calls actions directly with no inter-process
hop. A standalone daemon binary is out of scope for this change.

#### Scenario: Consumer embeds the library
- **WHEN** a consumer program depends on the core library crate
- **THEN** it can start the control layer and invoke actions as in-process function
  calls

### Requirement: Concurrent lifecycle without deadlock
The library SHALL expose a run entry point of the form
`CodexControl::run(cfg, |handle| async { … }).await`. It SHALL complete shared
initialization, then SPAWN all background loops (transport reader, reducer, ingestion,
streaming) so they are actively running, and THEN invoke the consumer closure
CONCURRENTLY with those loops. The consumer closure SHALL NOT be required to return
before events begin flowing.

<!-- review-fix (#3): if the closure awaits events forever but the event loop only runs after the closure returns, it deadlocks. Loops must run concurrently with the closure. -->

#### Scenario: Consumer awaits events and still receives them
- **WHEN** the consumer closure awaits `handle.lifecycle()` (or `handle.deltas()`) in a
  loop and never returns
- **THEN** events flow to it, because the background loops were spawned before the closure
  was invoked and run concurrently with it

#### Scenario: Init completes before the closure
- **WHEN** a consumer calls `run` with its own closure
- **THEN** shared initialization (supervisor readiness, transport connect, reducer +
  ingestion started) completes before the consumer closure is invoked

### Requirement: Explicit cancellation and shutdown
The library SHALL provide `handle.shutdown().await` and clean shutdown when the consumer
closure returns or errors. Those graceful paths SHALL stop background loops, unsubscribe
active threads, and close the socket. Dropping/cancelling the `run` future SHALL signal
task cancellation and close owned resources promptly, but asynchronous unsubscribe SHALL
be documented as best effort because a dropped future cannot await cleanup.

<!-- review-fix (#3): concurrent loops + closure need defined cancellation/shutdown. -->

#### Scenario: Shutdown on closure return
- **WHEN** the consumer closure returns
- **THEN** the library unsubscribes active threads, stops background loops, and closes the
  socket

#### Scenario: Explicit shutdown is awaited
- **WHEN** the consumer calls `handle.shutdown().await`
- **THEN** the library awaits unsubscribe, stops background loops, and closes the socket

#### Scenario: Run future is dropped
- **WHEN** the run future is dropped without explicit shutdown
- **THEN** cancellation is signaled and owned resources close promptly, while remote
  unsubscribe is reported as best effort rather than guaranteed

### Requirement: Capabilities only via the handle
All consumer-accessible capabilities SHALL be reachable only through a `Handle` object
minted by initialization, so a consumer cannot act or read events before init completed.

#### Scenario: No capability without a handle
- **WHEN** a consumer has not obtained a `Handle`
- **THEN** it has no way to call actions or read the event streams

### Requirement: Streams, replay, and state access
The `Handle` SHALL expose the two planes as distinct pulled streams — a lifecycle stream
and a delta stream — not a single ambiguous `events()`. It SHALL also expose
`events_since(sequence)` backed by bounded lifecycle replay and a current-state snapshot.

<!-- review-fix (G3): design promised two topics; a single events() is ambiguous. Also expose snapshot for resync (#2). -->

#### Scenario: Consumer pulls lifecycle and deltas separately
- **WHEN** a consumer obtains `handle.lifecycle()` and `handle.deltas()`
- **THEN** each yields its plane's events (carrying receipt sequence), at the consumer's
  own pace subject to that plane's backpressure policy

#### Scenario: Consumer resynchronizes after lag
- **WHEN** a consumer lags on the lifecycle stream
- **THEN** it requests retained transitions after its last sequence and then obtains a
  current-state snapshot

#### Scenario: Consumer lag exceeds replay retention
- **WHEN** the consumer's last sequence predates bounded lifecycle replay
- **THEN** the handle returns `GapTooOld` plus a snapshot and does not claim missing
  historical transitions were recovered

### Requirement: Event queries and health through the handle
The `Handle` SHALL expose time-window event queries, sequence-based retained-event
queries, and current health/degradation state, so all consumer capabilities remain behind
the initialization-minted handle.

#### Scenario: Consumer correlates an external timestamp
- **WHEN** the consumer queries a thread around Unix time T with a tolerance
- **THEN** the handle returns retained events in receipt-sequence order plus any gap
  metadata

#### Scenario: Subscription capacity degrades
- **WHEN** ingestion reports `CapacityDegraded`
- **THEN** the condition and affected thread are visible through handle health state

### Requirement: Explicit action methods on the handle
The `Handle` SHALL expose the action space explicitly, including at least
`interrupt`, `steer`, and `turn/start`, as async methods addressed by thread, delegating
to the action-control capability's confirmation and retry-safety contracts.

<!-- review-fix (G3): design promised turn/start; the handle must guarantee it, not only interrupt/steer. -->

#### Scenario: Interrupt via the handle
- **WHEN** a consumer calls `handle.interrupt(thread)` for a thread with an active turn
- **THEN** the interrupt action is issued and returns its explicit outcome per the
  action-control contract

#### Scenario: Start a turn via the handle
- **WHEN** a consumer calls the handle's start-turn method for a thread
- **THEN** it receives `Confirmed`, `Rejected`, or `OutcomeUnknown` per the action-control
  contract, with no duplicate turn on an ambiguous write
