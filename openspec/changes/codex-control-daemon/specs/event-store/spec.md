## ADDED Requirements

### Requirement: In-memory time-indexed buffer
The system SHALL maintain an in-memory, time-indexed buffer of recent events per active
thread for real-time correlation. It SHALL NOT persist events to disk, because Codex
already durably persists them to rollout jsonl files.

#### Scenario: Recent events queryable by time
- **WHEN** a consumer queries the store for the events around a given timestamp on an
  active thread
- **THEN** the store returns the buffered events in time order

#### Scenario: No disk writes
- **WHEN** the store ingests events
- **THEN** it writes nothing to the filesystem

### Requirement: Store is fed by authoritative ingress
The store SHALL receive known events directly from authoritative ingress before consumer
broadcast fan-out. It SHALL NOT consume the lossy delta broadcast as its source of truth.

#### Scenario: Delta consumer topic lags
- **WHEN** the delta broadcast overwrites old entries for a slow consumer
- **THEN** retained correlation data is unaffected because the store already received the
  events directly from ingress

### Requirement: Per-event timing and receipt sequence preserved
Each stored event SHALL retain Unix receipt time in milliseconds, monotonic time since
the current run began, daemon-provided `emittedAtMs` when present, and receipt sequence.
External time queries SHALL use Unix milliseconds with explicit before/after tolerances;
sequence SHALL determine order when wall time moves.

<!-- review-fix (G2): coalescing to content destroys per-event timing, undermining "events around time T"; keep timing + sequence for correlation. -->

#### Scenario: Timing preserved for correlation
- **WHEN** a consumer asks for the events around time T on a thread
- **THEN** each returned event has its own receipt time and receipt sequence, not a
  single merged timestamp

#### Scenario: Cross-plane ordering reconstructable
- **WHEN** stored events came from both the lifecycle and delta topics
- **THEN** they can be ordered by receipt sequence into their true arrival order

### Requirement: Hard-bounded retention
The store SHALL enforce configurable time, per-thread byte, and global byte limits. The
defaults SHALL be a ten-minute raw correlation window, 8 MiB per thread, and 64 MiB
globally. A time window alone SHALL NOT be treated as a memory bound. When a byte cap is
reached, oldest deltas SHALL be summarized with time/sequence spans before lifecycle
records are removed.

<!-- review-fix (G2): if older data is summarized, keep spans, not just content. -->

#### Scenario: Old data summarized with spans
- **WHEN** data ages past the correlation window and is summarized to bound memory
- **THEN** the summary preserves the time span it covered, not just merged content

#### Scenario: High-volume turn reaches byte cap
- **WHEN** events within the correlation window exceed a per-thread or global byte cap
- **THEN** the store applies the documented delta-first summarization policy and remains
  within the configured hard limit

### Requirement: Event query and sequence replay APIs
The store SHALL support a time-window query by thread and Unix timestamp/tolerance, plus
a retained-event query starting after a receipt sequence. Both APIs SHALL report when the
requested range predates retained data.

#### Scenario: Query around an external sensor timestamp
- **WHEN** a consumer queries thread X around Unix time T with before/after tolerances
- **THEN** the store returns retained events in receipt-sequence order and reports any
  retention gap

### Requirement: Idle eviction from live state
The store SHALL evict a thread's buffer after the thread has been idle beyond the
configured window, and SHALL derive active-thread state from the live stream / reducer
rather than from filesystem metadata.

#### Scenario: Idle thread evicted
- **WHEN** a thread has no activity for longer than the idle window
- **THEN** its buffered events are evicted from the store

#### Scenario: State rebuilt on restart
- **WHEN** the daemon restarts
- **THEN** active-thread state is rebuilt from the live event stream without reading
  persisted store metadata

### Requirement: Reconstructed active-turn anchor at subscribe boundary
The store SHALL record an honest reconstructed anchor when a post-subscribe `thread/read`
reveals an in-flight turn whose wire `turn/started` was not received. The anchor SHALL
carry the daemon's own receipt time, SHALL be marked as reconstructed, and SHALL NOT be
presented as a genuine wire notification.

<!-- live-fix: the store occasionally missed turn/started at the subscribe boundary while the reducer still tracked the active turn. -->

#### Scenario: Turn active before subscribe
- **WHEN** the daemon subscribes to a thread whose turn already started, and the
  follow-up `thread/read` reports an in-flight turn with no wire `turn/started` stored
- **THEN** the store records a reconstructed anchor for that turn with the daemon's
  receipt time and a reconstructed marker

#### Scenario: Reconstructed anchor is distinguishable
- **WHEN** a consumer queries stored events including a reconstructed anchor
- **THEN** the anchor is marked reconstructed and is distinguishable from genuine wire
  events
