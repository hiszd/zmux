-- Built-in configuration. LuaLS types come from lua/zmux.lua.
--
opt.config_version = 3
-- The cursor shape used while the PTY is focused: "block", "underline", or
-- "bar".
opt.cursor_shape = "block"
-- Watch this config file for changes while attached; disabled by default.
opt.live_reload = false
-- Shell executable for newly created sessions. If omitted, $SHELL is used.
opt.default_shell = "/usr/bin/fish"
-- The key that starts a key-binding sequence, in Neovim key notation.
opt.prefix_key = "<C-Space>"
-- Time in milliseconds to wait for the next key in a sequence. Set to zero to
-- keep waiting indefinitely.
opt.sequence_timeout_ms = 700

-- Style the PTY border with any Ratatui preset, or set it to "none" to use
-- the full panel for terminal cells. border_style accepts a ZmuxStyle table.
opt.ui.pty.border = "plain"
-- opt.ui.pty.border = "none"
-- opt.ui.pty.border = "rounded"
-- opt.ui.pty.border_style = { fg = "cyan", bold = true }
-- The which-key popup can use at most this many columns. Its default is 7.
opt.ui.which.maxcolumns = 7
-- The which-key popup height can be a percentage string or a row count. The
-- effective value is always clamped to 60% of the terminal height.
opt.ui.which.maxheight = "60%"

-- Bars are ordinary retained UI Elements. Their rows reserve space only while
-- visible, and their child Layouts can be changed in event callbacks.
local ui = cmd.ui
local style = cmd.style
local tabs = ui.row({})
local leader_text = ui.text({
	text = "",
	style = style.ref("Bar.LeaderIndicator"),
})
local top_content = ui.slices({
	slices = 3,
	items = {
		{ index = 1, slice_width = 2, node = tabs },
		{ index = 3, slice_width = 1, node = ui.align({ content = leader_text, horizontal = "right" }) },
	},
})
local status_text = ui.text({ text = "" })
local session_label = ui.text({ text = "" })
local session_text = ui.align({ content = session_label, horizontal = "right" })
local bottom_content = ui.slices({
	slices = 3,
	items = {
		{ index = 1, slice_width = 2, node = status_text },
		{ index = 3, slice_width = 1, node = session_text },
	},
})
local bars = {
	top = cmd.ui.create({
		type = "bar",
		position = "top",
		content = top_content,
		style = style.ref("Bar"),
		visible = true,
	}),
	bottom = cmd.ui.create({
		type = "bar",
		position = "bottom",
		content = bottom_content,
		style = style.ref("Bar.Status"),
		visible = true,
	}),
}

local tab_nodes = {}
-- Update the Tab row only on events that can change its contents or order.
local function update_tabs()
	local tab_items = {}
	local active_tabs = {}
	for _, tab in ipairs(g.tabs) do
		local node = tab_nodes[tab.id]
		if not node then
			node = ui.text({ text = "" })
			tab_nodes[tab.id] = node
		end
		table.insert(tab_items, node)
		active_tabs[tab.id] = true
		node.set("text", string.format(" %d %s ", tab.index, tab.title))
		node.set("style", style.ref(tab.selected and "Bar.TabActive" or "Bar.Tab"))
	end
	for id in pairs(tab_nodes) do
		if not active_tabs[id] then
			tab_nodes[id] = nil
		end
	end
	tabs.set("children", tab_items)
end
cmd.event.on({
	"ClientLoaded",
	"SessionCreated", "SessionSelected", "SessionClosed",
	"WorkspaceCreated", "WorkspaceClosed", "WorkspaceSelected",
	"TabCreated", "TabClosed", "TabSelected", "TerminalTitleChanged",
}, update_tabs)

-- The leader indicator and top Bar background depend only on leader mode.
local function update_leader()
	leader_text.set("text", g.leader_mode and " LEADER " or "")
	bars.top.set("style", style.ref(g.leader_mode and "Bar.Leader" or "Bar"))
end
cmd.event.on({ "ClientLoaded", "LeaderModeChanged" }, update_leader)

-- Update the active-Tab line when its selection, title, process, or error changes.
local function update_status()
	if g.last_error then
		status_text.set("text", string.format(" Error: %s ", g.last_error))
	elseif not g.active_tab then
		status_text.set("text", " No active tab ")
	else
		local status = g.active_tab.running and "running" or "exited"
		status_text.set("text", string.format(" %s · %s ", g.active_tab.title, status))
	end
	bars.bottom.set("style", style.ref(g.last_error and "Bar.Error" or "Bar.Status"))
end
cmd.event.on({
	"ClientLoaded",
	"LastErrorChanged", "SessionCreated", "SessionSelected", "SessionClosed",
	"WorkspaceSelected", "TabCreated", "TabClosed", "TabSelected",
	"TerminalTitleChanged", "ProcessExited",
}, update_status)

-- Session labels change on session selection; dimensions change on terminal resize.
local function update_session_info()
	local session = g.session and g.session.number or "?"
	session_label.set("text", string.format(" Session %s · %dx%d ", session, g.terminal.columns, g.terminal.rows))
end
cmd.event.on({
	"ClientLoaded", "SessionCreated", "SessionSelected", "SessionClosed", "TerminalResized",
}, update_session_info)

-- PreAttach hooks run after the TUI is initialized and before the Client attaches.
-- `args` contains raw command-line arguments (excluding the executable), and
-- `launch` holds the resolved startup target and settings.
--
-- Example: let `zmux new` choose an existing Session before attaching.
-- cmd.event.on("PreAttach", function()
-- 	if args[1] ~= "new" or launch.session ~= nil then
-- 		return
-- 	end
-- 	cmd.ui.pick({ "work", "personal" }, function(session)
-- 		if session then
-- 			launch.session = session
-- 			launch.create_if_missing = false
-- 		end
-- 	end)
-- end, { await = true })
--
-- Example: use the Session picker for `zmux list -a`.
-- cmd.event.on("PreAttach", function()
-- 	if args[1] == "list" then
-- 		cmd.session.switch()
-- 	end
-- end, { await = true })
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
cmd.key.bind("th", function()
	cmd.ui.pick(cmd.style.themes(), function(theme)
		if theme then
			cmd.style.apply_theme(theme)
		end
	end)
end, { desc = "Choose a style theme" })

cmd.key.bind("<C-x>", function()
	cmd.server.stop()
end, { desc = "Stop the background process" })

local ui = cmd.ui

local message = ui.text({ text = "Hello from your custom drawer!" })
local drawer = ui.create({
	type = "drawer",
	side = "left",
	width = 36,
	border = "quadrant_inside",
	content = ui.column({ message }),
	keys = {
		q = function(element)
			element.hide()
		end,
	},
})

-- Press the configured leader, then "ui" to open and focus the drawer.
cmd.key.bind("ui", function()
	drawer.focus() -- Also shows it if it's hidden.
end, { desc = "Open custom drawer" })
