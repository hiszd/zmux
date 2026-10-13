# tmux Session Management: Primary-Source Notes

Research date: 2026-10-12

## Summary

In tmux, a **session is a server-owned workspace**, not the tmux process or a single shell. It contains one or more window links; each window contains one or more panes, and each pane owns a terminal/PTY. Clients attach to sessions and can detach or switch sessions without destroying the sessions. A server can manage multiple sessions and exits by default when no sessions remain.

## Findings

### Session hierarchy and ownership

- The tmux server manages clients, sessions, windows, and panes. A session is a collection of pseudo-terminals and has one or more windows linked into it.
- A window can be linked into more than one session. Panes belong to the window, so linked sessions can expose the same window/pane processes.
- Session state includes a current window and a last-window stack; selecting windows changes the session's current window.
- Clients attach to sessions. Multiple clients may attach to the same session, and switching a client to another session is distinct from creating or destroying a session.

Sources: [tmux manual — DESCRIPTION](https://man.openbsd.org/tmux.1), [tmux manual — CLIENTS AND SESSIONS](https://man.openbsd.org/tmux.1), [session.c](https://github.com/tmux/tmux/blob/master/session.c).

### Creation, attachment, naming, and lifetime

- Starting tmux without a command creates a session with one window and attaches it to the current terminal.
- `new-session` creates a session; it is attached by default and detached with `-d`. It can take a session name, initial window name, starting directory, environment entries, and an initial shell command.
- Names are unique among live sessions on a tmux server. If no explicit name is given, tmux generates one. Session IDs are distinct from names and are unique for the lifetime of that server.
- `new-session -A` means “attach if the named session already exists, otherwise create it.” `attach-session` requires an existing session.
- Detaching a client does not destroy its session. Destroying a session detaches its clients and unlinks its windows; windows linked to another session can remain alive there.
- By default, the server exits when there are no sessions (`exit-empty` is on). This is server lifetime behavior, not persistence across a server restart.

Sources: [tmux manual — DESCRIPTION and CLIENTS AND SESSIONS](https://man.openbsd.org/tmux.1), [new-session implementation](https://github.com/tmux/tmux/blob/master/cmd-new-session.c), [attach-session implementation](https://github.com/tmux/tmux/blob/master/cmd-attach-session.c), [kill-session implementation](https://github.com/tmux/tmux/blob/master/cmd-kill-session.c), [session.c](https://github.com/tmux/tmux/blob/master/session.c), [exit-empty option definition](https://github.com/tmux/tmux/blob/master/options-table.c).

### Session selection and groups

- `switch-client` changes the session shown by a particular client; it supports next, previous, and last-session selection as well as explicit targets.
- Session names and IDs are targetable. The manual describes name matching (exact, prefix, and glob matching) and stable `$ID` targets.
- Session groups are an advanced feature. Sessions in a group share the same set of window links, while each session keeps its own current/previous window and session options. Creating/linking/unlinking windows is synchronized across the group.

Sources: [tmux manual — target-session and CLIENTS AND SESSIONS](https://man.openbsd.org/tmux.1), [switch-client implementation](https://github.com/tmux/tmux/blob/master/cmd-switch-client.c), [new-session implementation](https://github.com/tmux/tmux/blob/master/cmd-new-session.c), [session group implementation](https://github.com/tmux/tmux/blob/master/session.c).

### Client and window sizing

- A tmux pane has one PTY size shared by its attached clients; clients do not get separate shell geometries for the same pane.
- The session `window-size` option controls window sizing. Its default is `latest`, which uses the size of the client with the most recent activity. `largest`, `smallest`, and `manual` are also available.
- The `ignore-size` client flag excludes a client from affecting other clients' sizes; `attach -r` enables this flag along with read-only mode.
- `aggressive-resize` changes which sessions are considered when choosing the size of a window: sessions for which that window is current, rather than just the session to which the client is attached. This matters because tmux windows can be linked into multiple sessions.
- A resize changes the shared terminal geometry, so applications in the affected panes respond to the new PTY size; the clients still render into their own physical terminals.

Sources: [tmux manual — `window-size`](https://man.openbsd.org/tmux.1#window-size), [`aggressive-resize`](https://man.openbsd.org/tmux.1#aggressive-resize), [`attach-session` client flags](https://man.openbsd.org/tmux.1#attach-session).

## Agreed zmux terminology and mapping

zmux intentionally uses a different level of detail from tmux:

1. A **Session** is the named top-level collection and may contain one or more **Workspaces**.
2. A **Workspace** contains Tabs and a working directory. Its optional explicit name defaults to the working directory's basename. The Workspace directory is the starting directory for new Tabs and changes only through Workspace UI; a Tab shell changing its own current directory does not change the Workspace directory. If a Workspace uses a derived name, changing its directory in the UI updates that name to the new basename.
3. A **Tab** is exactly one PTY-backed terminal, rather than a tmux-style window that can contain several panes.
4. A **Client** is a TUI attached to a Session. Multiple clients receive Rust-owned state updates from the server; active Workspace and Tab selections are shared Session state. Closing an active Tab selects the next Tab in order, or the previous one when the removed Tab was last. Closing an active Workspace applies the same rule to Workspaces.
5. Sessions are automatically assigned sequential numbers rather than user-supplied names. Launching without a target creates a new Session; specifying an existing Session Number attaches to it. Session Numbers identify Sessions to users; internal Session IDs may remain separate runtime identities.
6. Detach preserves the server and sessions. Destroying a session closes its tabs and selects another session for all attached clients, or closes clients if none remain. Stopping the server destroys every session. Sessions are ephemeral and do not survive server restart.
7. Next-session selection uses creation order. Session groups and shared tabs are deferred.

The session working-directory rule follows tmux's general precedent that a session working directory is used when creating new windows, while a running pane can have its own working directory. zmux applies that default at the Workspace level because Workspaces are its tab grouping. Workspace creation/editing will use a directory-selection dialog and an optional name field; creating a Workspace starts one Tab in the selected directory and activates it. The generic Fuzzy Picker remains only a reusable list-selection primitive that returns an item or cancellation through its completion callback.

The adopted zmux client-sizing policy is recorded separately in [ADR 0002](../adr/0002-shared-pty-sizing.md): one shared PTY geometry per Session using the maximum attached-client rows and columns independently, with per-client viewport projection.
