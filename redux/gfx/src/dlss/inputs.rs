//! What DLSS Super Resolution and Frame Generation both hand Streamline: the frame's camera
//! constants (`sl::Constants`) and wgpu textures as Streamline's Vulkan resources.
use super::sl;
use crate::taa::MotionRows;
use ash::vk::{self, Handle};
use bevy::{
    math::DMat4,
    prelude::*,
    render::render_resource::{TextureFormat, TextureUsages},
};
use wgpu::hal::api::Vulkan;

/// The frame's constants (`sl::Constants`): the unjittered camera from the motion rows, its
/// world frame, the jitter and whether the history is dropped.
pub(super) fn constants(
    world_from_view: &GlobalTransform,
    jitter: Vec2,
    rows: &MotionRows,
    reset: bool,
) -> sl::Constants {
    let (now, then) = rows.matrices();
    let clip_to_prev = then.as_dmat4() * now.as_dmat4().inverse();
    let cols = |m: DMat4| m.as_mat4().to_cols_array();
    let frame = world_from_view.affine().matrix3;
    let position = world_from_view.translation();
    sl::Constants {
        camera_view_to_clip: now.to_cols_array(),
        clip_to_camera_view: cols(now.as_dmat4().inverse()),
        clip_to_prev_clip: cols(clip_to_prev),
        prev_clip_to_clip: cols(clip_to_prev.inverse()),
        jitter_offset: (-jitter).to_array(),
        mvec_scale: [-1.0, -1.0],
        camera_pos: position.to_array(),
        camera_up: frame.y_axis.to_array(),
        camera_right: frame.x_axis.to_array(),
        camera_fwd: (-frame.z_axis).to_array(),
        // Bevy's infinite reverse-Z perspective: near in `w_axis.z`, 1 / tan(fov / 2) in `y_axis.y`.
        camera_near: now.w_axis.z,
        camera_far: benilla_world::view::CAM_FAR,
        camera_fov: 2.0 * (1.0 / now.y_axis.y).atan(),
        camera_aspect_ratio: now.y_axis.y / now.x_axis.x,
        depth_inverted: sl::TRUE,
        camera_motion_included: sl::TRUE,
        reset: if reset { sl::TRUE } else { sl::FALSE },
        ..default()
    }
}

/// The Vulkan format of a texture DLSS reads or writes.
pub(super) fn vk_format(format: TextureFormat) -> Option<vk::Format> {
    Some(match format {
        TextureFormat::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
        TextureFormat::Rgba32Float => vk::Format::R32G32B32A32_SFLOAT,
        TextureFormat::Rg11b10Ufloat => vk::Format::B10G11R11_UFLOAT_PACK32,
        TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        TextureFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        TextureFormat::Rgb10a2Unorm => vk::Format::A2B10G10R10_UNORM_PACK32,
        TextureFormat::Rg16Float => vk::Format::R16G16_SFLOAT,
        TextureFormat::Depth32Float => vk::Format::D32_SFLOAT,
        _ => return None,
    })
}

/// wgpu-hal's `map_texture_usage` for the usages these textures carry.
fn vk_usage(usage: TextureUsages, depth: bool) -> vk::ImageUsageFlags {
    let mut flags = vk::ImageUsageFlags::empty();
    for (from, to) in [
        (TextureUsages::COPY_SRC, vk::ImageUsageFlags::TRANSFER_SRC),
        (TextureUsages::COPY_DST, vk::ImageUsageFlags::TRANSFER_DST),
        (TextureUsages::TEXTURE_BINDING, vk::ImageUsageFlags::SAMPLED),
        (TextureUsages::STORAGE_BINDING, vk::ImageUsageFlags::STORAGE),
    ] {
        if usage.contains(from) {
            flags |= to;
        }
    }
    if usage.contains(TextureUsages::RENDER_ATTACHMENT) {
        flags |= if depth {
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
        } else {
            vk::ImageUsageFlags::COLOR_ATTACHMENT
        };
    }
    flags
}

/// A wgpu texture as Streamline takes it, in the layout wgpu-hal gives `state`
/// (`derive_image_layout`: sampled colour is shader-read-only, sampled depth is
/// depth-stencil-read-only, storage is general).
pub(super) fn image(
    texture: &wgpu::Texture,
    view: &wgpu::TextureView,
    state: wgpu::TextureUses,
) -> Option<sl::VkImage> {
    let depth = texture.format().is_depth_stencil_format();
    let layout = match (state, depth) {
        (wgpu::TextureUses::RESOURCE, false) => vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        (wgpu::TextureUses::RESOURCE, true) => vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
        _ => vk::ImageLayout::GENERAL,
    };
    // SAFETY: only the handles are read; Bevy's caches keep both alive past this frame's submit.
    let raw_image = unsafe { texture.as_hal::<Vulkan>() }.map(|t| unsafe { t.raw_handle() })?;
    // SAFETY: as above.
    let raw_view = unsafe { view.as_hal::<Vulkan>() }.map(|v| unsafe { v.raw_handle() })?;
    let size = texture.size();
    Some(sl::VkImage {
        image: raw_image.as_raw(),
        view: raw_view.as_raw(),
        layout: layout.as_raw(),
        width: size.width,
        height: size.height,
        format: vk_format(texture.format())?.as_raw(),
        usage: vk_usage(texture.usage(), depth).as_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(now: Mat4, then: Mat4) -> MotionRows {
        MotionRows::from_matrices(now, then)
    }

    #[test]
    fn a_still_camera_has_no_clip_motion_and_bevys_conventions() {
        let projection = Mat4::perspective_infinite_reverse_rh(1.0, 16.0 / 9.0, 0.1);
        // At Northshire (x ≈ −8900).
        let at = GlobalTransform::from_translation(Vec3::new(-8900.0, 80.0, 130.0));
        let c = constants(
            &at,
            Vec2::new(0.25, -0.125),
            &rows(projection, projection),
            true,
        );
        let identity = Mat4::IDENTITY.to_cols_array();
        for (a, b) in c.clip_to_prev_clip.iter().zip(identity) {
            assert!((a - b).abs() < 1e-5);
        }
        assert_eq!(c.jitter_offset, [-0.25, 0.125]);
        assert_eq!(c.mvec_scale, [-1.0, -1.0]);
        assert!((c.camera_near - 0.1).abs() < 1e-6);
        assert!((c.camera_fov - 1.0).abs() < 1e-5);
        assert!((c.camera_aspect_ratio - 16.0 / 9.0).abs() < 1e-5);
        assert_eq!((c.reset, c.depth_inverted), (sl::TRUE, sl::TRUE));
        assert_eq!(c.camera_fwd, [0.0, 0.0, -1.0]);
    }
}
