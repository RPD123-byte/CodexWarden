## ADDED Requirements

### Requirement: Authoritative lifecycle state is never dropped
The system SHALL maintain authoritative thread/turn lifecycle state in a non-dropping
internal reducer that is updated synchronously as each notification is read from the
socket, BEFORE any broadcast fan-out. Correctness of lifecycle state (which threads
exist, which turns are active) SHALL NOT depend on a bounded broadcast channel, because
a bounded channel overwrites messages when a consumer lags and cannot recover a missed
`turn/started` or `turn/completed`.

<!-- review-fix (#2): a bounded broadcast is lossy; Lagged reports loss, it does not prevent it. Source of truth must be a non-dropping reducer. -->

#### Scenario: Reducer updated before fan-out
- **WHEN** a `turn/started` or `turn/completed` notification is read from the socket
- **THEN** the internal reducer records the state change synchronously before the event
  is published to any broadcast topic

#### Scenario: Lifecycle correctness survives a lagging consumer
- **WHEN** a consumer of the lifecycle topic lags and misses broadcast messages
- **THEN** the authoritative reducer state is still complete and correct, unaffected by
  the consumer's lag

### Requirement: Authoritative consumers precede broadcast fan-out
The system SHALL route each known event-bearing message through authoritative ingress
before consumer broadcast: notifications update the reducer/store/waiters as applicable,
and server-initiated reviewer requests are recorded in the store without response. None
of those authoritative components SHALL depend on a bounded broadcast channel.

#### Scenario: Consumer topic overwrites a notification
- **WHEN** a bounded consumer topic overwrites an event because its receiver lagged
- **THEN** reducer state, retained correlation data, and action confirmation remain
  unaffected because they were updated before fan-out

#### Scenario: Reviewer request is received
- **WHEN** a server-initiated approval or elicitation request arrives
- **THEN** authoritative ingress sequences and records it without emitting a response

### Requirement: Receipt sequencing across planes
The system SHALL assign a monotonic receipt sequence number to every inbound JSON-RPC
frame at read time, before classification, and SHALL attach that frame's sequence to each
derived published or retained event. This preserves total cross-plane ordering.

<!-- review-fix (G2): two topics lose cross-plane ordering without a shared sequence. -->

#### Scenario: Cross-plane reordering possible
- **WHEN** a consumer receives events from both the lifecycle and delta topics
- **THEN** it can reconstruct the original arrival order using the receipt sequence
  numbers

### Requirement: Two consumable topics from one connection
The system SHALL read all inbound notifications from the single daemon WebSocket
connection and publish them onto two in-process broadcast topics: a lifecycle topic and
a delta topic. The system SHALL NOT open a second socket or port to separate the topics.
These topics are for consumer notification; authoritative state lives in the reducer.

#### Scenario: Lifecycle event published to lifecycle topic
- **WHEN** a `thread/started`, `thread/status/changed`, `turn/started`, `turn/completed`,
  or error notification is received
- **THEN** it is published (with its receipt sequence) on the lifecycle topic

#### Scenario: Delta event published to delta topic
- **WHEN** an `item/agentMessage/delta`, `item/reasoning/*Delta`, or output-delta
  notification is received
- **THEN** it is published (with its receipt sequence) on the delta topic

### Requirement: Consumer replay and resynchronization on lag
When a consumer lags on the lifecycle topic, the system SHALL offer bounded lifecycle
transition replay from a requested receipt sequence followed by a reducer snapshot. If
the sequence predates retained replay, the system SHALL return `GapTooOld` with the
snapshot and SHALL NOT claim the unavailable transitions were recovered.

<!-- review-fix (#2): recovery path for lagging consumers. -->

#### Scenario: Lifecycle consumer recovers retained transitions
- **WHEN** a lifecycle consumer detects it lagged (missed broadcast messages)
- **THEN** it requests `events_since(last_sequence)`, applies retained transitions in
  sequence order, then confirms current state from a reducer snapshot

#### Scenario: Lifecycle gap predates replay
- **WHEN** the requested sequence is older than the bounded lifecycle replay log
- **THEN** the system returns `GapTooOld` plus a current-state snapshot, clearly marking
  that historical transitions are unavailable

### Requirement: Delta topic is explicitly lossy
The delta topic SHALL be bounded and MAY drop the oldest deltas under backpressure when
a consumer lags, since delta loss is cosmetic and does not affect lifecycle correctness.

#### Scenario: Delta consumer lags
- **WHEN** a delta-topic consumer falls behind beyond the bounded capacity
- **THEN** the system drops oldest deltas to make progress and continues delivering
  newer deltas

### Requirement: Bounded authoritative state
The authoritative reducer SHALL retain state only for threads that are relevant —
subscribed threads and threads with an active turn — and SHALL NOT accumulate an entry
for every historical thread returned by reconciliation or every thread that emits a
global lifecycle event. Idle, non-subscribed threads SHALL be pruned.

<!-- live-fix: the reducer accumulated ~201 idle threads from thread/list history and global status events. -->

#### Scenario: Idle non-subscribed threads pruned
- **WHEN** global lifecycle events arrive for many threads the daemon is not subscribed
  to, and those threads are idle
- **THEN** the reducer does not retain a state entry for them

#### Scenario: Subscribed and active threads retained
- **WHEN** a thread is subscribed or has an active turn
- **THEN** the reducer retains its state
