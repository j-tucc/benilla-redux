# GM Panel (`/gm`)

A searchable window for vmangos GM commands. It is a 1.12 addon (`## Interface: 11200`,
Lua 5.0), written against the 1.12 API alone, and runs in benilla like any other addon.

## Install

```bash
python redux/gm/install.py
```

This copies `redux/gm/GMPanel/` into `<main checkout>/benilla-config/AddOns/GMPanel`. It then
runs `gen_data.py`, which queries the local vmangos DB through `vmangos/db/bin/mariadb.exe`
(127.0.0.1:3306, mangos/mangos) and writes `GMPanel_Data.lua` beside the addon. That file holds
17.7k items, 997 teleports, 10.4k creatures, 4.4k quests, 22.4k spells and the 755 leaf command
paths with their security levels. It is game data, so it stays in the gitignored install and is
never committed (REDUX.md rule 3).

Each table keeps the row the server loads: the highest `patch <= 10` per entry, and for spells
the highest `build <= 5875`.

Options:
- `--addons <dir>` installs somewhere else, for example a scratch `BENILLA_HOME`.
- `--no-data` skips the DB query.

Without the data file, every search offers a "Search the server: .lookup …" row, and links in
the reply feed the result list.

Re-run the installer after changing the addon or the world DB, then `/reload`.

## Use

- **Open and close.** `/gm` toggles the window. `/gm <text>` opens it with a search filled in.
  "Toggle GM panel" can be bound in Key Bindings. `/jtu gm` also works when JTUPrefs is loaded.
  Esc first clears the search, then closes the window.
- **Search.** One box searches commands (fuzzy, with aliases such as summon, appear, kill and
  god) and every name table at once. Prefixes scope it: `i:` items, `t:` teleports, `n:` NPCs,
  `q:` quests, `s:` spells, `.` the raw command list. Tab and Shift-Tab move the selection, and
  Enter runs it.
- **Name rows run the obvious command.**
  - A teleport row runs `.tele <name>`.
  - An item row opens Add item, with the cursor in Count.
  - A creature row opens `.npc summon`.
  - A quest row opens `.quest complete`. The Quests category lists your quest log first.
  - A spell row opens `.learn`.
- **Forms.** Each catalogue command has typed fields. Item, teleport, creature, quest and spell
  fields are pickers: type to search, Tab to move, Enter to pick. The line at the bottom is
  exactly what is sent. You can edit it, and Up and Down recall what you sent before.
- **Target.** The note above the preview says who a command acts on, using vmangos's own rule
  for that command:
  - `.levelup` levels a targeted NPC.
  - `.modify money` fails with an NPC targeted, so the button becomes "Clear target & send".
  - Commands that take a name are given yours when an NPC is targeted.
- **Favourites.** The strip under the search box holds up to 10 one-click commands. Toggles show
  their on/off state, which the panel reads from `.cheat status` and from replies. Right-click a
  command to pin or unpin it.
- **Safety.** Destructive commands (`.die` on yourself, resets, removing items or quests, spawn
  deletes, kick, server shutdown, setting a GM level) need a second press within 3 seconds.
  Spawns that write the world DB are labelled permanent. Commands above your account level are
  greyed out, and the note says the level they need.
- **Replies.** System replies in the 3 seconds after a send appear in the bottom pane. Their item
  links are clickable.

## How it sends

The panel sends commands with `SendChatMessage(text, "SAY")`; vmangos parses the leading dot on
any chat type before routing it.
- **While dead.** The client refuses a dead player's SAY (`ERR_CHAT_WHILE_DEAD`), so the panel
  whispers the command to you instead, and `.revive` works from a ghost.
- **On first open.** It sends `.account` and `.cheat status` to learn your level and toggle
  states, and keeps those two replies out of the chat frame.

## Files

| File | What |
|---|---|
| `GMPanel/Catalog.lua` | about 100 curated commands: path, security, typed parameters, target rule, danger flag. Syntax is read from `vmangos/core/src/game/Commands/*.cpp`, because vmangos ships no help text |
| `GMPanel/Core.lua` | state, sending, reply parsing, favourites and recents (`GMPanelDB`, per account) |
| `GMPanel/Search.lua` | scoring and the grouped result list |
| `GMPanel/Panel.lua` | the window: stock 1.12 dialog look, with a `GMPANEL_THEME` table for a later skin |
| `gen_data.py`, `install.py` | the name-table generator and the installer |

## Testing

```bash
python redux/gm/install.py --addons <scratch>/AddOns
target/play/examples/addon_harness.exe <scratch>/AddOns --probe GMPanel --event VARIABLES_LOADED --eval 'return GMPanel_Search("orgr","all")[2].row[2]'
```

```bash
python redux/headless/shoot.py --account-env <vmangos>/claude-test-account.env --home <scratch> --lua "<chunk>"
```

The live run (CLAUDE account only, hidden window) drives the panel through `GMPanel_SelectCommand`
and `GMPanel_SendForm`, and checks the results with `ProbeLog`.
