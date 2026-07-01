//! CPU-bake `arena_skills` [`VfxParamBinding`]s into a `bevy_vfx` [`VfxSystem`] before insert.
//!
//! `apply_modulated_param` maps a named authoring param onto the first emitter's `bevy_vfx`
//! module stack (`"scale"`→`SetSize`, `"emission"`→`SpawnModule::Rate`, `"color"`→scale
//! `SetColor` RGB); `bake_bindings` resolves each binding's live driver via
//! [`resolve_binding`] and applies it. Baking happens on the CPU before the `VfxSystem` is
//! inserted — the `bevy_vfx` extract clones `EmitterDef` and the prepare stage re-uploads on
//! `PartialEq` change, so a baked value rides through the normal pipeline.

use arena_skills::{resolve_binding, VfxParamBinding};
use bevy::color::LinearRgba;
use bevy_vfx::data::{ColorSource, EmitterDef, InitModule, ScalarRange, SpawnModule, VfxSystem};

fn set_size(em: &mut EmitterDef, v: f32) {
    for m in em.init.iter_mut() {
        if let InitModule::SetSize(r) = m {
            *r = ScalarRange::Constant(v);
            return;
        }
    }
    em.init.push(InitModule::SetSize(ScalarRange::Constant(v)));
}

fn scale_color(em: &mut EmitterDef, mult: f32) {
    for m in em.init.iter_mut() {
        if let InitModule::SetColor(ColorSource::Constant(c)) = m {
            *c = LinearRgba::rgb(c.red * mult, c.green * mult, c.blue * mult);
            return;
        }
    }
    em.init.push(InitModule::SetColor(ColorSource::Constant(
        LinearRgba::rgb(mult, mult, mult),
    )));
}

/// Bake a single resolved `value` into the first emitter under the named `param`.
///
/// Unknown params + empty-emitter systems are no-ops.
pub fn apply_modulated_param(system: &mut VfxSystem, param: &str, value: f32) {
    let Some(em) = system.emitters.first_mut() else {
        return;
    };
    match param {
        "scale" => set_size(em, value),
        "emission" => em.spawn = SpawnModule::Rate(value),
        "color" => scale_color(em, value),
        _ => {}
    }
}

/// Resolve + bake every binding. `source_for` supplies each binding's raw live driver
/// (charge fraction / stat value), which [`resolve_binding`] normalizes + modulates.
pub fn bake_bindings(
    system: &mut VfxSystem,
    bindings: &[VfxParamBinding],
    source_for: impl Fn(&VfxParamBinding) -> f32,
) {
    for b in bindings {
        let v = resolve_binding(b, source_for(b));
        apply_modulated_param(system, &b.param, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arena_skills::VfxBindSource;

    #[test]
    fn apply_scale_inserts_or_replaces_set_size() {
        let mut system = VfxSystem::default();
        apply_modulated_param(&mut system, "scale", 0.7);
        let em = system.emitters.first().unwrap();
        let sizes: Vec<f32> = em
            .init
            .iter()
            .filter_map(|m| match m {
                InitModule::SetSize(ScalarRange::Constant(v)) => Some(*v),
                _ => None,
            })
            .collect();
        assert_eq!(sizes, vec![0.7]);
    }

    #[test]
    fn apply_emission_sets_spawn_rate() {
        let mut system = VfxSystem::default();
        apply_modulated_param(&mut system, "emission", 120.0);
        match system.emitters.first().unwrap().spawn {
            SpawnModule::Rate(r) => assert_eq!(r, 120.0),
            _ => panic!("expected SpawnModule::Rate"),
        }
    }

    #[test]
    fn bake_charge_binding_modulates_scale() {
        let mut system = VfxSystem::default();
        let bindings = [VfxParamBinding {
            param: "scale".into(),
            source: VfxBindSource::Charge,
            min: 0.2,
            max: 1.0,
        }];
        bake_bindings(&mut system, &bindings, |_| 0.5);
        let em = system.emitters.first().unwrap();
        let size = em
            .init
            .iter()
            .find_map(|m| match m {
                InitModule::SetSize(ScalarRange::Constant(v)) => Some(*v),
                _ => None,
            })
            .expect("SetSize present");
        // modulate(0.2, 1.0, 0.5) == 0.6
        assert!((size - 0.6).abs() < 1e-6, "got {size}");
    }
}
