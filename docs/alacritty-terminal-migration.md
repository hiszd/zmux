# Alacritty Terminal Migration Plan

## Goal

Replace `vt100::Parser` with `alacritty_terminal::Term` while preserving zmux's existing session, PTY, client, and ratatui architecture. The migration should provide complete terminal emulation, terminal-query responses, title events, clipboard plumbing, and Windows-compatible behavior.

Do not leave both terminal emulators active in the final implementation. A temporary adapter or feature flag is acceptable during migration, but `vt100` should be removed after the Alacritty implementation is validated.

## Current architecture

```text
portable-pty::Pane
    -> vt100::Parser
    -> TerminalSnapshot / Client TerminalView
    -> ui::draw
    -> ratatui
```

Relevant files:

- `src/pane.rs`: owns the PTY, child process, parser, shell, output processing, title detection, and resize behavior.
- `src/server.rs`: owns server-side `Pane` instances, broadcasts terminal output/state, detects foreground processes, and creates tabs.
- `src/client.rs`: owns client-side terminal snapshots/parsers, processes server output, and builds the UI view.
- `src/ui.rs`: renders terminal cells, cursor, bars, overlays, and workspace UI.
- `src/protocol.rs`: defines `TerminalSnapshot`, `ClientHello`, and server/client messages.
- `src/main.rs`: CLI and server/client startup.

## Target architecture

```text
portable-pty::Pane
    -> alacritty_terminal::Term<EventProxy>
    -> EventProxy queues terminal responses/events
    -> Pane writes PtyWrite responses to the child PTY
    -> server broadcasts terminal state/output
    -> client renders Alacritty grid cells into ratatui
```

The server should remain authoritative for terminal state and session shell configuration. The client should not maintain an independent terminal parser that can disagree with the server.

## Migration stages

### 1. Add an emulator abstraction

Create an internal abstraction before replacing the parser directly. Suggested shape:

```rust
struct TerminalEmulator {
    term: alacritty_terminal::Term<EventProxy>,
    parser: alacritty_terminal::vte::Parser,
    events: EventProxy,
}

impl TerminalEmulator {
    fn new(rows: u16, columns: u16, events: EventProxy) -> Self;
    fn process(&mut self, bytes: &[u8]);
    fn resize(&mut self, rows: u16, columns: u16);
    fn title(&self) -> Option<&str>;
    fn cursor(&self) -> CursorState;
    fn grid(&self) -> &alacritty_terminal::Grid<alacritty_terminal::term::cell::Cell>;
    fn take_pty_writes(&mut self) -> Vec<Vec<u8>>;
}
```

The exact API may differ by the selected crate version. Keep all Alacritty-specific types inside the emulator module where possible.

### 2. Implement an event proxy

Implement `alacritty_terminal::event::EventListener`.

The proxy must collect at least:

- `Event::PtyWrite`: bytes that must be written back to the child PTY.
- `Event::Title`: application-reported title.
- `Event::ResetTitle`: title reset.
- `Event::ClipboardStore`: clipboard write request.
- `Event::ClipboardLoad`: clipboard read request.
- `Event::Exit` and `Event::ChildExit`: lifecycle events.
- `Event::TextAreaSizeRequest`: respond if dimensions are known.

Do not manually scan PTY output for Primary Device Attribute queries after Alacritty is active. Alacritty should generate the appropriate `PtyWrite` response.

The proxy must be thread-safe if events are produced while server state is being updated. Prefer a local queue drained synchronously by `Pane::process_output` unless the selected API requires another design.

### 3. Replace `Pane`'s parser

In `src/pane.rs`:

- Replace `vt100::Parser` with the Alacritty emulator abstraction.
- Construct `Term` with `alacritty_terminal::term::Config`.
- Set scrollback to the current `10_000` equivalent.
- Feed output through `vte::Parser::advance` into `Term`.
- Drain `PtyWrite` events and write them to the PTY child.
- Preserve `screen()`, `send()`, `resize()`, and `poll_exit()` responsibilities through equivalent methods.
- Remove the manual `terminal_query_buffer` and `contains_primary_device_attribute_query` implementation.
- Preserve process-title fallback behavior, but prefer Alacritty `Event::Title` for application-reported titles.

A likely configuration baseline is:

```rust
alacritty_terminal::term::Config {
    scrolling_history: 10_000,
    osc52: Osc52::OnlyCopy, // Revisit when clipboard support is implemented.
    ..Default::default()
}
```

### 4. Establish title precedence

Use one title source and document the precedence clearly:

1. Application-reported title from `Event::Title`.
2. Foreground process name (`fish`, `nvim`, etc.) when no explicit title is active.
3. Stable fallback `Terminal <tab_id>`.

Do not independently derive the displayed title from:

- the server snapshot,
- the client parser,
- the process name,
- and the OSC title simultaneously.

The server should store the effective title in `ServerTab`. The client should render the title received in the server snapshot.

When the application emits `ResetTitle`, revert to the foreground process name or fallback.

### 5. Replace terminal snapshots

`TerminalSnapshot` currently contains formatted VT100 bytes. Decide whether to retain that wire format temporarily or replace it.

Recommended transitional approach:

- Keep `TerminalSnapshot.bytes` as a serialized terminal replay format only if Alacritty can consume the same output stream deterministically.
- Prefer storing/broadcasting the original PTY output stream plus dimensions and title.
- On a new client connection, send a replay buffer or a full grid snapshot rather than relying on `vt100::Screen::contents_formatted()`.

Do not send Alacritty's internal `Grid` directly over the protocol unless the wire format is deliberately designed and versioned.

Bump `PROTOCOL_VERSION` when changing snapshot semantics.

### 6. Replace client-side parsing

In `src/client.rs`:

- Remove `TerminalView.parser: vt100::Parser`.
- Store the Alacritty emulator or a server-provided renderable terminal snapshot.
- Ensure output is parsed exactly once when possible.
- Do not parse the same PTY output independently on both server and client unless required by the protocol.
- Keep `TerminalView.title` and `exit_code` as client display state only if the server remains authoritative.

The preferred end state is server-side terminal emulation with the client receiving enough state to render. If performance or protocol size makes that undesirable, use an explicitly shared replay model and test it carefully.

### 7. Replace the ratatui renderer

In `src/ui.rs`:

- Keep the immediate-mode renderer introduced in the previous simplification.
- Replace `vt100::Cell` access with Alacritty grid/renderable-content access.
- Map Alacritty cell flags to ratatui modifiers:
  - bold
  - italic
  - underline
  - inverse
  - hidden
  - crossed-out
- Map named, indexed, and RGB colors.
- Correctly handle wide cells and spacer cells.
- Use Alacritty cursor state and cursor style where possible.
- Continue letting ratatui perform the final terminal output diff.
- Do not reintroduce a custom per-cell invalidation cache without profiling.

Keep `needs_draw` in the client event loop as the only application-level redraw scheduling mechanism.

### 8. Clipboard support

Add a client-side clipboard service rather than putting clipboard contents in session snapshots.

Recommended flow:

```text
OSC 52 copy from application
    -> Alacritty Event::ClipboardStore
    -> client clipboard backend

Application requests paste
    -> Alacritty Event::ClipboardLoad
    -> client clipboard backend
    -> Event::PtyWrite response
```

Use a small abstraction such as:

```rust
trait ClipboardProvider {
    fn read(&self) -> Result<String, ClipboardError>;
    fn write(&self, text: &str) -> Result<(), ClipboardError>;
}
```

Treat clipboard loading as security-sensitive. Start with copy-only behavior if necessary, matching Alacritty's safer default, and add paste only with an explicit configuration option.

### 9. Windows requirements

Do not use Unix-only assumptions in the emulator migration.

Keep these behind platform-specific modules:

- `/proc` foreground process detection.
- `tcgetpgrp` process-group inspection.
- Unix shell login flags.
- Unix raw terminal setup.
- Unix PTY-specific behavior.

Windows shell handling should support at least:

- `cmd.exe`
- PowerShell
- `pwsh.exe`

Do not blindly append `-il` on Windows.

Continue using `portable-pty` for PTY creation unless Alacritty's TTY layer is intentionally adopted. Do not mix two competing PTY ownership models.

Verify that `alacritty_terminal` is compiled with the required Windows features and that the core `Term`/grid/parser APIs are available on Windows.

### 10. Testing plan

Add tests before removing `vt100`.

#### Emulator unit tests

- Primary Device Attribute query produces a PTY response.
- Device status query produces a PTY response.
- OSC title produces `Event::Title`.
- Title reset falls back correctly.
- Alternate screen enter/exit works.
- Cursor visibility and position are correct.
- Resize preserves valid dimensions.
- Wide characters occupy the correct cells.
- Combining marks do not consume additional columns.
- Indexed/RGB/default colors map correctly.

#### Shell integration tests

- Fish starts with the forwarded environment.
- Fish reads a temporary `config.fish`.
- Fish does not emit the ten-second device-attribute warning.
- `nvim` or a fake interactive application can enter alternate screen.
- Shell title changes when the foreground application changes.

#### Clipboard tests

- OSC 52 copy reaches the clipboard provider.
- Clipboard read generates a PTY response.
- Disabled clipboard configuration does not write/read data.

#### Protocol tests

- Snapshot replay is deterministic.
- Protocol version mismatch is rejected.
- New fields serialize and deserialize on all supported platforms.

#### Windows checks

At minimum, run:

```sh
cargo check --target x86_64-pc-windows-msvc
```

or the project's configured Windows target in CI. Do not rely on Linux-only tests for the terminal core.

### 11. Remove old implementation

Only after all emulator and integration tests pass:

- Remove `vt100` from `Cargo.toml`.
- Remove all `vt100::` imports.
- Remove manual terminal-query response code.
- Remove old VT100 snapshot formatting.
- Remove parser-specific tests.
- Update documentation and protocol version.
- Run the full validation loop:

```sh
cargo check
cargo test
cargo clippy --no-deps
cargo fmt
```

## Suggested commit sequence

1. `add terminal emulator abstraction`
2. `add alacritty terminal backend and event proxy`
3. `route terminal responses through the pty`
4. `migrate server terminal state and titles`
5. `migrate client terminal state`
6. `migrate ratatui grid rendering`
7. `add clipboard provider`
8. `add windows-specific terminal handling`
9. `add terminal compatibility integration tests`
10. `remove vt100 backend`

Each commit should compile and pass the relevant tests. Do not remove `vt100` until the Alacritty implementation has passed equivalent rendering, title, resize, and PTY-response tests.
