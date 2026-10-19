-- User configuration. LuaLS types come from lua/zmux.lua.
--
-- The cursor shape used while the PTY is focused: "block", "underline", or
-- "bar".
opt.cursor_shape = "block"
-- Shell executable for newly created sessions. If omitted, $SHELL is used.
opt.default_shell = "/usr/bin/fish"
-- The key that starts a key-binding sequence, in Neovim key notation.
opt.prefix_key = "<C-Space>"
-- Time in milliseconds to wait for the next key in a sequence. Set to zero to
-- keep waiting indefinitely.
opt.sequence_timeout_ms = 8000

-- UI anchors reserve rows at the top and bottom of the terminal. Each anchor
-- accepts `height` (rows) and `visible` (boolean).
opt.ui.anchors.top.height = 1
opt.ui.anchors.bottom.height = 1
-- The which-key popup can use at most this many columns. Its default is 7.
opt.ui.which.maxcolumns = 7
-- The which-key popup height can be a percentage string or a row count. The
-- effective value is always clamped to 60% of the terminal height.
opt.ui.which.maxheight = "60%"

-- Bar content may be a string, a segment, a list of segments, or a function
-- returning one of those forms. Functions receive the global `g` state.
---@type ZmuxBarContentProvider
-- Each bar has optional `left`, `center`, and `right` content.
opt.ui.bars.top.left = function(g)
	---@type ZmuxBarSegment[]
	local segments = {}
	for _, tab in ipairs(g.tabs) do
		table.insert(segments, {
			text = string.format(" %d %s ", tab.index, tab.title),
			role = tab.selected and "selected_tab" or "tab",
		})
	end
	return segments
end
opt.ui.bars.bottom.left = function(g)
	if g.last_error then
		return string.format(" Error: %s ", g.last_error)
	end
	if not g.active_tab then
		return " No active tab "
	end
	local status = g.active_tab.running and "running" or "exited"
	return string.format(" %s · %s ", g.active_tab.title, status)
end
opt.ui.bars.bottom.right = function(g)
	return string.format(" Session %s · %dx%d ", g.session.number, g.terminal.columns, g.terminal.rows)
end
-- The default style provider receives the segment and global state. A segment's
-- explicit `style` overrides this provider.
opt.ui.style = function(segment, g)
	if segment.role == "selected_tab" then
		return { fg = "black", bg = "cyan", bold = true }
	end
	if segment.role == "leader" then
		return { fg = "black", bg = "yellow", bold = true }
	end
	return { fg = "gray" }
end

-- Startup hooks run before the Client is connected. `args` contains the raw
-- command-line arguments (excluding the executable), and `launch` holds the
-- mutable launch settings. Interrupt handlers wait for built-in interactions.
--
-- Example: let `zmux new` choose an existing Session before connecting.
-- cmd.event.on("PreClient", function()
-- 	if args[1] ~= "new" or launch.session ~= nil then
-- 		return
-- 	end
-- 	cmd.ui.pick({ "work", "personal" }, function(session)
-- 		if session then
-- 			launch.session = session
-- 			launch.create_if_missing = false
-- 		end
-- 	end)
-- end, { interrupt = true })
--
-- Example: use the built-in Session picker for `zmux list -a`.
-- cmd.event.on("PreClient", function()
-- 	if args[1] == "list" then
-- 		cmd.session.switch()
-- 	end
-- end, { interrupt = true })
--
-- ClientLoaded runs after queued startup commands have settled and before the
-- first normal frame. Commands here are applied before that frame is drawn.
-- cmd.event.on("ClientLoaded", function()
-- 	if launch.command == "new" then
-- 		cmd.tab.create()
-- 	end
-- end)

-- Tab
for index = 1, 9 do
	local tab_index = index
	cmd.key.bind(tostring(index), function()
		cmd.tab.select(tab_index)
	end, {
		desc = "Select tab " .. tab_index,
	})
end
cmd.key.bind("tc", function()
	cmd.tab.create()
end, { desc = "Create a tab" })
cmd.key.bind("tx", function()
	cmd.tab.close()
end, { desc = "Close the focused tab" })
cmd.key.bind("n", function()
	cmd.tab.next()
end, { desc = "Focus the next tab" })
cmd.key.bind("p", function()
	cmd.tab.previous()
end, { desc = "Focus the previous tab" })
cmd.key.bind("tc", function()
	cmd.tab.create()
end, { desc = "Create a tab" })

-- Workspace
cmd.key.bind("<C-h>", function()
	cmd.ui.workspace_drawer.toggle()
end, { desc = "Toggle workspace drawer" })
cmd.key.bind("we", function()
	cmd.workspace.edit()
end, { desc = "Edit the active workspace" })
cmd.key.bind("wn", function()
	cmd.workspace.next()
end, { desc = "Focus the next workspace" })
cmd.key.bind("wp", function()
	cmd.workspace.previous()
end, { desc = "Focus the previous workspace" })
cmd.key.bind("wc", function()
	cmd.workspace.create()
end, { desc = "Create a workspace" })
cmd.key.bind("wx", function()
	cmd.workspace.close()
end, { desc = "Close the active workspace" })

-- Session
cmd.key.bind("d", function()
	cmd.client.detach()
end, { desc = "Detach from the current session" })
cmd.key.bind("sw", function()
	cmd.session.switch()
end, { desc = "Switch active session" })

cmd.key.bind("px", function()
	cmd.server.stop()
end, { desc = "Stop the background process" })
