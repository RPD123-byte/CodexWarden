## ADDED Requirements

### Requirement: Discover active threads from multiple signals
The system SHALL discover threads from creation/status signals plus reconciliation. It
SHALL treat `thread/started` and any globally received `thread/status/changed` activation
as discovery triggers. Global second-client delivery of the status signal SHALL be
verified by a live spike before idle subscription release is enabled.

<!-- review-fix (#1): thread/started alone goes blind on resumed conversations, which emit no creation event. -->

#### Scenario: New chat created in the GUI
- **WHEN** the GUI creates a new chat and the daemon pushes `thread/started`
- **THEN** the ingestion service observes the new thread id from `params.thread.id`
  without having subscribed to it beforehand

#### Scenario: Existing conversation resumed
- **WHEN** the user reopens an existing conversation and sends a message, so the daemon
  pushes `thread/status/changed` for a thread with no fresh `thread/started`
- **THEN** the ingestion service treats it as an active thread and subscribes to it

### Requirement: Reconciliation on connect, reconnect, and interval
The system SHALL reconcile its active-thread set against the daemon on connect,
on reconnect, and periodically. It SHALL request `thread/list` sorted by `updatedAt`
descending, paginate with a configured candidate bound and watermark, and call
`thread/read` only for recent/active candidates. Reconciliation SHALL subscribe to newly
active threads it is not already tracking without rescanning all history.

<!-- review-fix (#1): re-subscribing only previously-active threads misses threads activated during a disconnect. -->

#### Scenario: Thread activated during a disconnect
- **WHEN** the connection was down while a thread became active, and on reconnect that
  thread is currently active per `thread/list`/`thread/read`
- **THEN** reconciliation discovers it and subscribes, rather than remaining blind to it

#### Scenario: Reconciliation ignores inactive history
- **WHEN** reconciliation lists many historical threads that are not currently active
- **THEN** it does not subscribe to the inactive ones

### Requirement: Thread selection without a Desktop discriminator
The system SHALL subscribe without relying on a nonexistent Desktop-only wire field.
The wire `Thread` exposes `source`, `threadSource`, `agentRole`, and `agentNickname` but
no `originator`, and observed Desktop threads report `source: "vscode"`. The system SHALL
observe all active threads on the managed daemon. Any narrower selection SHALL be an
explicit configured filter, not an assumed notification property.

<!-- review-fix (#5): confirmed empirically — no wire `originator`; source=="vscode" for Desktop threads; the working spike subscribed to everything. -->

#### Scenario: Subscribe without relying on a Desktop field
- **WHEN** a thread becomes active and its wire object carries `source: "vscode"` and no
  `originator`
- **THEN** the system still subscribes to it, because selection does not depend on a
  Desktop-only wire discriminator

### Requirement: Subscribe with rollout-write retry
The system SHALL subscribe to a thread by issuing `thread/resume`, and SHALL retry with
bounded backoff when the resume fails because the thread's rollout is not yet written to
disk ("no rollout found").

#### Scenario: Resume before rollout exists
- **WHEN** `thread/resume` is issued immediately after discovery and fails with "no
  rollout found"
- **THEN** the service retries with bounded backoff until the resume succeeds or the
  retry budget is exhausted

#### Scenario: Ephemeral thread never resolvable
- **WHEN** a thread (e.g. the ephemeral title-generator) never produces a rollout
- **THEN** the service gives up after the retry budget without erroring the daemon

### Requirement: Close the turn-start race
After a successful subscribe, the system SHALL immediately issue `thread/read` with
turns included and adopt any `inProgress` turn as the thread's active turn, so a turn
that started during the subscribe window is not missed.

#### Scenario: Turn started during subscribe window
- **WHEN** a turn began between discovery and the completed subscribe, so its
  `turn/started` push was not received
- **THEN** the follow-up `thread/read` finds the `inProgress` turn and records it as the
  active turn for that thread

### Requirement: Deduplicate concurrent discovery
The system SHALL serialize subscription state transitions per thread so simultaneous
creation, status, and reconciliation signals do not issue competing `thread/resume`
requests or overwrite newer active-turn state.

#### Scenario: Two discovery signals identify one thread
- **WHEN** a global notification and reconciliation discover the same thread concurrently
- **THEN** one subscription state machine performs the resume/read sequence exactly once

### Requirement: Bounded subscriptions with explicit degradation
The system SHALL retain successful non-ephemeral subscriptions by default until global
second-client reactivation delivery is proven. It SHALL enforce a configurable
subscription cap by reporting `CapacityDegraded`, not by silently releasing a thread and
claiming zero blind spots. Idle release MAY be enabled only after the reactivation signal
is verified; periodic reconciliation remains a bounded backstop.

<!-- review-fix (#1): idle-unsubscribe must not create a blind spot; reactivation must be rediscoverable. -->

#### Scenario: Idle thread released then reactivated
- **WHEN** verified idle release is enabled, a thread is released, and it later reactivates
- **THEN** the system observes the reactivation via the global signal and re-subscribes,
  rather than silently missing the new turn

#### Scenario: Subscription capacity reached before verification
- **WHEN** the configured subscription cap is reached while idle release is disabled
- **THEN** health reports `CapacityDegraded` and identifies the untracked thread

### Requirement: Observation never implies bulk action authority
The system SHALL require every action to name an explicit thread and, where required,
turn. It SHALL NOT expose an implicit "act on all active threads" operation merely because
ingestion observes all active shared-daemon threads.

#### Scenario: Multiple surfaces are active
- **WHEN** GUI, CLI, or editor-originated threads are active on the shared daemon
- **THEN** no action is issued until the consumer explicitly selects its target pair
