//! The program's per-frame graphics data (an Elderoth engine patch).
//!
//! One resource, [`GfxData`], that the program writes and the engine never reads: the light packer
//! copies it every frame into [`GFX_ROWS`] rows appended to the shared light buffer right after
//! the point-light table, so the offsets of every row before it never move. The WGSL mirror is
//! `benilla::gfx_data::GfxData`; every receiver's `WowLight` struct declares it as its `gfx`
//! member and passes it to the program's `benilla::gfx_hooks` bodies. What the rows mean is the
//! program's: the stock hooks ignore them, and all zero is the default.
//!
//! The block has room for twice what the program uses, so it can hold the previous frame's rows
//! beside the current ones (a motion-vector pass re-running a vertex hook) without a new binding.

use bevy::prelude::*;

/// Rows in the appended block.
pub const GFX_ROWS: usize = 64;

/// The per-frame rows, `vec4<f32>` each.
#[repr(C)]
#[derive(Resource, Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GfxData {
    pub rows: [[f32; 4]; GFX_ROWS],
}

impl Default for GfxData {
    fn default() -> Self {
        Self {
            rows: [[0.0; 4]; GFX_ROWS],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rust_and_wgsl_blocks_agree() {
        let src = include_str!("../../../benilla-assets/src/shaders/gfx_data.wgsl");
        assert!(src.contains(&format!("rows: array<vec4<f32>, {GFX_ROWS}>")));
    }
}
