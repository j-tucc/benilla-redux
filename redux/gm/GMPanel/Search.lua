-- GM Panel: search over the catalogue, the raw command list and the name tables.

GMPANEL_DATASETS = {
	{ key = "tele",      label = "Teleports", prefix = "t", lookup = "lookup tele",     cat = "tele" },
	{ key = "items",     label = "Items",     prefix = "i", lookup = "lookup item",     cat = "item" },
	{ key = "creatures", label = "Creatures", prefix = "n", lookup = "lookup creature", cat = "npc" },
	{ key = "quests",    label = "Quests",    prefix = "q", lookup = "lookup quest",    cat = "quest" },
	{ key = "spells",    label = "Spells",    prefix = "s", lookup = "lookup spell",    cat = "spell" },
}

GMPANEL_DATASET = {}
for i = 1, table.getn(GMPANEL_DATASETS) do
	GMPANEL_DATASET[GMPANEL_DATASETS[i].key] = GMPANEL_DATASETS[i]
end

-- The parameter type each picker searches.
GMPANEL_PICKER_DATASET = { item = "items", tele = "tele", creature = "creatures", quest = "quests", spell = "spells" }

local GROUP_SHOWN = 5   -- rows per group in a mixed search
local SCOPE_MAX = 300   -- rows kept when one group is searched alone

local lowers = {}

local function Rows(ds)
	if GMPanel_Data and GMPanel_Data[ds] and table.getn(GMPanel_Data[ds]) > 0 then
		return GMPanel_Data[ds], false
	end
	return GMPanel.lookup[ds] or {}, true
end

function GMPanel_HasData(ds)
	local _, live = Rows(ds)
	return not live
end

local function Lowers(ds, rows)
	local l = lowers[ds]
	if l and l.n == table.getn(rows) and l.rows == rows then return l end
	l = { n = table.getn(rows), rows = rows }
	for i = 1, l.n do l[i] = string.lower(rows[i][2]) end
	lowers[ds] = l
	return l
end

-- 3 prefix, 2 word start, 1 anywhere, nil no match.
local function DataScore(low, q)
	local s = string.find(low, q, 1, true)
	if not s then return nil end
	if s == 1 then return 3 end
	local c = string.sub(low, s - 1, s - 1)
	if c == " " or c == "-" or c == "'" or c == "(" then return 2 end
	if string.find(low, " " .. q, 1, true) then return 2 end
	return 1
end

-- Matches in one name table, best first; returns rows and the total match count.
function GMPanel_SearchData(ds, q, max)
	local rows = Rows(ds)
	local found, total = {}, 0
	if q == "" then
		for i = 1, math.min(table.getn(rows), max) do tinsert(found, { row = rows[i], score = 0 }) end
		return found, table.getn(rows)
	end
	local low = Lowers(ds, rows)
	local buckets = { {}, {}, {} }
	for i = 1, low.n do
		local sc = DataScore(low[i], q)
		if sc then
			total = total + 1
			local b = buckets[sc]
			if table.getn(b) < max then tinsert(b, rows[i]) end
		end
	end
	for sc = 3, 1, -1 do
		local b = buckets[sc]
		table.sort(b, function(a, c)
			local la, lc = string.len(a[2]), string.len(c[2])
			if la ~= lc then return la < lc end
			return a[1] < c[1]
		end)
		for i = 1, table.getn(b) do
			if table.getn(found) >= max then break end
			tinsert(found, { row = b[i], score = sc })
		end
	end
	return found, total
end

-- Fuzzy: a subsequence of the query's letters, scored by how early and how tight.
local function Fuzzy(hay, q)
	local pos, gaps, first = 1, 0, nil
	for i = 1, string.len(q) do
		local c = string.sub(q, i, i)
		local at = string.find(hay, c, pos, true)
		if not at then return nil end
		if not first then first = at end
		gaps = gaps + (at - pos)
		pos = at + 1
	end
	-- Loose subsequences ("orgr" in "go corpse") are noise, not matches.
	if gaps > string.len(q) then return nil end
	return 100 - gaps - first
end

local function CommandScore(e, q)
	if q == "" then return 1 end
	local label, path = string.lower(e.label), e.path
	local score = nil
	if GMPANEL_ALIASES[q] == e.id then score = 1000 end
	local function try(s)
		if s and (not score or s > score) then score = s end
	end
	if string.sub(label, 1, string.len(q)) == q or string.sub(path, 1, string.len(q)) == q then try(900) end
	if string.find(" " .. label, " " .. q, 1, true) or string.find(" " .. path, " " .. q, 1, true) then try(800) end
	local at = string.find(label .. " " .. path .. " " .. e.id, q, 1, true)
	if at then try(600 - at) end
	try(Fuzzy(label .. " " .. path, q))
	-- An alias whose word starts the query ("summon" -> namego).
	for word, id in pairs(GMPANEL_ALIASES) do
		if id == e.id and string.sub(word, 1, string.len(q)) == q then try(850) end
	end
	return score
end

function GMPanel_SearchCommands(q, cat)
	local found = {}
	for i = 1, table.getn(GMPANEL_CATALOG) do
		local e = GMPANEL_CATALOG[i]
		if cat == nil or cat == "all" or e.cat == cat then
			local sc = CommandScore(e, q)
			if sc then tinsert(found, { e = e, score = sc, order = i }) end
		end
	end
	table.sort(found, function(a, b)
		if a.score ~= b.score then return a.score > b.score end
		return a.order < b.order
	end)
	return found
end

local function RawCommands()
	if GMPanel_Data and GMPanel_Data.commands then return GMPanel_Data.commands end
	if not GMPanel.rawFallback then
		local t, seen = {}, {}
		for i = 1, table.getn(GMPANEL_CATALOG) do
			local e = GMPANEL_CATALOG[i]
			if not seen[e.path] then
				seen[e.path] = true
				tinsert(t, { e.path, e.sec })
			end
		end
		table.sort(t, function(a, b) return a[1] < b[1] end)
		GMPanel.rawFallback = t
	end
	return GMPanel.rawFallback
end

-- The player's quest log, matched by title to the quest table.
local function QuestLogRows()
	local out = {}
	local rows = Rows("quests")
	local byTitle = {}
	for i = 1, table.getn(rows) do
		local t = rows[i][2]
		byTitle[t] = byTitle[t] or {}
		tinsert(byTitle[t], rows[i])
	end
	for i = 1, GetNumQuestLogEntries() do
		local title, level, _, isHeader = GetQuestLogTitle(i)
		if title and not isHeader then
			local cands = byTitle[title]
			if cands then
				local pick = cands[1]
				for j = 1, table.getn(cands) do
					if cands[j][3] == level then pick = cands[j] end
				end
				tinsert(out, pick)
			end
		end
	end
	return out
end

-- Splits a scope prefix: "i:thunder" -> "items", "thunder"; ".gm" -> "raw", "gm".
function GMPanel_ParseQuery(text)
	local q = string.lower(text or "")
	q = string.gsub(q, "^%s+", "")
	q = string.gsub(q, "%s+$", "")
	local _, _, p, rest = string.find(q, "^(%a):%s*(.*)$")
	if p then
		for i = 1, table.getn(GMPANEL_DATASETS) do
			if GMPANEL_DATASETS[i].prefix == p then return GMPANEL_DATASETS[i].key, rest end
		end
	end
	if string.sub(q, 1, 1) == "." then return "raw", string.sub(q, 2) end
	return nil, q
end

local function AddData(out, ds, q, max, grouped)
	local d = GMPANEL_DATASET[ds]
	local found, total = GMPanel_SearchData(ds, q, max)
	local live = not GMPanel_HasData(ds)
	if total == 0 and not live then return end
	if grouped or total > 0 then
		tinsert(out, { kind = "header", label = d.label, more = total - table.getn(found), scope = ds })
	end
	for i = 1, table.getn(found) do
		tinsert(out, { kind = "data", ds = ds, row = found[i].row })
	end
	if live and q ~= "" then
		tinsert(out, { kind = "lookup", ds = ds, q = q, label = "Search the server: ." .. d.lookup .. " " .. q })
	end
	return found[1] and found[1].score or 0
end

local STRONG = 500  -- prefix, word-start or substring; below is fuzzy only

local function AddCommands(out, q, cat, max, header, minScore, maxScore)
	local all = GMPanel_SearchCommands(q, cat)
	local cmds = {}
	for i = 1, table.getn(all) do
		local sc = all[i].score
		if (not minScore or sc >= minScore) and (not maxScore or sc < maxScore) then tinsert(cmds, all[i]) end
	end
	if table.getn(cmds) == 0 then return end
	if header then
		tinsert(out, { kind = "header", label = "Commands", more = math.max(0, table.getn(cmds) - max), scope = "cmds" })
	end
	for i = 1, math.min(table.getn(cmds), max) do
		tinsert(out, { kind = "cmd", e = cmds[i].e })
	end
end

local function AddRaw(out, q)
	local raw = RawCommands()
	for i = 1, table.getn(raw) do
		local path = raw[i][1]
		if q == "" or string.find(path, q, 1, true) then
			tinsert(out, { kind = "raw", path = path, sec = raw[i][2] })
		end
	end
end

local function AddRecent(out)
	for i = 1, table.getn(GMPanelDB.recent) do
		tinsert(out, { kind = "recent", text = GMPanelDB.recent[i] })
	end
end

-- The result list for the search box, the selected category and an optional scope.
function GMPanel_Search(text, cat, scope)
	local pscope, q = GMPanel_ParseQuery(text)
	scope = pscope or scope
	local out = {}
	if scope == "raw" then
		AddRaw(out, q)
		return out
	end
	if scope == "cmds" then
		AddCommands(out, q, cat, 1000, false)
		return out
	end
	if scope and GMPANEL_DATASET[scope] then
		AddData(out, scope, q, SCOPE_MAX, false)
		return out
	end
	if cat == "raw" then
		AddRaw(out, q)
	elseif cat == "recent" then
		AddRecent(out)
	elseif cat == "fav" then
		for i = 1, table.getn(GMPanelDB.favs) do
			local e = GMPanel.byId[GMPanelDB.favs[i]]
			if e and (q == "" or CommandScore(e, q)) then tinsert(out, { kind = "cmd", e = e }) end
		end
	elseif cat == "all" or cat == nil then
		if q == "" then
			tinsert(out, { kind = "header", label = "Favourites" })
			for i = 1, table.getn(GMPanelDB.favs) do
				local e = GMPanel.byId[GMPanelDB.favs[i]]
				if e then tinsert(out, { kind = "cmd", e = e }) end
			end
			if table.getn(GMPanelDB.recent) > 0 then
				tinsert(out, { kind = "header", label = "Recent" })
				AddRecent(out)
			end
		else
			-- Strong command matches lead; fuzzy-only ones go after the name tables.
			AddCommands(out, q, nil, GROUP_SHOWN, true, STRONG)
			-- Name tables in order of their best match ("frostbolt": the spell before the tome).
			local blocks = {}
			for i = 1, table.getn(GMPANEL_DATASETS) do
				local block = {}
				local best = AddData(block, GMPANEL_DATASETS[i].key, q, GROUP_SHOWN, true)
				if table.getn(block) > 0 then tinsert(blocks, { rows = block, best = best or 0, order = i }) end
			end
			table.sort(blocks, function(a, b)
				if a.best ~= b.best then return a.best > b.best end
				return a.order < b.order
			end)
			for i = 1, table.getn(blocks) do
				for j = 1, table.getn(blocks[i].rows) do tinsert(out, blocks[i].rows[j]) end
			end
			AddCommands(out, q, nil, GROUP_SHOWN, true, nil, STRONG)
		end
	else
		AddCommands(out, q, cat, 1000, false)
		for i = 1, table.getn(GMPANEL_DATASETS) do
			local d = GMPANEL_DATASETS[i]
			if d.cat == cat then
				if d.key == "quests" and q == "" then
					local log = QuestLogRows()
					if table.getn(log) > 0 then
						tinsert(out, { kind = "header", label = "Your quest log" })
						for j = 1, table.getn(log) do tinsert(out, { kind = "data", ds = "quests", row = log[j] }) end
					end
				elseif q ~= "" or d.key == "tele" then
					AddData(out, d.key, q, SCOPE_MAX, true)
				end
			end
		end
	end
	return out
end

-- The picker list for one parameter.
function GMPanel_SearchPicker(ds, text)
	local out = {}
	local q = string.lower(text or "")
	if ds == "quests" and q == "" then
		local log = QuestLogRows()
		if table.getn(log) > 0 then tinsert(out, { kind = "header", label = "Your quest log" }) end
		for j = 1, table.getn(log) do tinsert(out, { kind = "data", ds = ds, row = log[j], pick = true }) end
		return out
	end
	AddData(out, ds, q, SCOPE_MAX, false)
	for i = 1, table.getn(out) do out[i].pick = true end
	return out
end
