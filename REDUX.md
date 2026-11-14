# benilla-redux

benilla-redux is a fork of [benilla](https://github.com/samwhosung/benilla) (`upstream`), kept so
that upstream changes can always be pulled in with a trivial `git merge upstream/main`. Every
change in this repository follows the rules below. They take precedence over upstream's
`docs/METHOD.md` and `docs/CONTRIBUTING.md`, whose 1.12.1-fidelity rules govern upstream benilla,
not redux's own features.

## Remotes

- `origin` is the fork, `j-tucc/benilla-redux`: push here, open pull requests here.
- `upstream` is `samwhosung/benilla`: fetch and merge from it, never push to it.
- Pull upstream with `git fetch upstream && git merge upstream/main`. If that merge stops on a
  conflict, a patch below has grown too big: shrink it while you resolve the conflict.

## Rule 1: redux code lives in redux-owned files

- Redux features go in redux-owned crates and files, never woven into upstream's code. New crates
  go under `redux/`, and redux binaries start the app through `benilla_app::run_with` the way
  `crates/benilla-app/examples/extended_launcher.rs` does.
- Redux settings are the redux crates' own, never new rows in upstream's CVar table, unless a
  recorded patch below says why that can't be avoided.
- Redux docs live in this file and under `redux/`. Upstream's docs stay as upstream ships them.

## Rule 2: changes to upstream files are tiny, recorded patches

A change to a file that upstream owns (anything that is not redux-owned) is allowed only when the
feature cannot hook in from outside. When it can't:

- Make the smallest possible patch: a hook, a `pub`, a plugin slot, a feature flag. Never the
  feature itself.
- Put each patch in its own commit, prefixed `patch(upstream):`, saying what the patch is and
  why redux-owned code can't do it.
- Record it in the patch ledger below, in the same commit.
- Don't reformat, reorder or rename anything in an upstream file. Every changed line is a future
  merge conflict.
- Leave upstream's `Cargo.toml` and `Cargo.lock` dependency resolution as it is unless a patch
  records why it has to change.

## Rule 3: nothing from a Blizzard install is committed

Interface code, art, models, sounds and data extracted from a WoW install (1.12.1 or any modern
client) are for local use only. They live in gitignored paths (`benilla-config/`, `overrides/`)
and are never committed to this repository, which is public. Third-party SDKs with their own
licences (for example the Streamline/DLSS SDK) are also local and gitignored unless their licence
allows redistribution.

## Patch ledger

Every upstream file redux changes, with the reason. Keep it current: a patch with no row here is
a bug.

| Upstream file | Lines changed | Why it can't live in redux-owned code |
|---|---|---|
| `AGENTS.md` | +6 | Points agents at this file before upstream's rules load. |
