-- GM Panel: the curated command catalogue.
-- Paths, argument order and target rules are read from the vmangos handlers
-- (core/src/game/Commands/*.cpp); vmangos ships no help text of its own.
--
-- Entry fields:
--   id, cat, label, path   the command is "." .. path .. " " .. args
--   sec                    security level (overridden by GMPanel_Data.commands when present)
--   params                 { k, t, label, def, min, max, opt, opts }
--                          t: int num text onoff bool01 enum item tele creature quest spell player
--   target                 how vmangos picks who it acts on:
--                            self       always you
--                            player     selected player, else you; fails with an NPC targeted
--                            playerfb   selected player, else you (an NPC targeted means you)
--                            unit       selected unit (player or NPC), else you
--                            unitfirst  .levelup: a targeted NPC first, else as `name`
--                            name       a name argument, else selected player, else you
--                            needsel    needs an explicit selection
--   danger                 true, or function(v) -> bool: asks for a second press
--   perm                   writes to the world DB
--   toggle                 key of the remembered on/off state
--   build                  function(v) -> args string, when plain argument order is not enough
--   fav                    a default favourite

GMPANEL_CATEGORIES = {
	{ key = "all",    label = "All" },
	{ key = "fav",    label = "Favourites" },
	{ key = "recent", label = "Recent" },
	{ key = "char",   label = "Character" },
	{ key = "toggle", label = "Toggles" },
	{ key = "tele",   label = "Teleport" },
	{ key = "item",   label = "Items" },
	{ key = "spell",  label = "Spells" },
	{ key = "life",   label = "Life / Combat" },
	{ key = "npc",    label = "NPCs / Objects" },
	{ key = "quest",  label = "Quests" },
	{ key = "world",  label = "World" },
	{ key = "server", label = "Server" },
	{ key = "raw",    label = "Raw / all" },
}

GMPANEL_SEC_NAMES = { [0] = "PLA", "MOD", "TIC", "GAM", "BAS", "DEV", "ADM", "CON" }

local function P(k, t, label, extra)
	local p = { k = k, t = t, label = label }
	if extra then
		for key, val in pairs(extra) do p[key] = val end
	end
	return p
end

local function onoff(cat, id, path, label, sec, toggle, target, fav)
	return { id = id, cat = cat, path = path, label = label, sec = sec, toggle = toggle,
		target = target or "self", fav = fav, params = { P("state", "onoff", "State") } }
end

local function plain(cat, id, path, label, sec, target, extra)
	local e = { id = id, cat = cat, path = path, label = label, sec = sec, target = target or "self", params = {} }
	if extra then
		for key, val in pairs(extra) do e[key] = val end
	end
	return e
end

local function money(v)
	local c = (tonumber(v.gold) or 0) * 10000 + (tonumber(v.silver) or 0) * 100 + (tonumber(v.copper) or 0)
	return tostring(c)
end

GMPANEL_CATALOG = {
	-- Character
	plain("char", "setlevel", "levelup", "Set level", 3, "unitfirst", { fav = true,
		params = { P("level", "int", "Level", { def = 60, min = 1, max = 60 }) },
		build = function(v)
			local lvl = tonumber(v.level) or 60
			local cur = GMPanel_TargetLevel() or UnitLevel("player") or 1
			return tostring(lvl - cur)
		end,
		note = "Sends .levelup with the difference from the current level." }),
	plain("char", "levelup", "levelup", "Level up by", 3, "unitfirst", {
		params = { P("levels", "int", "Levels", { def = 1, min = -59, max = 59 }) } }),
	plain("char", "charlevel", "character level", "Set level (direct)", 5, "name", {
		params = { P("level", "int", "Level", { def = 60, min = 1, max = 60 }) } }),
	plain("char", "money", "modify money", "Give money", 4, "player", { fav = true,
		params = { P("gold", "int", "Gold", { def = 100 }), P("silver", "int", "Silver", { opt = true }),
			P("copper", "int", "Copper", { opt = true }) },
		build = money, note = "Negative amounts take money away." }),
	plain("char", "speed", "modify speed", "Run speed", 2, "player", {
		params = { P("rate", "num", "Rate", { def = 2, min = 0.1, max = 10 }) } }),
	plain("char", "aspeed", "modify aspeed", "All speeds", 2, "player", {
		params = { P("rate", "num", "Rate", { def = 2, min = 0.1, max = 10 }) } }),
	plain("char", "swim", "modify swim", "Swim speed", 2, "player", {
		params = { P("rate", "num", "Rate", { def = 2, min = 0.1, max = 10 }) } }),
	plain("char", "hp", "modify hp", "Set health", 3, "unit", {
		params = { P("value", "int", "Health", { def = 10000 }) } }),
	plain("char", "mana", "modify mana", "Set mana", 3, "unit", {
		params = { P("value", "int", "Mana", { def = 10000 }) } }),
	plain("char", "scale", "modify scale", "Scale", 3, "unit", {
		params = { P("scale", "num", "Scale", { def = 1, min = 0.1, max = 10 }) } }),
	plain("char", "morph", "modify morph", "Morph", 3, "unit", {
		params = { P("display", "int", "Display ID", { def = 10045 }) } }),
	plain("char", "demorph", "demorph", "Demorph", 3, "unit"),
	plain("char", "maxskill", "maxskill", "Max weapon skills", 3, "playerfb"),
	plain("char", "repair", "repairitems", "Repair items", 3, "name", { fav = true }),
	plain("char", "talentpts", "modify tp", "Set talent points", 4, "player", {
		params = { P("points", "int", "Points", { def = 51, min = 0, max = 51 }) } }),
	plain("char", "resettalents", "reset talents", "Reset talents", 3, "name", { danger = true }),
	plain("char", "resetlevel", "reset level", "Reset level", 5, "name", { danger = true }),
	plain("char", "resetspells", "reset spells", "Reset spells", 5, "player", { danger = true }),
	plain("char", "resetstats", "reset stats", "Reset stats", 5, "name", { danger = true }),
	plain("char", "premadegear", "character premade gear", "Premade gear", 4, "player", {
		params = { P("template", "text", "Template", { opt = true }) },
		note = "No template lists the ones for your class." }),
	plain("char", "premadespec", "character premade spec", "Premade talent spec", 4, "player", {
		params = { P("template", "text", "Template", { opt = true }) } }),
	plain("char", "save", "save", "Save character", 0, "self"),

	-- Toggles
	onoff("toggle", "gm", "gm", "GM mode", 2, "gm", "self", true),
	onoff("toggle", "gmvisible", "gm visible", "GM visible", 2, "visible", "self"),
	onoff("toggle", "god", "cheat god", "God mode", 3, "god", "name", true),
	onoff("toggle", "fly", "cheat fly", "Fly", 3, "fly", "playerfb", true),
	onoff("toggle", "cooldown", "cheat cooldown", "No cooldowns", 3, "cooldown", "name"),
	onoff("toggle", "casttime", "cheat casttime", "No cast time", 3, "casttime", "name"),
	onoff("toggle", "powercost", "cheat powercost", "No power costs", 3, "powercost", "name"),
	onoff("toggle", "waterwalk", "cheat waterwalk", "Water walk", 3, "waterwalk", "name"),
	onoff("toggle", "wallclimb", "cheat wallclimb", "Wall climb", 3, "wallclimb", "name"),
	onoff("toggle", "untargetable", "cheat untargetable", "Untargetable", 3, "untargetable", "name"),
	onoff("toggle", "immunenpc", "cheat immunenpc", "Immune to creatures", 3, "immunenpc", "name"),
	onoff("toggle", "beastmaster", "cheat beastmaster", "Beastmaster", 3, "beastmaster", "name"),
	onoff("toggle", "taxicheat", "taxicheat", "All flight paths", 2, "taxi", "playerfb"),
	{ id = "explore", cat = "toggle", path = "explorecheat", label = "Explore all zones", sec = 2,
		target = "player", params = { P("state", "bool01", "Explored") } },
	plain("toggle", "cheatstatus", "cheat status", "Cheat status", 3, "name"),

	-- Teleport
	plain("tele", "tele", "tele", "Teleport to", 2, "self", { fav = true,
		params = { P("loc", "tele", "Location") } }),
	plain("tele", "telename", "tele name", "Teleport player to", 2, "self", {
		params = { P("player", "player", "Player"), P("loc", "tele", "Location") } }),
	plain("tele", "goxyz", "go xyz", "Go to coordinates", 2, "self", {
		params = { P("x", "num", "X"), P("y", "num", "Y"), P("z", "num", "Z"),
			P("map", "int", "Map", { opt = true }) } }),
	plain("tele", "goname", "goname", "Go to player", 2, "name", {
		params = { P("player", "player", "Player", { opt = true }) } }),
	plain("tele", "namego", "namego", "Summon player", 2, "name", {
		params = { P("player", "player", "Player", { opt = true }) } }),
	plain("tele", "recall", "recall", "Recall", 1, "name", {
		params = { P("player", "player", "Player", { opt = true }) } }),
	plain("tele", "gocorpse", "gocorpse", "Go to corpse", 2, "self"),
	plain("tele", "gps", "gps", "Show position", 1, "unit"),

	-- Items
	plain("item", "additem", "additem", "Add item", 3, "playerfb", { fav = true,
		params = { P("item", "item", "Item"), P("count", "int", "Count", { def = 1 }) },
		danger = function(v) return (tonumber(v.count) or 1) < 0 end }),
	plain("item", "removeitem", "additem", "Remove item", 3, "playerfb", { danger = true,
		params = { P("item", "item", "Item"), P("count", "int", "Count", { def = 1, min = 1 }) },
		build = function(v) return tostring(v.item) .. " -" .. tostring(math.abs(tonumber(v.count) or 1)) end }),
	plain("item", "additemset", "additemset", "Add item set", 3, "playerfb", {
		params = { P("set", "int", "Item set ID") } }),
	plain("item", "lookupitem", "lookup item", "Look up item", 2, "self", {
		params = { P("text", "text", "Name contains") } }),
	plain("item", "lookupitemset", "lookup itemset", "Look up item set", 2, "self", {
		params = { P("text", "text", "Name contains") } }),

	-- Spells
	plain("spell", "learn", "learn", "Learn spell", 5, "player", { fav = true,
		params = { P("spell", "spell", "Spell") } }),
	plain("spell", "learnclass", "learn all_myclass", "Learn all class spells", 5, "player"),
	plain("spell", "learntalents", "learn all_mytalents", "Learn all talents", 5, "player"),
	plain("spell", "learncrafts", "learn all_crafts", "Learn all professions", 3, "player"),
	plain("spell", "learnrecipes", "learn all_recipes", "Learn all recipes", 3, "player", {
		params = { P("skill", "text", "Profession") } }),
	plain("spell", "learnlang", "learn all_lang", "Learn all languages", 1, "player"),
	plain("spell", "learntaxis", "learn all_mytaxis", "Learn my flight paths", 2, "player"),
	plain("spell", "learngm", "learn all_gm", "Learn GM spells", 3, "player"),
	plain("spell", "unlearn", "unlearn", "Unlearn spell", 3, "player", {
		params = { P("spell", "spell", "Spell") } }),
	plain("spell", "cast", "cast", "Cast spell", 5, "unit", {
		params = { P("spell", "spell", "Spell") } }),
	plain("spell", "aura", "aura", "Apply aura", 4, "unit", {
		params = { P("spell", "spell", "Spell"), P("secs", "int", "Seconds", { opt = true }) } }),
	plain("spell", "unaura", "unaura", "Remove aura", 3, "unit", {
		params = { P("spell", "spell", "Spell") } }),
	plain("spell", "unauraall", "unaura", "Remove all auras", 3, "unit", {
		build = function() return "all" end }),
	plain("spell", "cdclear", "cooldown clear", "Clear cooldowns", 3, "unit"),
	plain("spell", "lookupspell", "lookup spell", "Look up spell", 2, "self", {
		params = { P("text", "text", "Name contains") } }),

	-- Life / combat
	plain("life", "revive", "revive", "Revive", 3, "name", { fav = true }),
	plain("life", "die", "die", "Kill (.die)", 3, "unit", {
		danger = function() return not UnitExists("target") or UnitIsUnit("target", "player") end }),
	plain("life", "damage", "damage", "Damage target", 3, "needsel", {
		params = { P("amount", "int", "Amount", { def = 100 }) } }),
	plain("life", "replenish", "replenish", "Replenish", 3, "unit"),
	plain("life", "combatstop", "combatstop", "Stop combat", 3, "name"),

	-- NPCs and objects
	plain("npc", "npcsummon", "npc summon", "Summon NPC (temporary)", 3, "self", { fav = true,
		params = { P("entry", "creature", "Creature") } }),
	plain("npc", "npcadd", "npc spawn add", "Spawn NPC (permanent)", 5, "self", { perm = true,
		params = { P("entry", "creature", "Creature") } }),
	plain("npc", "npcdelete", "npc spawn delete", "Delete NPC spawn", 5, "needsel", { danger = true, perm = true }),
	plain("npc", "npcinfo", "npc info", "NPC info", 1, "needsel"),
	plain("npc", "goadd", "gobject add", "Spawn object (permanent)", 5, "self", { perm = true,
		params = { P("entry", "int", "Object entry"), P("respawn", "int", "Respawn s", { opt = true }) } }),
	plain("npc", "gotmp", "gobject tmpadd", "Spawn object (temporary)", 3, "self", {
		params = { P("entry", "int", "Object entry") } }),
	plain("npc", "gonear", "gobject near", "Objects nearby", 2, "self", {
		params = { P("dist", "int", "Distance", { opt = true }) } }),
	plain("npc", "godelete", "gobject delete", "Delete object", 5, "self", { danger = true, perm = true,
		params = { P("guid", "int", "Object GUID") } }),
	plain("npc", "lookupcreature", "lookup creature", "Look up creature", 2, "self", {
		params = { P("text", "text", "Name contains") } }),
	plain("npc", "lookupobject", "lookup object", "Look up object", 2, "self", {
		params = { P("text", "text", "Name contains") } }),

	-- Quests
	plain("quest", "questcomplete", "quest complete", "Complete quest", 4, "name", { fav = true,
		params = { P("quest", "quest", "Quest") } }),
	plain("quest", "questadd", "quest add", "Add quest", 4, "name", {
		params = { P("quest", "quest", "Quest") } }),
	plain("quest", "questremove", "quest remove", "Remove quest", 3, "name", { danger = true,
		params = { P("quest", "quest", "Quest") } }),
	plain("quest", "queststatus", "quest status", "Quest status", 3, "name", {
		params = { P("quest", "quest", "Quest") } }),
	plain("quest", "lookupquest", "lookup quest", "Look up quest", 2, "self", {
		params = { P("text", "text", "Name contains") } }),

	-- World
	plain("world", "weather", "wchange", "Change weather", 4, "self", {
		params = { P("type", "enum", "Type", { def = "1", opts = { { "0", "Fine" }, { "1", "Rain" }, { "2", "Snow" }, { "3", "Storm" } } }),
			P("grade", "num", "Intensity", { def = 0.5, min = 0, max = 1 }) },
		note = "Needs weather enabled in mangosd.conf." }),
	plain("world", "eventlist", "event list", "Game events", 3, "self"),
	plain("world", "eventstart", "event start", "Start event", 4, "self", {
		params = { P("id", "int", "Event ID") } }),
	plain("world", "eventstop", "event stop", "Stop event", 4, "self", {
		params = { P("id", "int", "Event ID") } }),
	plain("world", "honoradd", "honor add", "Add honor", 4, "player", {
		params = { P("amount", "int", "Honor", { def = 1000 }) } }),
	plain("world", "honorshow", "honor show", "Show honor", 2, "player"),
	plain("world", "rep", "modify rep", "Set reputation", 4, "player", {
		params = { P("faction", "int", "Faction ID"), P("amount", "int", "Amount", { def = 42999 }) } }),

	-- Server and account
	plain("server", "account", "account", "My account level", 0, "self"),
	plain("server", "serverinfo", "server info", "Server info", 0, "self"),
	plain("server", "announce", "announce", "Announce", 4, "self", {
		params = { P("text", "text", "Message") } }),
	plain("server", "pinfo", "pinfo", "Player info", 2, "name", {
		params = { P("player", "player", "Player", { opt = true }) } }),
	plain("server", "kick", "kick", "Kick player", 2, "name", { danger = true,
		params = { P("player", "player", "Player") } }),
	plain("server", "shutdown", "server shutdown", "Shut down server", 6, "self", { danger = true,
		params = { P("secs", "int", "Delay s", { def = 60, min = 0 }) } }),
	plain("server", "restart", "server restart", "Restart server", 6, "self", { danger = true,
		params = { P("secs", "int", "Delay s", { def = 60, min = 0 }) } }),
	plain("server", "shutdowncancel", "server shutdown cancel", "Cancel shutdown", 6, "self"),
	plain("server", "gmlevel", "account set gmlevel", "Set account GM level", 7, "self", { danger = true,
		params = { P("account", "text", "Account"), P("level", "int", "Level", { def = 3, min = 0, max = 6 }) } }),
}

-- Words people type for commands vmangos names differently.
GMPANEL_ALIASES = {
	summon = "namego", appear = "goname", kill = "die", port = "tele", teleport = "tele",
	give = "additem", item = "additem", money = "money", gold = "money", level = "setlevel",
	god = "god", invincible = "god", fly = "fly", res = "revive", rez = "revive", resurrect = "revive",
	spawn = "npcsummon", npc = "npcsummon", quest = "questcomplete", complete = "questcomplete",
	learn = "learn", spell = "learn", repair = "repair", speed = "speed", weather = "weather",
}
