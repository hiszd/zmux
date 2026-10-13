# Remote access (deferred)

The prototype intentionally supports local PTYs only. Remote access is documented here so it does not accidentally shape the local session model prematurely.

A pane owns a backend boundary. The prototype implements a local PTY backend; a future remote backend could provide the same input, output, and resize behavior without changing terminal emulation or Ratatui rendering.

Possible future transports include SSH or an encrypted browser-sharing protocol. No transport, authentication model, or remote control API is part of the prototype. In particular, remote access must not be added by exposing the local PTY directly to a network socket.
