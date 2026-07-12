//! `DepthTestedDecalExt` — the arena's depth-TESTED forward-decal material extension: a near-verbatim
//! fork of bevy_pbr 0.18.1's [`bevy::pbr::decal::ForwardDecalMaterialExt`] with EXACTLY ONE
//! divergence — it does NOT force `depth_compare = Always` in `specialize`.
//!
//! WHY THE FORK (bug A): stock `ForwardDecalMaterialExt::specialize` (bevy_pbr 0.18.1
//! `decal/forward.rs:128`) overrides the pipeline's depth comparison to `CompareFunction::Always`, so
//! the decal's flush quad draws OVER ANY nearer opaque geometry — the frost ground painted itself
//! straight THROUGH the rolling glacier boulder (a verified-opaque sphere). Leaving the forward pass's
//! standard depth comparison in place (bevy is reverse-Z, so the default is `GreaterEqual`) makes
//! nearer opaque geometry occlude the decal again: the boulder correctly hides the frost behind it.
//! The decal still draws OVER the floor because `client/surfaces.rs` ground-snaps it a `+0.01` bias
//! ABOVE the floor — that 1 cm now doubles as the depth-test-winning z-offset vs the floor plane.
//!
//! SHADER: no vendored wgsl. The extension reuses bevy's already-loaded `bevy_pbr::decal::forward`
//! shader LIBRARY (registered by `ForwardDecalPlugin`, which `PbrPlugin` always adds) exactly as the
//! stock extension does: `StandardMaterial`'s `pbr.wgsl` `#import`s `get_forward_decal_info` and runs
//! the decal projection under `#ifdef FORWARD_DECAL` (pbr.wgsl:33-34,59-60), and `specialize` below
//! pushes that `FORWARD_DECAL` shader-def. The binding-200 uniform reuses bevy's public
//! [`ForwardDecalMaterialExtUniform`] so the GPU layout is byte-identical to the library's declaration.
//!
//! Registered `MaterialPlugin::<DepthTestedDecalMaterial>` in `client/app_windowed.rs` (windowed-only,
//! exactly like the stock guarded decal plugin add); `client/surfaces.rs` builds the decal on it.

use bevy::pbr::decal::ForwardDecalMaterialExtUniform;
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
    StandardMaterial,
};
use bevy::prelude::*;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    AsBindGroup, AsBindGroupShaderType, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::render::texture::GpuImage;

/// The arena forward decal material: a [`StandardMaterial`] extended with [`DepthTestedDecalExt`].
/// Drop-in replacement for bevy's `ForwardDecalMaterial<StandardMaterial>`, minus the depth override.
pub type DepthTestedDecalMaterial = ExtendedMaterial<StandardMaterial, DepthTestedDecalExt>;

/// A [`MaterialExtension`] clone of bevy's `ForwardDecalMaterialExt` (same `depth_fade_factor`
/// uniform, same shader path) that leaves the forward pass's STANDARD depth test in place so decals
/// are occluded by nearer opaque geometry. See the module docs for the one-line divergence.
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
#[uniform(200, ForwardDecalMaterialExtUniform)]
pub struct DepthTestedDecalExt {
    /// Distance (metres) over which the decal fades to full opacity against the surface it projects
    /// onto — bevy's `ForwardDecalMaterialExt::depth_fade_factor`, semantics unchanged.
    pub depth_fade_factor: f32,
}

impl Default for DepthTestedDecalExt {
    fn default() -> Self {
        // Bevy's own default; `client/surfaces.rs` sets 1.0 explicitly at build.
        Self {
            depth_fade_factor: 8.0,
        }
    }
}

// Produce the binding-200 GPU uniform exactly as bevy's stock extension does (identical inverse-fade
// math) so the reused `bevy_pbr::decal::forward` shader library reads a byte-identical layout.
impl AsBindGroupShaderType<ForwardDecalMaterialExtUniform> for DepthTestedDecalExt {
    fn as_bind_group_shader_type(
        &self,
        _images: &RenderAssets<GpuImage>,
    ) -> ForwardDecalMaterialExtUniform {
        ForwardDecalMaterialExtUniform {
            inv_depth_fade_factor: 1.0 / self.depth_fade_factor.max(0.001),
        }
    }
}

impl MaterialExtension for DepthTestedDecalExt {
    fn alpha_mode() -> Option<AlphaMode> {
        // Same as stock: decals alpha-blend with the surface beneath them.
        Some(AlphaMode::Blend)
    }

    fn enable_shadows() -> bool {
        // Same as stock: a projected decal must not cast shadows.
        false
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // THE FORK: bevy's `ForwardDecalMaterialExt::specialize` sets
        //   descriptor.depth_stencil.as_mut().unwrap().depth_compare = CompareFunction::Always;
        // here — the line that made the frost draw through the boulder. We DELIBERATELY OMIT it,
        // leaving the forward pass's standard depth comparison so nearer opaque geometry occludes.
        // Everything below is byte-for-byte identical to the stock extension.

        // Enable the decal projection branch in `StandardMaterial`'s pbr.wgsl (imports + runs
        // `bevy_pbr::decal::forward::get_forward_decal_info`). Without this the binding-200 uniform is
        // unused and the quad renders as a plain flat sprite.
        descriptor.vertex.shader_defs.push("FORWARD_DECAL".into());
        if let Some(fragment) = &mut descriptor.fragment {
            fragment.shader_defs.push("FORWARD_DECAL".into());
        }

        if let Some(label) = &mut descriptor.label {
            *label = format!("depth_tested_decal_{label}").into();
        }

        Ok(())
    }
}
