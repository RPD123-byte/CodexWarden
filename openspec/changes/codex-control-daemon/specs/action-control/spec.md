## ADDED Requirements

### Requirement: Codex action space
The system SHALL expose the Codex thread/turn action space programmatically, including
at least `turn/interrupt`, `turn/steer`, and `turn/start`, addressed by `threadId` (and
`turnId` where required), issued over the daemon transport.

#### Scenario: Interrupt an active turn
- **WHEN** a caller invokes interrupt for a thread with a known active turn
- **THEN** the daemon sends `turn/interrupt {threadId, turnId}` and returns the outcome
  to the caller

#### Scenario: Steer an active turn
- **WHEN** a caller invokes steer with guidance text for a thread with an active turn
- **THEN** the daemon sends `turn/steer` with the required `expectedTurnId` precondition

### Requirement: Per-thread action serialization
The system SHALL serialize actions targeting the same thread so that concurrent or
retried actions cannot interleave and produce inconsistent turn state.

<!-- review-fix (#4): serialize per thread so retries/concurrent actions don't race turn state. -->

#### Scenario: Concurrent actions on one thread ordered
- **WHEN** two actions target the same thread concurrently
- **THEN** they are applied one at a time in a defined order for that thread

### Requirement: Confirmation waiter registered before send
For actions with a completion signal, the system SHALL register the completion waiter
on authoritative ingress BEFORE sending the request, so a confirming notification that
arrives before the JSON-RPC response is not missed and consumer broadcast lag cannot lose
the confirmation. Interrupt confirmation SHALL require observing
`turn/completed(status = interrupted)` for the targeted turn, correlated by
`threadId`/`turnId`.

<!-- review-fix (#4): turn/completed can arrive before the response; register the waiter first to avoid the race. -->

#### Scenario: Completion arrives before the response
- **WHEN** an interrupt is sent and `turn/completed(interrupted)` arrives before the
  JSON-RPC response to the interrupt request
- **THEN** the pre-registered waiter still observes the completion and confirms success

### Requirement: Per-action retry safety
The system SHALL classify retry safety per action rather than retrying blindly, because
a timeout does not prove the request was not executed. It SHALL retry only transport
`NotWritten` failures. For `WrittenOutcomeUnknown`, it SHALL reconcile only when a unique
observation can confirm the request; otherwise it SHALL return `OutcomeUnknown` without
resending.

<!-- review-fix (#4): blanket retry duplicates non-idempotent actions (turn/start, steer). -->

#### Scenario: Known-not-sent failure retried
- **WHEN** an action fails with transport classification `NotWritten`
- **THEN** it is retried, since it is known not to have executed

#### Scenario: Written steer has no unique confirmation
- **WHEN** `turn/steer` was written but its response is lost and thread state does not
  uniquely identify whether that guidance was accepted
- **THEN** the system returns `OutcomeUnknown` and does not resend the guidance

#### Scenario: Concurrent GUI start prevents unique attribution
- **WHEN** `turn/start` was written without a response and the GUI could have started the
  newly observed turn concurrently
- **THEN** the system does not promote the plausible turn to confirmed solely from state;
  it returns `OutcomeUnknown` unless another unique completion contract exists

#### Scenario: Terminal failure logged
- **WHEN** an action is ultimately unresolved after its retry/reconcile policy
- **THEN** the failure is logged for observability, and no message broker or dead-letter
  queue is involved

### Requirement: Explicit action outcomes
Every action SHALL return one of `Confirmed`, `Rejected`, or `OutcomeUnknown`, with the
target thread/turn and supporting response or observation metadata. `OutcomeUnknown`
SHALL be a non-success result that callers must handle explicitly.

#### Scenario: Caller receives an ambiguous result
- **WHEN** an action was written but cannot be uniquely confirmed or rejected
- **THEN** the caller receives `OutcomeUnknown` rather than success or an automatic retry

### Requirement: Read-only on server-initiated requests
The system SHALL NEVER send a response to any server→client request (including
`*/requestApproval` and `elicitation_request`). Where the protocol's `initialize`
handshake allows a client to opt out of handling such requests, the system SHALL declare
that it does not handle them; never-responding SHALL remain the backstop regardless.

<!-- review-fix (G6): opt out via capabilities where possible; never-answer as safe backstop. -->

#### Scenario: Approval request received but not answered
- **WHEN** the daemon receives an `item/commandExecution/requestApproval` (or any
  server→client request) for a subscribed thread
- **THEN** it records the event but sends no response, leaving resolution to the GUI

#### Scenario: Silence does not stall the turn
- **WHEN** the daemon does not answer an approval request and the GUI approves it
- **THEN** the turn proceeds to completion, unaffected by the daemon's silence

### Requirement: Never claim the reviewer role
The system SHALL NOT set or override `approvalsReviewer` on `thread/resume`, `turn/start`,
or any other outbound request, so approval routing stays with the GUI.

#### Scenario: Resume preserves GUI as reviewer
- **WHEN** the daemon resumes a thread to subscribe to its events
- **THEN** it does not set `approvalsReviewer` to itself, and the GUI continues to
  receive and resolve approval prompts

#### Scenario: Start turn preserves GUI reviewer
- **WHEN** the daemon constructs `turn/start`
- **THEN** it omits the `approvalsReviewer` override and does not alter reviewer routing

### Requirement: Interrupt outcome is correlated, not attributed
The system SHALL treat a confirming `turn/completed(interrupted)` as a correlated
outcome (this client issued an interrupt for that thread/turn and observed it end
interrupted), NOT as proof of causation, since the notification carries no causal request
id and a simultaneous GUI interrupt would be indistinguishable.

<!-- review-fix (G1): high request ids do not prove causation; reframe as best-effort correlation. -->

#### Scenario: Interrupt reported as correlated outcome
- **WHEN** the daemon interrupts a turn and observes `turn/completed(interrupted)` for it
- **THEN** it reports a correlated interruption for that thread/turn, without asserting it
  was the sole cause
