# redux-headless: screenshots without a window

`benilla-headless` is the client with one plugin on top (`src/lib.rs`). With `REDUX_HEADLESS=1`
the window is created hidden: it never shows, never takes focus, never appears in the taskbar and
never goes fullscreen. The client still renders the full frame, world and UI, and writes PNGs on a
schedule. Use it for agent test runs on a machine someone is working at.

## Build

```bash
cargo build --release --manifest-path redux/headless/Cargo.toml
```

It is its own Cargo workspace (upstream's root manifest stays untouched). From a linked worktree,
add `CARGO_TARGET_DIR=<main checkout>/target` to reuse the main checkout's build.

## Run: `shoot.py`

```bash
python redux/headless/shoot.py                          # log in as Claude, shots at world+8s, world+15s
python redux/headless/shoot.py --login-only --shots 8   # the login screen
python redux/headless/shoot.py --shots world+12 --lua "ToggleCharacter('PaperDollFrame')" --lua-at 5
```

It makes a scratch `BENILLA_HOME`, uses `WOW_DATA=D:\WoW Vanilla\Data`, `WOW_HOST=127.0.0.1` and
the CLAUDE account from `C:\Users\Nick\Workspace\vmangos\claude-test-account.env` with
`WOW_CHAR=Claude` (it refuses the PLAYER account), runs the client silent and unattended, waits for
it to exit and prints each screenshot as `PNG: <path>`. Agents open those with the Read tool. The
log is `<home>/run.log`.

| Option | Meaning |
|---|---|
| `--shots` | `REDUX_SHOT_AT`, comma list of `<secs>` since launch or `world+<secs>` after world entry |
| `--every N` | also a shot every N seconds |
| `--exit N` | hard limit in seconds (default 120); the run otherwise ends once every `--shots` shot is on disk |
| `--keep-running` | run until `--exit` even after the shots |
| `--login-only` | don't log in |
| `--lua CHUNK`, `--lua-at S` | Lua in the in-game UI S seconds after world entry (default 10); `Screenshot()` in it writes an extra `WoWScrnShot_*.png` |
| `--glue-lua CHUNK`, `--glue-lua-at S` | Lua in the login-screen UI |
| `--size WxH` | frame size in pixels (default 1280x720) |
| `--home`, `--out` | `BENILLA_HOME` and the PNG folder (default `<home>/Screenshots`) |
| `--env NAME=VALUE` | any other client env, e.g. `--env WOW_GM=off` |

Timing: our player appears about 2 s after launch, but the loading screen stays up about 8 s more
while the world streams in, so in-world shots want `world+10` or later. `world+` counts from the
latest entry, so after a reconnect the remaining shots wait for the new one.

One account, one client: a second login to CLAUDE kicks the first, and both clients then
reconnect and kick each other every few seconds (the log shows `net: disconnected`, and
`shoot.py` prints a warning). `shoot.py` runs queue on a machine-wide lock (`--lock-wait`), but a
client started any other way on CLAUDE still collides.

## The switches, for a hand-rolled run

| Env | Meaning |
|---|---|
| `REDUX_HEADLESS=1` | hide the window; without it the binary is the ordinary client |
| `REDUX_SHOT_AT` | as `--shots` |
| `REDUX_SHOT_EVERY` | as `--every` |
| `REDUX_SHOT_DIR` | PNG folder (default `<BENILLA_HOME>/Screenshots`) |
| `REDUX_SHOT_EXIT=1` | exit once every `REDUX_SHOT_AT` shot is written |

Files are `shot-<NNN>-<label>.png` (`shot-001-t8.0s.png`, `shot-002-world+15.0s.png`). Pair them
with upstream's `WOW_UNATTENDED=1 WOW_NOSOUND=1 WOW_WIN=1280x720 WOW_DPI=1 WOW_PROBE_EXIT_AT=<secs>`
and the login variables, as `shoot.py` does.
