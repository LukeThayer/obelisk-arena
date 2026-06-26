//! arena_skills — the .skillfx.ron cosmetic-binding layer for obelisk skills.
//!
//! This crate owns the **SkillFx authoring format** (`.skillfx.ron`) and (in a later task) the
//! binding layer that turns obelisk `CueEvent`s into `LaneEvent`s the game consumes. It mirrors
//! obelisk's own `.cast.ron` loader pattern (`obelisk-bevy/src/assets/mod.rs`) but does NOT touch
//! the sim — it's a pure consumer of `observe_cue`, so it stays render-free and headless-testable.

use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use bevy::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;

/// Authored cosmetic layer for one skill, loaded from `<skill>.skillfx.ron`.
/// Mirrors obelisk's `CastTimeline`: a serde-deserialized RON asset keyed by `skill_id`.
#[derive(Asset, TypePath, Debug, Clone, Deserialize)]
pub struct SkillFx {
    pub skill_id: String,
    /// Maps an obelisk cue slot key — the cue_id VALUE obelisk fires (e.g. `"firebolt_cast"` /
    /// `"firebolt_impact"`, from the `.cast.ron` `vfx_cues` map) — to a lane reaction.
    #[serde(default)]
    pub lanes: HashMap<String, LaneEvent>,
}

/// One cosmetic reaction bound to a cue slot. The game turns this into particles / projectile /
/// anim-layer changes. Authored, not code.
#[derive(Debug, Clone, Deserialize)]
pub struct LaneEvent {
    /// Stable lane id for tracing / debugging (e.g. "firebolt_muzzle").
    pub lane_id: String,
    /// Which timeline moment this lane reacts to.
    pub kind: CueKind,
    /// Particle burst params (M1 uses this for muzzle + impact).
    #[serde(default)]
    pub particle: Option<ParticleSpec>,
    /// Cosmetic (non-authoritative) projectile to spawn for OnCast/OnWindow lanes.
    #[serde(default)]
    pub projectile: Option<ProjectileCosmetic>,
    /// Animation layer to drive on the source rig (e.g. "cast_release").
    #[serde(default)]
    pub anim: Option<AnimLayer>,
}

/// Arena's OWN mirror of obelisk's `CueKind` so `.skillfx.ron` can declare which moment a lane
/// reacts to. We do NOT re-use obelisk's `CueKind` in the RON to keep the authoring format
/// decoupled from the sim crate; the dispatcher maps obelisk `CueKind` -> this 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum CueKind {
    OnCast,
    OnWindow,
    OnHit,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ParticleSpec {
    pub count: u32,
    #[serde(default = "default_lifetime")]
    pub lifetime: f32,
    /// Local-space color (RGB 0..1) for the emissive stand-in / billboard tint.
    #[serde(default)]
    pub color: [f32; 3],
    #[serde(default = "default_speed")]
    pub speed: f32,
}
fn default_lifetime() -> f32 {
    0.5
}
fn default_speed() -> f32 {
    3.0
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectileCosmetic {
    /// World units/sec. MUST match the `.cast.ron` window motion speed so the cosmetic mesh tracks
    /// the authoritative hitbox (firebolt = 20.0). NOT speed-scaled.
    pub speed: f32,
    #[serde(default)]
    pub color: [f32; 3],
    #[serde(default = "default_proj_radius")]
    pub radius: f32,
}
fn default_proj_radius() -> f32 {
    0.2
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnimLayer {
    /// Logical anim state name, mapped to a clip node in arena_game's AnimationGraph.
    pub state: String,
}

/// A dispatched lane reaction, surfaced as a Bevy `Message` the game's spawn systems read.
/// Carries the resolved world position + source entity so the consumer has no obelisk dependency.
#[derive(Message, Debug, Clone)]
pub struct CueMessage {
    pub lane_id: String,
    pub kind: CueKind,
    pub source: Entity,
    pub position: Vec3,
    pub event: LaneEvent,
}

/// RON loader for `*.skillfx.ron`. Mirrors obelisk's hand-rolled `CastTimelineLoader`
/// (`obelisk-bevy/src/assets/mod.rs`): a `ron`-crate `AssetLoader` matched on the extension, with a
/// `thiserror` error enum that stringifies the IO / RON errors (those types aren't `Send + Sync`
/// across the async boundary, so we capture their messages).
#[derive(Default, TypePath)]
pub struct SkillFxLoader;

impl AssetLoader for SkillFxLoader {
    type Asset = SkillFx;
    type Settings = ();
    type Error = SkillFxLoadError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _ctx: &mut LoadContext<'_>,
    ) -> Result<SkillFx, SkillFxLoadError> {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| SkillFxLoadError::Io(e.to_string()))?;
        ron::de::from_bytes::<SkillFx>(&bytes).map_err(|e| SkillFxLoadError::Ron(e.to_string()))
    }

    fn extensions(&self) -> &[&str] {
        &["skillfx.ron"]
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillFxLoadError {
    #[error("io: {0}")]
    Io(String),
    #[error("ron: {0}")]
    Ron(String),
}

/// Registers the `SkillFx` asset + its `.skillfx.ron` loader and the `CueMessage` message channel.
/// Render-free: the actual cue→message dispatch + VFX spawning live in later tasks / `arena_game`.
pub struct ArenaSkillsPlugin;

impl Plugin for ArenaSkillsPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<SkillFx>()
            .register_asset_loader(SkillFxLoader)
            .add_message::<CueMessage>(); // bevy 0.18: add_message, not add_event
    }
}
