#!/usr/bin/env python3
"""Install the GM Panel addon into an AddOns folder and generate its name tables.

Default target: <main checkout>/benilla-config/AddOns/GMPanel (gitignored). The addon's own files
are copied from redux/gm/GMPanel; GMPanel_Data.lua is written by gen_data.py from the local
vmangos DB. Without a reachable DB the panel still works and searches through `.lookup` replies.
"""

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SRC = HERE / "GMPanel"


def main_checkout():
    out = subprocess.run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
                         cwd=HERE, capture_output=True, text=True, check=True).stdout.strip()
    return Path(out).parent


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--addons", help="AddOns folder (default: <main checkout>/benilla-config/AddOns)")
    ap.add_argument("--no-data", action="store_true", help="copy the addon only")
    ap.add_argument("--vmangos", help="vmangos folder, passed to gen_data.py")
    args = ap.parse_args()

    addons = Path(args.addons) if args.addons else main_checkout() / "benilla-config" / "AddOns"
    dst = addons / "GMPanel"
    dst.mkdir(parents=True, exist_ok=True)
    for f in SRC.iterdir():
        if f.is_file():
            shutil.copy2(f, dst / f.name)
    print(f"addon: {dst}")
    data = dst / "GMPanel_Data.lua"
    if not args.no_data:
        cmd = [sys.executable, str(HERE / "gen_data.py"), "--out", str(data)]
        if args.vmangos:
            cmd += ["--vmangos", args.vmangos]
        if subprocess.run(cmd).returncode != 0:
            print("data: not generated; the panel falls back to .lookup replies", file=sys.stderr)
    if not data.exists():
        # The .toc names the file; an empty one keeps the load log clean.
        data.write_text("-- No name tables: run redux/gm/gen_data.py. The panel uses .lookup replies.\n",
                        encoding="utf-8")


if __name__ == "__main__":
    main()
