## ADDED Requirements

### Requirement: Ensure the standalone install and managed daemon
The system SHALL ensure the standalone Codex install exists under
`~/.codex/packages/standalone/` and that the managed app-server daemon is running before
attempting to attach. If the standalone install is missing, the system SHALL surface a
clear, actionable error rather than silently failing.

#### Scenario: Standalone install missing
- **WHEN** the daemon starts and the standalone Codex install is absent
- **THEN** the supervisor reports that the standalone install is required and does not
  proceed to attach

### Requirement: Conditional daemon ownership
The system SHALL NOT blindly start the managed daemon. It SHALL detect whether a managed
daemon is already running: if none is running, it SHALL start one; if the GUI's
remote-control already owns a running daemon, it SHALL attach to that daemon instead of
starting a competing instance.

#### Scenario: No daemon running
- **WHEN** no managed daemon is running at init
- **THEN** the supervisor starts the managed daemon and attaches to it

#### Scenario: GUI already owns a daemon
- **WHEN** the GUI's remote-control already owns a running managed daemon
- **THEN** the supervisor attaches to the existing daemon and does not start another
  (avoiding the "only one instance" collision)

### Requirement: Version compatibility check
The system SHALL verify that the running managed daemon's version satisfies the GUI's
local-daemon compatibility expectation before relying on daemon mode, and SHALL surface
a clear failure if it does not.

#### Scenario: Incompatible daemon version
- **WHEN** the managed daemon version does not satisfy the GUI's compatibility check
- **THEN** the supervisor reports the incompatibility rather than proceeding as if
  daemon mode will engage

### Requirement: Supervisor-owned daemon-mode GUI launch
The system SHALL own launching a GUI that needs daemon mode. It SHALL launch ChatGPT
directly with `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1` and SHALL NOT rely on a LaunchAgent,
`launchctl setenv`, a Dock launch, or `ipc.sock` to inject or mutate the environment.

<!-- review-fix (G4): a LaunchAgent does not inject env into Dock launches. Pick and specify the ownership model. -->

#### Scenario: GUI is not in daemon mode
- **WHEN** the supervisor determines a GUI relaunch is required
- **THEN** the supervisor launches the GUI process itself with
  `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1`

### Requirement: Detect and preserve an already-correct GUI
The system SHALL detect a GUI already running in daemon mode and leave it running rather
than restarting it.

#### Scenario: GUI already in daemon mode
- **WHEN** a running GUI already has `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1` (detectable
  via `ps eww`) and is connected to the daemon socket
- **THEN** the supervisor leaves it running and does not restart it

### Requirement: Graceful relaunch never force-kills by default
The system SHALL request graceful GUI termination before a daemon-mode relaunch. If the
GUI does not exit within the configured timeout, it SHALL return `UserActionRequired`
and SHALL NOT force-kill the GUI by default.

#### Scenario: GUI refuses graceful quit
- **WHEN** a non-daemon-mode GUI remains running after the graceful-quit timeout
- **THEN** the supervisor reports `UserActionRequired` without killing the process

### Requirement: Ongoing GUI and daemon supervision
The system SHALL continue monitoring after initialization. It SHALL detect GUI or managed
daemon restart, repeat daemon-version and actual shared-socket attachment checks, and
re-enter attach/start/launch/reconciliation as necessary.

#### Scenario: Managed daemon restarts after readiness
- **WHEN** the managed daemon exits and later restarts while the library is running
- **THEN** the supervisor and transport restore compatibility, connection, and ingestion
  reconciliation without requiring host-process restart

#### Scenario: GUI auto-update changes compatibility
- **WHEN** a restarted GUI no longer attaches to the shared socket after preflight
- **THEN** health reports a compatibility failure rather than assuming the version command
  alone proved success

### Requirement: Managed daemon is not stopped on library shutdown
The system SHALL leave the managed daemon running on library shutdown whether it started
or attached to it, because the GUI or another local client may still depend on it.

#### Scenario: Library started the daemon then shuts down
- **WHEN** graceful library shutdown occurs after the supervisor started the managed daemon
- **THEN** subscriptions and this client connection close, but the managed daemon remains
  running

### Requirement: Never rely on the ipc.sock handoff to set the environment
The system SHALL NOT attempt to place a running GUI into daemon mode via the
single-instance `ipc.sock` handoff, because that handoff cannot mutate a running
process's environment; the daemon-mode variable is launch-time-only.

#### Scenario: Handoff cannot flip a running instance
- **WHEN** a GUI is already running without daemon mode and a second launch with the
  variable is attempted
- **THEN** the supervisor does not expect the handoff to change the running instance, and
  instead applies the supervisor-owned graceful relaunch path
