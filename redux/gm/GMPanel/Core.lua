-- GM Panel: state, sending, server replies, favourites and recents.
-- Written for the 1.12 API (Lua 5.0: `this`, `arg1`, getglobal, table.getn).

BINDING_HEADER_GMPANEL = "GM Panel"
BINDING_NAME_GMPANEL_TOGGLE = "Toggle GM panel"

GMPanel = {
	level = nil,         -- account security level, from the `.account` reply
	toggles = {},        -- toggle key -> true/false, last known
	capture = nil,       -- { cmd, untilT } while a sent command's replies are expected
	lookup = {},         -- dataset -> rows parsed from `.lookup` replies
	probeUntil = 0,      -- replies to the panel's own probes are kept out of chat until then
	byId = {},
}

local CAPTURE_SECS = 3
local MAX_RECENT = 20
local MAX_FAVS = 10

function GMPanel_Print(msg)
	DEFAULT_CHAT_FRAME:AddMessage("|cff33ff99GM:|r " .. msg)
end

for i = 1, table.getn(GMPANEL_CATALOG) do
	local e = GMPANEL_CATALOG[i]
	GMPanel.byId[e.id] = e
end

-- The server's own security levels win over the catalogue's.
local function ApplyServerLevels()
	if not (GMPanel_Data and GMPanel_Data.commands) then return end
	local sec = {}
	for i = 1, table.getn(GMPanel_Data.commands) do
		local row = GMPanel_Data.commands[i]
		sec[row[1]] = row[2]
	end
	for i = 1, table.getn(GMPANEL_CATALOG) do
		local e = GMPANEL_CATALOG[i]
		if sec[e.path] then e.sec = sec[e.path] end
	end
end

local function DefaultFavs()
	local favs = {}
	for i = 1, table.getn(GMPANEL_CATALOG) do
		local e = GMPANEL_CATALOG[i]
		if e.fav and table.getn(favs) < MAX_FAVS then tinsert(favs, e.id) end
	end
	return favs
end

local function InitDB()
	if type(GMPanelDB) ~= "table" then GMPanelDB = {} end
	if type(GMPanelDB.favs) ~= "table" then GMPanelDB.favs = DefaultFavs() end
	if type(GMPanelDB.recent) ~= "table" then GMPanelDB.recent = {} end
	if type(GMPanelDB.cat) ~= "string" then GMPanelDB.cat = "all" end
end

-- Target ---------------------------------------------------------------------

-- "self", "player" or "npc", and the name.
function GMPanel_TargetInfo()
	if UnitExists("target") and not UnitIsUnit("target", "player") then
		if UnitIsPlayer("target") then
			return "player", UnitName("target")
		end
		return "npc", UnitName("target")
	end
	return "self", UnitName("player")
end

function GMPanel_TargetLevel()
	local kind = GMPanel_TargetInfo()
	if kind ~= "self" then return UnitLevel("target") end
	return UnitLevel("player")
end

-- Who a command will act on, from its vmangos target rule.
-- Returns text, severity ("ok", "warn", "block"), and whether to name yourself.
function GMPanel_TargetNote(e, v)
	local kind, name = GMPanel_TargetInfo()
	local rule = e.target or "self"
	local named = v and v.player and v.player ~= ""
	if named then return "On " .. v.player, "ok" end
	if rule == "self" then return "On you", "ok" end
	if rule == "needsel" then
		if kind == "self" and not UnitExists("target") then return "Select a target first", "block" end
		return "On " .. name, "ok"
	end
	if kind == "self" then return "On you", "ok" end
	if rule == "unit" then
		return "On " .. name .. (kind == "npc" and " (NPC)" or ""), "ok"
	end
	if rule == "unitfirst" then
		if kind == "npc" then return "On the NPC " .. name, "warn" end
		return "On " .. name, "ok"
	end
	if kind == "player" then return "On " .. name, "ok" end
	-- An NPC is targeted.
	if rule == "playerfb" then return "On you (NPC target ignored)", "ok" end
	if rule == "name" then return "On you (named, NPC target ignored)", "ok", true end
	return "Needs a player or no target", "block"
end

-- Building the command line --------------------------------------------------

local function ParamText(p, val)
	if val == nil or val == "" then return nil end
	if p.t == "onoff" then return val and "on" or "off" end
	if p.t == "bool01" then return val and "1" or "0" end
	return tostring(val)
end

function GMPanel_Build(e, v)
	v = v or {}
	local args
	if e.build then
		args = e.build(v)
	else
		local parts = {}
		for i = 1, table.getn(e.params) do
			local p = e.params[i]
			local t = ParamText(p, v[p.k])
			if t then tinsert(parts, t) end
		end
		args = table.concat(parts, " ")
	end
	local _, _, nameSelf = GMPanel_TargetNote(e, v)
	if nameSelf then
		local me = UnitName("player")
		if e.path == "character level" or e.path == "levelup" then
			args = me .. (args ~= "" and (" " .. args) or "")
		else
			args = (args ~= "" and (args .. " ") or "") .. me
		end
	end
	if args and args ~= "" then return "." .. e.path .. " " .. args end
	return "." .. e.path
end

-- Required parameters that are still empty.
function GMPanel_Missing(e, v)
	for i = 1, table.getn(e.params) do
		local p = e.params[i]
		local val = v and v[p.k]
		if not p.opt and (val == nil or val == "") and p.t ~= "onoff" and p.t ~= "bool01" then
			return p
		end
	end
	return nil
end

-- Sending ---------------------------------------------------------------------

local function AddRecent(text)
	local r = GMPanelDB.recent
	for i = table.getn(r), 1, -1 do
		if r[i] == text then tremove(r, i) end
	end
	tinsert(r, 1, text)
	while table.getn(r) > MAX_RECENT do tremove(r) end
end

-- Toggle states follow what was sent until the server says otherwise.
local function NoteToggle(text)
	for i = 1, table.getn(GMPANEL_CATALOG) do
		local e = GMPANEL_CATALOG[i]
		if e.toggle then
			local _, _, state = string.find(string.lower(text), "^%." .. e.path .. " (%a+)")
			if state == "on" then GMPanel.toggles[e.toggle] = true end
			if state == "off" then GMPanel.toggles[e.toggle] = false end
		end
	end
end

function GMPanel_Send(text, quiet)
	text = string.gsub(text or "", "^%s+", "")
	text = string.gsub(text, "%s+$", "")
	if text == "" then return false end
	if string.sub(text, 1, 1) ~= "." then text = "." .. text end
	if string.len(text) > 255 then
		GMPanel_Print("command is longer than 255 characters; the server refuses it")
		return false
	end
	if quiet then
		GMPanel.probeUntil = GetTime() + CAPTURE_SECS
	else
		GMPanel.capture = { cmd = text, untilT = GetTime() + CAPTURE_SECS }
		AddRecent(text)
		NoteToggle(text)
		GMPanel_LogAdd("|cff9d9d9d> " .. text .. "|r")
	end
	-- The client refuses a dead player's SAY (ERR_CHAT_WHILE_DEAD) but not a whisper, and vmangos
	-- parses commands on every chat type before routing it, so the dead whisper themselves.
	if UnitIsDeadOrGhost("player") then
		SendChatMessage(text, "WHISPER", nil, UnitName("player"))
	else
		SendChatMessage(text, "SAY")
	end
	if GMPanel_OnSent then GMPanel_OnSent(text, quiet) end
	return true
end

-- Server replies --------------------------------------------------------------

local LINK_KINDS = { item = "items", tele = "tele", creature_entry = "creatures", quest = "quests", spell = "spells" }

local STATUS_LINES = {
	["Fly"] = "fly", ["Beastmaster"] = "beastmaster", ["God"] = "god", ["No cooldowns"] = "cooldown",
	["No cast time"] = "casttime", ["No power costs"] = "powercost", ["Untargetable"] = "untargetable",
	["Immune to creatures"] = "immunenpc", ["Water walking"] = "waterwalk", ["Wall climbing"] = "wallclimb",
}

-- Rows from `|Hkind:id...|h[name]|h` links in a reply line.
local function ParseLinks(msg)
	for kind, id, name in string.gfind(msg, "|H([%a_]+):(%d+)[^|]*|h%[(.-)%]|h") do
		local ds = LINK_KINDS[kind]
		if ds then
			GMPanel.lookup[ds] = GMPanel.lookup[ds] or {}
			local rows = GMPanel.lookup[ds]
			local n = tonumber(id)
			local seen = false
			for i = 1, table.getn(rows) do
				if rows[i][1] == n then seen = true end
			end
			if not seen then tinsert(rows, { n, name }) end
		end
	end
end

-- Learns from a system line.
local function OnSystem(msg)
	local _, _, lvl = string.find(msg, "^Your account level is: (%d+)")
	if lvl then
		GMPanel.level = tonumber(lvl)
	end
	if string.find(msg, "^Cheats active on ") then
		for _, key in pairs(STATUS_LINES) do GMPanel.toggles[key] = false end
	end
	local _, _, cheat = string.find(msg, "^%- (.+)$")
	if cheat and STATUS_LINES[cheat] then
		GMPanel.toggles[STATUS_LINES[cheat]] = true
	end
	if msg == "GM mode is ON" then GMPanel.toggles.gm = true end
	if msg == "GM mode is OFF" then GMPanel.toggles.gm = false end
	local _, _, god = string.find(msg, "^You set god mode to (%a+)")
	if god then GMPanel.toggles.god = (god == "on") end

	local cap = GMPanel.capture
	if cap and GetTime() <= cap.untilT then
		ParseLinks(msg)
		GMPanel_LogAdd(msg)
		if string.find(msg, "^Incorrect syntax") or string.find(msg, "do not have permission")
			or string.find(msg, "^There is no such command") then
			cap.failed = true
		end
		if GMPanel_OnReply then GMPanel_OnReply(msg, cap) end
	end
end

-- A reply to the panel's own quiet probes (`.account`, `.cheat status`).
local function IsProbeLine(msg)
	if GetTime() > GMPanel.probeUntil then return false end
	if string.find(msg, "^Your account level is: ") or string.find(msg, "^Cheats active on ") then return true end
	local _, _, cheat = string.find(msg, "^%- (.+)$")
	return cheat ~= nil and STATUS_LINES[cheat] ~= nil
end

-- Keeps those replies out of the chat frames, which see the event before the panel does.
local origChatFrameOnEvent = ChatFrame_OnEvent
function ChatFrame_OnEvent(event)
	if event == "CHAT_MSG_SYSTEM" and IsProbeLine(arg1 or "") then
		return
	end
	return origChatFrameOnEvent(event)
end

-- Favourites -----------------------------------------------------------------

function GMPanel_IsFav(id)
	for i = 1, table.getn(GMPanelDB.favs) do
		if GMPanelDB.favs[i] == id then return i end
	end
	return nil
end

function GMPanel_ToggleFav(id)
	local i = GMPanel_IsFav(id)
	if i then
		tremove(GMPanelDB.favs, i)
	elseif table.getn(GMPanelDB.favs) < MAX_FAVS then
		tinsert(GMPanelDB.favs, id)
	else
		GMPanel_Print("the favourites strip holds " .. MAX_FAVS .. "; remove one first")
	end
	if GMPanel_Refresh then GMPanel_Refresh() end
end

-- Probes the panel sends when first opened: the account level and the cheat states.
function GMPanel_Probe()
	if GMPanel.probed then return end
	GMPanel.probed = true
	GMPanel_Send(".account", true)
	GMPanel_Send(".cheat status", true)
end

-- Events ---------------------------------------------------------------------

local events = CreateFrame("Frame", "GMPanelEvents")
events:RegisterEvent("VARIABLES_LOADED")
events:RegisterEvent("CHAT_MSG_SYSTEM")
events:RegisterEvent("PLAYER_TARGET_CHANGED")
events:SetScript("OnEvent", function()
	if event == "VARIABLES_LOADED" then
		InitDB()
		ApplyServerLevels()
		if JTU_RegisterCommand then
			JTU_RegisterCommand("gm", function(rest) GMPanel_Slash(rest) end, "open the GM panel")
		end
		if GMPanel_OnLoaded then GMPanel_OnLoaded() end
	elseif event == "CHAT_MSG_SYSTEM" then
		OnSystem(arg1 or "")
		if GMPanel_Refresh then GMPanel_Refresh() end
	elseif event == "PLAYER_TARGET_CHANGED" then
		if GMPanel_Refresh then GMPanel_Refresh() end
	end
end)
