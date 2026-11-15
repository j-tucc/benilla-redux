#!/usr/bin/env python3
"""Write GMPanel_Data.lua: the name tables the /gm panel searches.

The rows come from the local vmangos world DB (items, teleports, creatures, quests, spells) and the
leaf command paths from vmangos's Chat.cpp. The output goes into the installed addon only; it is
never committed (REDUX.md rule 3: it is game data).

Each table keeps the row the server loads: the highest `patch <= WowPatch` per entry
(ObjectMgr.cpp), and for spells the highest `build <= 5875`.
"""

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

DEFAULT_VMANGOS = Path(r"C:\Users\Nick\Workspace\vmangos")
BLOCK = 1500  # rows per Lua closure, so no one function holds too many constants

SEC = {
    "SEC_PLAYER": 0, "SEC_MODERATOR": 1, "SEC_TICKETMASTER": 2, "SEC_GAMEMASTER": 3,
    "SEC_BASIC_ADMIN": 4, "SEC_DEVELOPER": 5, "SEC_ADMINISTRATOR": 6, "SEC_CONSOLE": 7,
}


def query(mariadb, args, db, sql):
    cmd = [str(mariadb), f"-h{args.db_host}", f"-P{args.db_port}", f"-u{args.db_user}",
           f"-p{args.db_pass}", "--default-character-set=utf8mb4", "-B", "-N", "-r", db, "-e", sql]
    out = subprocess.run(cmd, capture_output=True, check=True).stdout.decode("utf-8", "replace")
    return [line.split("\t") for line in out.splitlines() if line]


def latest(table, cols, patch):
    return (f"SELECT {cols} FROM {table} t WHERE t.patch = (SELECT MAX(u.patch) FROM {table} u "
            f"WHERE u.entry = t.entry AND u.patch <= {patch}) ORDER BY t.entry")


def lua_str(s):
    s = s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "")
    return f'"{s}"'


def lua_val(v):
    return str(v) if isinstance(v, int) else lua_str(v)


def emit(out, name, rows):
    out.append(f"GMPanel_Data.{name} = {{}}")
    for i in range(0, len(rows), BLOCK):
        body = ",\n".join("{" + ",".join(lua_val(v) for v in r) + "}" for r in rows[i:i + BLOCK])
        out.append(f"GMPanel_DataAppend(GMPanel_Data.{name}, (function() return {{\n{body}\n}} end)())")


def chat_commands(chat_cpp):
    """Every leaf command path with its security, walked from the root `commandTable`."""
    src = chat_cpp.read_text(encoding="utf-8", errors="replace")
    tables = {}
    for m in re.finditer(r"static ChatCommand (\w+)\[\]\s*=\s*\{(.*?)\n\s*\};", src, re.S):
        rows = re.findall(r'\{\s*"([^"]*)"\s*,\s*(SEC_\w+)\s*,\s*\w+\s*,\s*([^,]+?)\s*,\s*"[^"]*"\s*,\s*(\w+)\s*\}',
                          m.group(2))
        tables[m.group(1)] = rows
    out = []

    def walk(table, prefix, depth):
        for name, sec, handler, sub in tables.get(table, []):
            path = (prefix + " " + name).strip()
            if sub != "nullptr" and sub in tables and depth < 6:
                walk(sub, path, depth + 1)
            elif handler != "nullptr" and path:
                out.append((path, SEC.get(sec, 7)))

    walk("commandTable", "", 0)
    seen, uniq = set(), []
    for path, sec in out:
        if path not in seen:
            seen.add(path)
            uniq.append((path, sec))
    return sorted(uniq)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--out", required=True, help="path of GMPanel_Data.lua to write")
    ap.add_argument("--vmangos", default=os.environ.get("VMANGOS_DIR", str(DEFAULT_VMANGOS)))
    ap.add_argument("--db-host", default="127.0.0.1")
    ap.add_argument("--db-port", default="3306")
    ap.add_argument("--db-user", default="mangos")
    ap.add_argument("--db-pass", default="mangos")
    ap.add_argument("--world-db", default="mangos")
    ap.add_argument("--patch", type=int, default=10, help="mangosd.conf WowPatch (default 10)")
    args = ap.parse_args()

    vm = Path(args.vmangos)
    mariadb = next((p for p in (vm / "db/bin/mariadb.exe", vm / "db/bin/mysql.exe") if p.exists()), None)
    if mariadb is None:
        sys.exit(f"no mariadb.exe under {vm / 'db/bin'}")
    w = args.world_db

    items = [(int(e), n, int(q), int(l)) for e, n, q, l in
             query(mariadb, args, w, latest("item_template", "t.entry, t.name, t.quality, t.item_level", args.patch))]
    tele = [(int(i), n, int(m)) for i, n, m in
            query(mariadb, args, w, "SELECT id, name, map FROM game_tele ORDER BY name")]
    creatures = [(int(e), n, int(a), int(b), s) for e, n, a, b, s in
                 query(mariadb, args, w, latest("creature_template",
                                                "t.entry, t.name, t.level_min, t.level_max, t.subname", args.patch))]
    quests = [(int(e), n, int(l)) for e, n, l in
              query(mariadb, args, w, latest("quest_template", "t.entry, t.Title, t.QuestLevel", args.patch))]
    spells = [(int(e), n, r) for e, n, r in
              query(mariadb, args, w,
                    "SELECT t.entry, t.name, t.nameSubtext FROM spell_template t WHERE t.build = "
                    "(SELECT MAX(u.build) FROM spell_template u WHERE u.entry = t.entry AND u.build <= 5875) "
                    "AND t.name <> '' ORDER BY t.entry")]
    commands = chat_commands(vm / "core/src/game/Chat/Chat.cpp")

    out = ["-- Generated by redux/gm/gen_data.py from the local vmangos DB. Do not commit.",
           "GMPanel_Data = { patch = %d }" % args.patch,
           "function GMPanel_DataAppend(dst, src) local n = table.getn(dst) "
           "for i = 1, table.getn(src) do dst[n + i] = src[i] end end"]
    emit(out, "items", items)
    emit(out, "tele", tele)
    emit(out, "creatures", creatures)
    emit(out, "quests", quests)
    emit(out, "spells", spells)
    emit(out, "commands", commands)
    dst = Path(args.out)
    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.write_text("\n".join(out) + "\n", encoding="utf-8")
    print(f"{dst}: {len(items)} items, {len(tele)} teleports, {len(creatures)} creatures, "
          f"{len(quests)} quests, {len(spells)} spells, {len(commands)} commands")


if __name__ == "__main__":
    main()
