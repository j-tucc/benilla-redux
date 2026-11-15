#!/usr/bin/env python3
"""Run benilla-redux headless and list the screenshots it wrote.

The client opens no visible window, takes no focus and never goes fullscreen, so it is safe to run
while someone works at the same desk. See redux/headless/README.md.

    python redux/headless/shoot.py                                # in-world as Claude, two shots
    python redux/headless/shoot.py --login-only --shots 6         # the login screen
    python redux/headless/shoot.py --shots world+10 --lua "TargetUnit('player')"

Prints one PNG path per line after `PNG:`; open them with an image viewer (agents: the Read tool).
"""

import argparse
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_DATA = r"D:\WoW Vanilla\Data"
DEFAULT_ACCOUNT_ENV = r"C:\Users\Nick\Workspace\vmangos\claude-test-account.env"
EXE_NAME = "benilla-headless.exe" if os.name == "nt" else "benilla-headless"
# Accounts a scripted run must never use: logging in kicks whoever is playing on them.
FORBIDDEN_ACCOUNTS = {"PLAYER"}
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def find_exe(explicit):
    if explicit:
        return Path(explicit)
    if os.environ.get("REDUX_HEADLESS_EXE"):
        return Path(os.environ["REDUX_HEADLESS_EXE"])
    candidates = []
    for profile in ("release", "play", "debug"):
        candidates.append(HERE / "target" / profile / EXE_NAME)
        candidates.append(HERE.parents[1] / "target" / profile / EXE_NAME)
    # A linked worktree builds into the main checkout's target (CARGO_TARGET_DIR) by convention.
    try:
        common = subprocess.run(
            ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
            cwd=HERE, capture_output=True, text=True, check=True,
        ).stdout.strip()
        for profile in ("release", "play", "debug"):
            candidates.append(Path(common).parent / "target" / profile / EXE_NAME)
    except (OSError, subprocess.CalledProcessError):
        pass
    found = [c for c in candidates if c.is_file()]
    if not found:
        sys.exit(
            f"no {EXE_NAME} found; build it:\n"
            f"  cargo build --release --manifest-path {HERE / 'Cargo.toml'}\n"
            "or pass --exe / set REDUX_HEADLESS_EXE"
        )
    return max(found, key=lambda p: p.stat().st_mtime)


def account_lock(timeout):
    """Holds a machine-wide lock while a run is logged in: two clients on one account kick each other.
    The OS drops it when this process dies, so a crash leaves no stale lock."""
    path = Path(tempfile.gettempdir()) / "redux-headless-account.lock"
    handle = open(path, "a+")
    deadline = time.time() + timeout
    announced = False
    while True:
        try:
            if os.name == "nt":
                import msvcrt
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return handle
        except OSError:
            if time.time() > deadline:
                sys.exit(f"another shoot.py run still holds {path} after {timeout:.0f}s")
            if not announced:
                print(f"waiting for another shoot.py run to finish (lock {path})")
                announced = True
            time.sleep(1)


def read_account(path):
    values = {}
    for line in Path(path).read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            values[key.strip()] = value.strip().strip('"')
    if "WOW_USER" not in values or "WOW_PASS" not in values:
        sys.exit(f"{path} must set WOW_USER and WOW_PASS")
    return values


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--shots", default="world+8,world+15",
                    help="REDUX_SHOT_AT: comma list of <secs> since launch or world+<secs> (default %(default)s)")
    ap.add_argument("--every", type=float, help="REDUX_SHOT_EVERY: also a shot every N seconds")
    ap.add_argument("--exit", type=float, default=120,
                    help="hard time limit in seconds (WOW_PROBE_EXIT_AT, default %(default)s)")
    ap.add_argument("--keep-running", action="store_true",
                    help="do not exit early once the --shots are written; run until --exit")
    ap.add_argument("--login-only", action="store_true",
                    help="do not log in: stay on the login screen")
    ap.add_argument("--lua", help="WOW_PROBE_LUA: a Lua chunk to run in the in-game UI after world entry; "
                                  "it may call Screenshot() for an extra shot")
    ap.add_argument("--lua-at", type=float, help="WOW_PROBE_LUA_AT: seconds after world entry (default 10)")
    ap.add_argument("--glue-lua", help="WOW_PROBE_GLUE_LUA: a Lua chunk to run in the login-screen UI")
    ap.add_argument("--glue-lua-at", type=float, help="WOW_PROBE_GLUE_LUA_AT: seconds after launch (default 10)")
    ap.add_argument("--size", default="1280x720", help="window size in pixels (WOW_WIN, default %(default)s)")
    ap.add_argument("--home", help="BENILLA_HOME (default: a fresh scratch folder)")
    ap.add_argument("--out", help="REDUX_SHOT_DIR (default: <home>/Screenshots)")
    ap.add_argument("--data", default=os.environ.get("WOW_DATA", DEFAULT_DATA), help="WOW_DATA")
    ap.add_argument("--host", default="127.0.0.1", help="WOW_HOST (default %(default)s)")
    ap.add_argument("--account-env", default=DEFAULT_ACCOUNT_ENV, help="file with WOW_USER= and WOW_PASS=")
    ap.add_argument("--char", default="Claude", help="WOW_CHAR (default %(default)s)")
    ap.add_argument("--lock-wait", type=float, default=600,
                    help="seconds to wait for another logged-in shoot.py run to finish (default %(default)s)")
    ap.add_argument("--exe", help="the benilla-headless binary (default: newest built one)")
    ap.add_argument("--env", action="append", default=[], metavar="NAME=VALUE",
                    help="extra environment for the client, repeatable (e.g. --env WOW_GM=off)")
    args = ap.parse_args()

    exe = find_exe(args.exe)
    home = Path(args.home) if args.home else Path(tempfile.mkdtemp(prefix="redux-headless-"))
    home.mkdir(parents=True, exist_ok=True)
    out = Path(args.out) if args.out else home / "Screenshots"
    config = home / "config.toml"
    if not config.exists():
        # A fresh home would open on the EULA and TOS screens; mark them read, as a played install has.
        config.write_text('[cvars]\nreadEULA = "1"\nreadTOS = "1"\n')
    log_path = home / "run.log"

    env = {k: v for k, v in os.environ.items()
           if not (k.startswith("WOW_") or k.startswith("REDUX_"))}
    env.update(
        BENILLA_HOME=str(home),
        WOW_DATA=args.data,
        WOW_HOST=args.host,
        WOW_UNATTENDED="1",
        WOW_NOSOUND="1",
        WOW_BG="1",
        WOW_WIN=args.size,
        # WOW_WIN in physical pixels whatever the display's scaling.
        WOW_DPI="1",
        WOW_PROBE_EXIT_AT=str(args.exit),
        REDUX_HEADLESS="1",
        REDUX_SHOT_AT=args.shots,
        REDUX_SHOT_DIR=str(out),
    )
    if not args.keep_running:
        env["REDUX_SHOT_EXIT"] = "1"
    if args.every:
        env["REDUX_SHOT_EVERY"] = str(args.every)
    if not args.login_only:
        account = read_account(args.account_env)
        if account["WOW_USER"].upper() in FORBIDDEN_ACCOUNTS:
            sys.exit(f"refusing to log in as {account['WOW_USER']}: that is a player's account")
        env.update(WOW_USER=account["WOW_USER"], WOW_PASS=account["WOW_PASS"], WOW_CHAR=args.char)
    if args.lua:
        env["WOW_PROBE_LUA"] = args.lua
        if args.lua_at is not None:
            env["WOW_PROBE_LUA_AT"] = str(args.lua_at)
    if args.glue_lua:
        env["WOW_PROBE_GLUE_LUA"] = args.glue_lua
        if args.glue_lua_at is not None:
            env["WOW_PROBE_GLUE_LUA_AT"] = str(args.glue_lua_at)
    for item in args.env:
        name, _, value = item.partition("=")
        env[name] = value

    lock = None if args.login_only else account_lock(args.lock_wait)
    print(f"exe:  {exe}")
    print(f"home: {home}")
    print(f"log:  {log_path}")
    started = time.time()
    flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
    with open(log_path, "wb") as log:
        proc = subprocess.Popen([str(exe)], env=env, cwd=str(home), stdout=log,
                                stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
                                creationflags=flags)
        try:
            code = proc.wait(timeout=args.exit + 45)
        except subprocess.TimeoutExpired:
            proc.kill()
            code = proc.wait()
            print("client overran its time limit and was killed")
    if lock:
        lock.close()
    print(f"exit: {code} after {time.time() - started:.0f}s")

    text = log_path.read_text(errors="replace")
    for line in text.splitlines():
        if ("redux-headless" in line or "panicked" in line or " ERROR " in line
                or "preflight" in line.lower() or "net: disconnected" in line):
            print("  " + ANSI.sub("", line).strip()[:240])
    pngs = sorted(out.glob("*.png"), key=lambda p: p.stat().st_mtime) if out.is_dir() else []
    pngs = [p for p in pngs if p.stat().st_mtime >= started - 1]
    if "net: disconnected" in text:
        print("WARNING: the server dropped the session mid-run; if mangosd logged another CLAUDE login, "
              "a run outside shoot.py was using the account. Shots after the drop show a reconnect.")
    if not pngs:
        print("no screenshots written; see the log")
        return 1
    for p in pngs:
        print(f"PNG: {p}")
    return 0 if code == 0 else 2


if __name__ == "__main__":
    sys.exit(main())
