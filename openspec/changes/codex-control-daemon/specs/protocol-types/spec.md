## ADDED Requirements

### Requirement: Generated protocol types
The system SHALL provide a `serde`-only Rust representation of the Codex app-server
JSON-RPC protocol, generated from the Codex JSON Schema bundle rather than hand-written
or vendor-copied from codex-rs source. The generated types SHALL cover, at minimum, the
client requests the daemon sends and the server notifications and server→client requests
it consumes.

#### Scenario: Deserialize a server notification
- **WHEN** a JSON line captured from the managed daemon (e.g. a `turn/started`
  notification) is deserialized into the generated notification type
- **THEN** deserialization succeeds and exposes the `threadId` and `turn.id` fields with
  the same values present in the raw JSON

#### Scenario: Serialize a client request
- **WHEN** the daemon constructs a `turn/interrupt` request from the generated request
  type with a given `threadId` and `turnId`
- **THEN** the serialized JSON matches the wire shape the managed daemon accepts

### Requirement: Protocol version pinning
The generated protocol types SHALL be pinned to a specific Codex protocol version, and
the pinned version SHALL be recorded in the repository alongside the committed schema
snapshot used to generate them.

#### Scenario: Pinned version is discoverable
- **WHEN** a developer inspects the protocol crate
- **THEN** the pinned Codex version/commit and the source schema snapshot are recorded
  and the regeneration command is documented

### Requirement: Deterministic schema regeneration and latest drift alarm
The system SHALL make normal CI reproducible by regenerating JSON Schema from the exact
pinned Codex version/commit and comparing it with the committed snapshot. A separate
scheduled or manually triggered job SHALL obtain the latest supported Codex version and
report upstream drift without making unrelated normal CI depend on whichever live version
happens to be installed on a runner.

#### Scenario: Pinned regeneration differs
- **WHEN** normal CI regenerates from the recorded pinned version and the result differs
- **THEN** CI fails because the committed snapshot is not reproducible

#### Scenario: Latest supported version changed
- **WHEN** the scheduled latest-version job finds schema differences from the pin
- **THEN** it raises a drift report identifying the upstream version and diff, without
  introducing runner-version nondeterminism into normal CI

### Requirement: Real-traffic fidelity check
The system SHALL validate its wire envelope against a sanitized, representative fixture
of inner protocol messages derived from real captured traffic and owned by the protocol
crate. Any message that fails to deserialize SHALL fail the check. The Rust workspace
SHALL NOT read fixtures or captures from `experimentation/`, and the fidelity fixture
SHALL NOT be treated as managed-daemon behavioral replay.

<!-- review-fix (G7): the corpus is stdio, wrapped; use the inner msg for type fidelity only. -->

#### Scenario: Replay the sanitized fidelity fixture
- **WHEN** the fidelity test deserializes each crate-owned representative inner message
- **THEN** every message deserializes without error

### Requirement: Unknown-method resilience
The reader SHALL NOT fail closed on an unrecognized method-bearing server message,
whether notification or server-initiated request. The protocol representation SHALL use
a known-message envelope with a raw fallback so a new method is captured, sequenced, and
logged rather than breaking the reader before CI drift detection runs. An unknown
server-initiated request SHALL still obey the never-respond invariant.

<!-- review-fix (G5): exhaustive generated enums break at runtime on a new upstream variant, before CI catches drift. -->

#### Scenario: New upstream notification method
- **WHEN** the daemon sends a notification whose method is not in the generated known set
- **THEN** the reader parses it as a raw/unknown event and logs it, without erroring or
  dropping the connection

#### Scenario: New upstream server-request method
- **WHEN** the daemon sends an id-bearing request whose method is not in the known set
- **THEN** the reader records it through the raw fallback, sends no response, and keeps the
  connection alive

#### Scenario: Known methods still typed
- **WHEN** the daemon sends a recognized notification
- **THEN** it is deserialized into its specific generated type, not the raw fallback
