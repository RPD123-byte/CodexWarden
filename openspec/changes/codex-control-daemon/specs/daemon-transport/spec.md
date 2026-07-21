## ADDED Requirements

### Requirement: Connect over the managed daemon socket
The system SHALL connect to the managed app-server daemon by opening the local unix
socket at `~/.codex/app-server-control/app-server-control.sock` and performing a
WebSocket handshake for the `/rpc` path. Raw newline-delimited JSON SHALL NOT be used,
because the socket speaks the WebSocket framing.

#### Scenario: Successful connection and handshake
- **WHEN** the managed daemon is running and the daemon opens the unix socket and
  performs the WebSocket handshake to `/rpc`
- **THEN** the connection is established and JSON-RPC messages can be exchanged as
  WebSocket text frames

#### Scenario: Socket absent
- **WHEN** the daemon socket does not exist (no managed daemon running)
- **THEN** the transport reports a connection error rather than hanging indefinitely

### Requirement: Initialize handshake
After the transport connects, the system SHALL send an `initialize` request with client
metadata and then emit an `initialized` notification before issuing any other request.

#### Scenario: Handshake precedes other requests
- **WHEN** the transport connects
- **THEN** the first message sent is `initialize`, followed by `initialized`, before any
  `thread/*` or `turn/*` request is sent

### Requirement: Reconnect with backoff and reconciliation
The system SHALL detect a dropped connection and reconnect using exponential backoff.
On successful reconnect it SHALL repeat the initialize handshake and invoke ingestion
reconciliation before accepting normal action traffic. Reconciliation SHALL discover both
previously tracked threads and threads activated while the connection was down.

#### Scenario: Daemon restarts
- **WHEN** the managed daemon restarts and the WebSocket drops
- **THEN** the transport reconnects with backoff, re-runs the initialize handshake, and
  completes reconcile/resubscribe before reopening the action gate

#### Scenario: Backoff bounds reconnect attempts
- **WHEN** the daemon is unreachable across multiple attempts
- **THEN** the delay between attempts increases up to a bounded maximum rather than
  busy-looping

### Requirement: Request/response correlation
The system SHALL correlate JSON-RPC responses to their originating requests by id, and
SHALL allocate ids unique among this connection's in-flight requests. A configured high
range MAY aid logging, but SHALL NOT be treated as a cross-client causation guarantee.

#### Scenario: Concurrent requests resolve correctly
- **WHEN** multiple requests are in flight and their responses arrive out of order
- **THEN** each response is delivered to the caller that issued the matching request id

### Requirement: Request write-state classification
The transport SHALL track whether each outbound request was not written, written but
unanswered, or answered. When the connection fails, it SHALL resolve every pending call
with `NotWritten` or `WrittenOutcomeUnknown` as appropriate rather than leaving it
pending across reconnect.

#### Scenario: Disconnect before write
- **WHEN** a request is queued but the connection fails before its frame is written
- **THEN** the caller receives `NotWritten`, allowing the action policy to retry safely

#### Scenario: Disconnect after write
- **WHEN** a request frame was written but no response arrived before disconnect
- **THEN** the caller receives `WrittenOutcomeUnknown`, and the transport does not resend
  it automatically

### Requirement: Half-open connection detection
The transport SHALL use WebSocket ping/pong or a bounded read-idle health check to detect
a half-open connection after daemon failure or laptop sleep.

#### Scenario: Socket remains open without progress
- **WHEN** the connection produces neither frames nor a valid health response beyond the
  configured health timeout
- **THEN** the transport treats it as dropped and starts bounded reconnect
