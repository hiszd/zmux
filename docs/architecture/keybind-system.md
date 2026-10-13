# Keybind System

Zmux key bindings are single-context sequences entered after a required prefix key. Bindings are Lua callbacks; there are no mode arguments or action-name strings.

```lua
opt.prefix_key = "<C-Space>"
opt.sequence_timeout_ms = 700

cmd.bind("tw", function()
    cmd.close_tab()
end, { desc = "Close the focused tab" })

-- Remove a binding
cmd.bind("tw", nil)
```

## Key notation

Literal printable characters are individual keys in a sequence (`"tw"` is `t`, then `w`). Named and modified keys use Neovim notation, for example `<C-c>`, `<M-x>`, `<S-Up>`, `<CR>`, `<Esc>`, `<Space>`, `<Tab>`, `<BS>`, `<Del>`, `<Home>`, `<PageUp>`, and `<F1>`. Modifier keys may be combined, such as `<C-S-Up>`. Plain key case is meaningful (`"t"` differs from `"T"`).

`opt.prefix_key` uses the same notation but must identify exactly one key. There are no separate mapping modes, buffer-local bindings, expression mappings, command mappings, or `<Leader>` alias; the configured prefix is always required before a binding sequence.

## Sequences and ambiguous bindings

Bindings are stored in a trie. A sequence can be both a complete binding and a prefix of a longer binding. In that case zmux waits up to `opt.sequence_timeout_ms` for another key. If a longer binding completes, its callback runs; if the timeout expires or the next key no longer matches, the shorter binding runs. A prefix with no complete binding is cancelled on timeout. The default sequence timeout is 700 ms; setting it to `0` disables automatic leader/sequence timeouts. With timeouts disabled, completing a binding still exits leader mode. Escape or an unbound key also leaves leader mode; an unbound key additionally displays the yellow warning.

After the prefix and after each sequence key, zmux shows a small which-key overlay when continuations exist. It is anchored to the bottom of the Client view and spans its full width. Entries show the next key and the binding description where available. Escape cancels the current sequence and is reserved: `cmd.bind` rejects any sequence containing `<Esc>`. An unmatched key ends the sequence and produces a short yellow warning at the bottom right.

Key callbacks run without arguments. They can request application operations through `cmd`; callback errors are caught and displayed in the same yellow warning area. The `inf` query API for reading tabs, Workspaces, and Session data is a follow-up API-design task and is not part of the keybind infrastructure change.

## Configuration API

- `cmd.bind(keys, callback, { desc = string })` adds/replaces a binding during configuration loading.
- `cmd.bind(keys, nil)` removes the binding.
- `desc` is optional and is used by the which-key overlay.
- Runtime commands called by a binding callback are queued and applied after that callback returns, just like commands requested by registered event callbacks.
- `opt.sequence_timeout_ms` defaults to `700`; `opt.prefix_key` defaults to `<C-Space>`.
