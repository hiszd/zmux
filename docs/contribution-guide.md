# Codebase Modification & Contribution Guide

Use this guide to find the code paths for common changes in zmux. Paths below are repository-relative.

## 1. Task-Based Change Recipes

### Add a CLI command or client operation

1. In `src/main.rs`, add a `Command` variant and parse its spelling and arguments in `parse_args`. Update `print_help` so the command is discoverable.
2. Choose the execution path:
   - For a one-shot administrative operation, map the command to an `AdminRequest` in `run_admin_command`. That function connects to the server and writes a framed message.
   - For a TUI operation, send a `ClientRequest` from the client event loop.
   - For a scriptable server operation, use a `HeadlessRequest`; see `src/headless.rs`.
3. Add or extend the corresponding serializable message type in `src/protocol/framing.rs`. The shared types live there—not in a `common/ipc.rs` or `src/protocol.rs` implementation file. Derive the serde traits used by the existing messages.
4. Implement the operation on the receiving side in `src/server/event_loop.rs`:
   - Administrative requests: `ServerState::handle_admin`.
   - TUI requests: `ServerState::handle_request`.
   - Headless requests: `ServerState::handle_headless_request`.
   Return the appropriate response and broadcast changed state when needed.
5. Use `write_frame` and `read_frame` from `src/protocol/framing.rs` for IPC. They encode with bincode’s legacy configuration and add a length prefix. Do not substitute JSON for IPC.
6. Add tests beside the changed code. Protocol round-trip and legacy-compatibility tests are in `src/protocol/framing.rs`; server behavior tests are in `src/server/event_loop.rs`.

For a new CLI-only output format, `serde_json` is used for keybind output in `src/main.rs`; that is separate from the bincode client/server protocol.

### Modify or add a UI widget or screen

1. For the attached client’s event-driven rendering, start in `src/client/event_loop.rs`. `run_event_loop` decides when to draw and builds the current render view.
2. Change drawing and layout in `src/ui/render.rs`:
   - `layout` computes the top bar, bottom bar, panel, PTY rectangle, and pane.
   - `draw` renders the layers and terminal cursor.
   - Add or adapt a widget there, or add supporting UI-tree behavior in `src/ui/tree.rs`.
3. To inspect the virtual terminal, follow the active client terminal state (`TerminalView` in `src/client/event_loop.rs`) to `TerminalEmulator::grid()` in `src/terminal_emulator/emulator.rs`. The grid is an Alacritty `Grid<Cell>`. `render_screen` in `src/ui/render.rs` shows how cells are projected into Ratatui’s buffer.
4. If layout changes affect the PTY’s usable area, also check `ui::pane_size` in `src/ui/render.rs` and the client’s resize/request handling in `src/client/event_loop.rs`. The server applies PTY sizing in `src/server/event_loop.rs`.
5. Add or update rendering tests in `src/ui/render.rs`; client state and event tests live inline in `src/client/event_loop.rs`.

Lua-configured UI elements are registered and managed through `src/lua/config.rs` and `src/ui/tree.rs`; the default user-facing Lua helpers are in `lua/defaults.lua` and `lua/zmux.lua`.

### Add a Lua API binding

There is no Lua runtime embedded in the server daemon. There are two client-side Lua entry points; decide which one the API belongs to:

- **Interactive configuration/runtime API:** `src/lua/config.rs` creates the Lua state in `load_config_with_source_path` and registers the `cmd` namespaces and other bindings. Add a Rust binding with `Lua::create_function` (or the appropriate mlua API), register it under the relevant table, and connect it to the existing command queue or UI/config state. Runtime commands are dispatched from `src/client/event_loop.rs`.
- **Headless `zmux run` API:** `src/headless.rs` creates a separate Lua state and registers script commands in `register_commands`. Those commands are parsed into `HeadlessRequest`s, sent over IPC, and handled by the server in `src/server/event_loop.rs`.

For either API, update Lua-facing defaults or helpers where appropriate (`lua/defaults.lua` or `lua/zmux.lua`) and add tests beside the registration/evaluation logic. Interactive config tests are in `src/lua/config.rs`; headless script tests are in `src/headless.rs`.

### Handle a TUI keyboard or mouse event

1. Crossterm events are read by the client input worker in `src/client/event_loop.rs` and sent into the client event channel.
2. Add handling in the client’s event processing path (`handle_input` in `src/client/event_loop.rs`). Respect existing key dispatch, focused-UI capture, modal behavior, paste handling, and redraw effects.
3. Decide whether the event is for zmux UI state or for the focused PTY:
   - For zmux UI behavior, update client state and request a redraw as needed.
   - For PTY input, convert it to the correct terminal byte sequence and send `ClientRequest::Input`.
4. The server routes `ClientRequest::Input` through `ServerState::handle_request` and `send_input` in `src/server/event_loop.rs`, then writes the bytes to the active pane.
5. If adding mouse passthrough, account for the active terminal’s mouse-reporting mode and encode the event accordingly; a Crossterm mouse event is not itself a PTY input byte sequence.
6. Add input tests in `src/client/event_loop.rs`. Existing key and paste forwarding tests are there; add mouse-specific coverage for new behavior.

### Modify the IPC protocol

1. Edit the shared message types in `src/protocol/framing.rs`: `ClientMessage`, `ServerMessage`, `ClientRequest`, `AdminRequest`, `HeadlessRequest`, and related payload structs.
2. Update both sender and receiver paths:
   - Client-side construction and response handling: `src/main.rs`, `src/client/event_loop.rs`, or `src/headless.rs`, depending on the operation.
   - Server-side dispatch and response/broadcast: `src/server/event_loop.rs`.
3. Keep using `read_frame` and `write_frame` in `src/protocol/framing.rs`. Current IPC is bincode, not serde_json.
4. Update protocol tests in `src/protocol/framing.rs`, including legacy bincode coverage. `Cargo.toml` defines `bincode_v1` as a development dependency for compatibility tests.

### Adjust PTY or terminal-emulation behavior

1. PTY creation, child process setup, input/output, resize, and lifecycle are in `src/pane/process.rs` (`Pane`), using `portable-pty`.
2. PTY reader threads send output as `AppEvent::PtyOutput`; event definitions are in `src/events/model.rs`.
3. The server receives those events through its event loop in `src/server/event_loop.rs`. `handle_pty_output` sends bytes to `Pane::process_output`, updates terminal-derived state, and forwards output to connected clients.
4. Terminal parsing is in `src/terminal_emulator/emulator.rs` (`TerminalEmulator::process`). It uses Alacritty’s VTE processor and terminal grid; terminal events can include title and clipboard handling.
5. If changing process creation or PTY behavior, review `shell_command_builder`, `Pane::send`, `Pane::resize`, and `Drop` in `src/pane/process.rs`. If changing parsing or emulation, add tests in `src/terminal_emulator/emulator.rs`.
6. Replay and snapshot behavior spans `Pane::process_output` in `src/pane/process.rs` and `TerminalEmulator::snapshot_replay` in `src/terminal_emulator/emulator.rs`. Check both when changing how terminal state is reconstructed for clients.

## 2. “Where Do I Put This?” Quick Reference

| Objective / Action | Location to Modify | Corresponding Test Location |
|---|---|---|
| Client UI drawing and layout | `src/ui/render.rs`; client render loop in `src/client/event_loop.rs`; UI tree in `src/ui/tree.rs` | `src/ui/render.rs` and client tests in `src/client/event_loop.rs` |
| Crossterm keys, paste, and events | Input worker and `handle_input` in `src/client/event_loop.rs`; key mapping in `src/input/` | Input/dispatch tests in `src/client/event_loop.rs` and `src/input/` |
| Server state and session loop | `src/server/event_loop.rs`; domain types in `src/domain/model.rs` | Inline server state/transition tests in `src/server/event_loop.rs` |
| IPC messages and framing | `src/protocol/framing.rs`; sender/receiver logic in the client, CLI, headless client, and server | Serialization and bincode-v1 compatibility tests in `src/protocol/framing.rs` |
| Interactive Lua configuration API | `src/lua/config.rs`; default helpers in `lua/defaults.lua` and `lua/zmux.lua` | Lua config tests in `src/lua/config.rs` |
| Headless Lua script API | `src/headless.rs`; corresponding server operation in `src/server/event_loop.rs` | Headless tests in `src/headless.rs` and server tests in `src/server/event_loop.rs` |
| PTY creation, I/O, and resizing | `src/pane/process.rs`; event handling in `src/server/event_loop.rs` | Pane tests in `src/pane/process.rs`; server PTY behavior tests in `src/server/event_loop.rs` |
| Terminal parsing and grid behavior | `src/terminal_emulator/emulator.rs`; rendering projection in `src/ui/render.rs` | Emulator tests in `src/terminal_emulator/emulator.rs`; grid-render tests in `src/ui/render.rs` |
| CLI parsing and output | `src/main.rs` | `tests/config_cli.rs` and inline tests in `src/main.rs` |

Most tests are inline with their implementation files. The repository has `tests/config_cli.rs`; it does not currently have a dedicated visual-test or PTY-integration-test directory.

## 3. Gotchas, Side Effects, and Guardrails

- **Restore the user’s terminal.** `src/client/event_loop.rs` has a `TerminalModeGuard` whose `Drop` restores raw mode, the alternate screen, cursor state, bracketed paste, and keyboard-enhancement flags. Preserve this guard around terminal-mode setup and cleanup. Rust unwinding normally drops it; aborting panics or process termination do not. If changing panic handling, verify cleanup behavior rather than relying on a panic hook alone.
- **The server owns live sessions and PTYs.** Client disconnection removes that client; it should not kill the server’s background PTY work. By contrast, stopping or crashing the server destroys its in-memory sessions and drops the panes, whose `Drop` kills the child process. Sessions are ephemeral, not persisted across server restarts.
- **Keep protocol peers compatible.** The protocol version is `PROTOCOL_VERSION` in `src/protocol/framing.rs` and is checked during TUI and headless hello handshakes. Bincode serializes the shared message shapes; changing enum variants or payload layouts can make mismatched client/server binaries fail to decode messages. Update the protocol version when compatibility is broken, and do not assume the version check protects every message path (administrative requests do not carry a hello handshake).
- **Treat terminal dimensions as cells, not bytes or scalar values.** `src/ui/render.rs` uses `unicode-width` and `unicode-segmentation` for display text. Wide characters occupy multiple cells; combining sequences may contain multiple scalar values but occupy fewer display cells. When slicing, clipping, or placing text, avoid byte offsets and account for Alacritty’s wide-character spacer cells.
- **Do not print into the active TUI.** Avoid `println!` and `eprintln!` in interactive paths: output can corrupt the rendered screen. Use the existing `Trace`/`tracing` instrumentation instead. To enable trace output, set `ZMUX_TRACE=1`; by default it goes to `zmux-trace.log`, or set `ZMUX_TRACE_LOG` to choose a path. The background server is started with the parent process environment, so inspect that trace log when debugging daemon behavior.
- **Distinguish transport from serialization.** `interprocess` local sockets/named pipes are managed in `src/transport/`; message framing and bincode serialization are in `src/protocol/framing.rs`. JSON in the CLI is output serialization, not the IPC wire format.
