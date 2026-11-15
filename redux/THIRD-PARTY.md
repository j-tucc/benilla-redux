# Third-party code in benilla-redux's own crates

redux's graphics (`redux/gfx`, `redux-gfx`) are ported from Elderoth's `elderoth-gfx`
(`j-tucc/elderoth`), which ported them from the projects below. Their attributions travel with
the code.

## The everwood graphics fork of Benilla

- **Project: [benilla-everwood_graphics](https://github.com/pkuzic/benilla-everwood_graphics)**, a
  graphics fork of Benilla, MIT or Apache-2.0 like Benilla itself, at revision `cdc7130b`.

Each ported file names its everwood source in its header. The engine carries only the generic hook
points they fill (REDUX.md's patch ledger: the `gfx` hook, foliage-wind, lit/shadow-caster and
surface patches).

- the shadow receiver and rig (`redux/gfx/src/shadow.wgsl`, `shadows.rs`, from everwood's
  `shadow_hook.wgsl` and `shadow_core.rs`);
- moonlight (`moonlight.rs`, `moon.wgsl`, from `lighting/moonlight.rs` and `moonlight_hook.wgsl`);
- the light shafts (`shafts.rs`, `shafts.wgsl`, from `volumetric_light.rs` and `.wgsl`);
- the ambient occlusion (`ssao.rs`, `ssao.wgsl`, from `ssao.rs` and `shaders/ssao.wgsl`).

The shadow filter is Bevy 0.18's `sample_shadow_map_castano_thirteen` (MIT or Apache-2.0). The
shafts' noise is Jimenez's interleaved gradient noise (2014). The ground mist (`mist.rs`) is a
textbook analytic height fog and takes no code from any project.

## NVIDIA Streamline, DLSS and Reflex (user-supplied, never committed)

- **Project: [NVIDIA Streamline](https://github.com/NVIDIA-RTX/Streamline)**, the SDK for DLSS
  Super Resolution, DLSS Frame Generation and Reflex.

A build with the `dlss` feature (`redux/gfx/src/dlss/`) holds no NVIDIA code: a hand-written FFI
to Streamline's C API, laid out from its headers. Its libraries are loaded at run time from a
Streamline release the user extracts into the gitignored `redux/overrides/streamline/<tag>/` (or
the folder `REDUX_STREAMLINE` names). Nothing from the SDK is committed or redistributed: not
`sl.interposer.dll` or the other `sl.*.dll` plugins, not NVIDIA's NGX libraries
(`nvngx_dlss.dll`, `nvngx_dlssg.dll`) nor `NvLowLatencyVk.dll`. Streamline's own source and
plugins are MIT-licensed (its `license.txt`); the NGX libraries are under the NVIDIA RTX SDKs
licence and Reflex under NVIDIA's SDK licence, both shipped beside them in its `bin/x64`. A release
that bundles them must follow those licences, NVIDIA's RTX UI guidelines and its signing
requirements.

## WarcraftXL

- **Project: [WarcraftXL](https://github.com/WarcraftXL)**: client extensions for the **3.3.5a**
  (build 12340) client, written in C++ with D3D9 / HLSL.
- **Author: iThorgrim.**

Everwood ported parts of WarcraftXL on the condition that the author and the original project are
named; Elderoth and redux keep that condition for the code taken from everwood. This file, and the
header line in each file below, is that attribution; keep both wherever the code is copied or
shared.

| redux file | WarcraftXL module | Source files | What was taken |
|---|---|---|---|
| `redux/gfx/src/grading.rs` + `grading.wgsl` | `wxl-retail-grading` | `Grading.cpp`, `shaders/Grading.ps.hlsl` | 32³ LUT convention, strength control and lookup mathematics, the cube uploaded as a native 3D texture (through everwood; used with the author's permission, attribution required). The cubes are Elderoth's own. |
| `redux/gfx/src/sky.wgsl` | `wxl-retail-clouds` | `Clouds.cpp`, `Clouds.hpp` | The billowed, domain-warped cloud detail (`cloud_billow`, `cloud_detail`), its drift rates, and the short sun march with its ambient floor (through everwood). |
| `redux/gfx/src/sky.rs` | `wxl-retail-clouds` | `Clouds.cpp` | The cloud march's direction and strength from the glow body's height (through everwood). |
| `redux/gfx/src/wind.rs` | `wxl-experimental-wind` | `field/Wind.hpp`, `field/Wind.cpp` | The wind profile defaults, the seeded three-sine wander, the gust shaping and the weather coupling (through everwood). |
| `redux/gfx/src/wind.wgsl` | `wxl-experimental-wind` | `grass/GrassWind.hpp`, `grass/GrassWind.cpp` | The grass's two travelling waves, gust response, downwind lean and the radial parting around the player (through everwood). |
| `crates/benilla-world/src/clutter.rs` (foliage-wind patch) | `wxl-experimental-wind` | `grass/GrassWind.hpp`, `grass/GrassWind.cpp` | The per-tuft bend weight and phase (through everwood). |
