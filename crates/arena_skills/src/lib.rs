//! arena_skills — the .skillfx.ron cosmetic-binding layer for obelisk skills.
//!
//! This crate owns the **SkillFx authoring format** (`.skillfx.ron`) and (in a later task) the
//! binding layer that turns obelisk `CueEvent`s into `LaneEvent`s the game consumes. It mirrors
//! obelisk's own `.cast.ron` loader pattern (`obelisk-bevy/src/assets/mod.rs`) but does NOT touch
//! the sim — it's a pure consumer of `observe_cue`, so it stays render-free and headless-testable.

use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use bevy::prelude::*;
use obelisk_bevy::prelude::CueKind as ObeliskCueKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

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

/// The runtime cue → lanes index, flattened from every `.skillfx.ron` in a directory.
///
/// Each `SkillFx.lanes` is a `cue_id -> LaneEvent` map (one lane per cue id per skill file); this
/// registry flattens *all* of them into `cue_id -> Vec<LaneEvent>` so a single cue id fired by
/// obelisk resolves every cosmetic lane bound to it across all skills. The serde `CueMessage` wire
/// type carries only the `cue_id` (not the lane); the consumer re-looks-up the lanes here via
/// [`resolve_cue`], keeping the wire payload small and `arena_skills` engine-neutral.
#[derive(Resource, Default)]
pub struct SkillFxRegistry {
    /// cue_id (the obelisk cue VALUE, e.g. `"firebolt_cast"`) → the lanes that react to it.
    pub by_cue: HashMap<String, Vec<LaneEvent>>,
}

impl SkillFxRegistry {
    /// Load every `*.skillfx.ron` in `dir` and flatten its lanes into the `by_cue` index. Missing
    /// dirs / unreadable / malformed files are skipped silently (spec §12: never crash on content),
    /// so a partial asset set still yields a usable registry.
    pub fn load_dir(dir: &Path) -> Self {
        let mut by_cue: HashMap<String, Vec<LaneEvent>> = HashMap::default();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.to_string_lossy().ends_with(".skillfx.ron") {
                    if let Ok(s) = std::fs::read_to_string(&p) {
                        if let Ok(fx) = ron::de::from_str::<SkillFx>(&s) {
                            for (cue_id, lane) in fx.lanes {
                                by_cue.entry(cue_id).or_default().push(lane);
                            }
                        }
                    }
                }
            }
        }
        Self { by_cue }
    }

    /// The lanes bound to `cue_id`, or `None` if no `.skillfx.ron` bound that cue.
    pub fn lanes(&self, cue_id: &str) -> Option<&[LaneEvent]> {
        self.by_cue.get(cue_id).map(|v| &v[..])
    }
}

/// One cosmetic reaction bound to a cue slot. The game turns this into particles / projectile /
/// anim-layer changes. Authored, not code.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CueKind {
    OnCast,
    OnWindow,
    OnHit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimLayer {
    /// Logical anim state name, mapped to a clip node in arena_game's AnimationGraph.
    pub state: String,
}

/// The serde **wire shape** for a fired cue (M2).
///
/// This is the engine-neutral payload that crosses the network: the obelisk `cue_id` VALUE (the
/// value the `.cast.ron` `vfx_cues` map fires, e.g. `"firebolt_cast"`), the **stable** `ObeliskId`
/// of the source (NOT a local `Entity` — entity ids differ per peer), the world `position`, and the
/// cue `kind`. It deliberately does NOT embed a `LaneEvent`: the consumer re-looks-up the lanes from
/// a [`SkillFxRegistry`] by `cue_id` (see [`resolve_cue`]). `arena_skills` stays lightyear-free —
/// `arena_game` owns the lightyear `CueWireMessage` wrapper (and, single-process, a `LocalCue`
/// wrapper) around this plain serde type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CueMessage {
    /// The obelisk cue VALUE (e.g. `"firebolt_cast"` / `"firebolt_impact"`) — the registry key.
    pub cue_id: String,
    /// The source's stable `ObeliskId` string (caster for OnCast/OnWindow, target for OnHit).
    pub source_id: String,
    /// World position to spawn cosmetics at. `Vec3` serdes via bevy's `serialize` feature.
    pub position: Vec3,
    /// Which timeline moment fired the cue.
    pub kind: CueKind,
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

/// Registers the `SkillFx` asset + its `.skillfx.ron` loader.
///
/// Render-free. The cue → wire dispatch is no longer a `CueMessage` bevy `Message` (M2 made
/// `CueMessage` a plain serde wire type); the egress + consumer halves are the pure
/// [`cue_event_to_message`] / [`resolve_cue`] helpers, wired up by `arena_game` (which owns the
/// lightyear / `LocalCue` message wrappers — keeping `arena_skills` lightyear-free).
pub struct ArenaSkillsPlugin;

impl Plugin for ArenaSkillsPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<SkillFx>()
            .register_asset_loader(SkillFxLoader);
    }
}

/// Maps obelisk's sim-side `CueKind` to arena's authoring-side `CueKind` (1:1). Keeps the
/// `.skillfx.ron` authoring format decoupled from the obelisk sim crate.
impl From<ObeliskCueKind> for CueKind {
    fn from(k: ObeliskCueKind) -> Self {
        match k {
            ObeliskCueKind::OnCast => CueKind::OnCast,
            ObeliskCueKind::OnWindow => CueKind::OnWindow,
            ObeliskCueKind::OnHit => CueKind::OnHit,
        }
    }
}

