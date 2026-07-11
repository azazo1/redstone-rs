use redstone_core::{BlockPos, Probe, SparseWorld};
use redstone_io::Scenario;
use redstone_java_26::Java26Registry;
use redstone_replay_26::{ReplayCameraHint, ReplayCameraOptions};

pub(crate) fn options(scenario: &Scenario) -> ReplayCameraOptions {
    scenario.replay.as_ref().map_or_else(
        ReplayCameraOptions::default,
        |replay| ReplayCameraOptions {
            view_distance: replay.camera.view_distance,
            position: replay.camera.position,
            yaw: replay.camera.yaw,
            pitch: replay.camera.pitch,
        },
    )
}

pub(crate) fn hints<'a>(
    scenario: &Scenario,
    worlds: impl IntoIterator<Item = &'a SparseWorld>,
    registry: &Java26Registry,
) -> Vec<ReplayCameraHint> {
    let mut hints = scenario
        .probes
        .iter()
        .filter_map(|probe| {
            let pos = probe_position(&probe.probe)?;
            let expected = scenario
                .expectations
                .iter()
                .any(|expectation| expectation.probe == probe.name);
            Some(ReplayCameraHint {
                position: block_center(pos),
                weight: if expected { 4.0 } else { 2.0 },
            })
        })
        .collect::<Vec<_>>();
    for world in worlds {
        append_observation_blocks(&mut hints, world, registry);
    }
    hints
}

fn probe_position(probe: &Probe) -> Option<BlockPos> {
    match probe {
        Probe::Signal { pos, .. }
        | Probe::BlockState { pos }
        | Probe::Property { pos, .. }
        | Probe::ContainerCount { pos } => Some(*pos),
        Probe::EntityCount { .. }
        | Probe::EntityField { .. }
        | Probe::EntityContainerCount { .. }
        | Probe::EventCount { .. } => None,
    }
}

fn append_observation_blocks(
    hints: &mut Vec<ReplayCameraHint>,
    world: &SparseWorld,
    registry: &Java26Registry,
) {
    hints.extend(world.iter_blocks().filter_map(|(pos, state)| {
        let name = registry.state(state)?.name.as_ref();
        if name == "minecraft:redstone_lamp" || name.ends_with("copper_bulb") {
            Some(ReplayCameraHint {
                position: block_center(pos),
                weight: 1.0,
            })
        } else {
            None
        }
    }));
}

fn block_center(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

#[cfg(test)]
mod tests {
    use redstone_core::BlockStateId;

    use super::*;

    #[test]
    fn expected_block_probe_uses_high_observation_weight() {
        let scenario = toml::from_str::<Scenario>(
            r#"
version = "26.1.2"
mode = "default"

[source]
path = "machine.nbt"

[[probes]]
name = "result"
type = "block_state"
pos = { x = -10, y = 0, z = 0 }

[[expectations]]
tick = 1
probe = "result"
equals = { state = 0 }
"#,
        )
        .unwrap();
        let registry = Java26Registry::new();
        let world = SparseWorld::new(BlockStateId(0));
        let hints = hints(&scenario, [&world], &registry);

        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].position, [-9.5, 0.5, 0.5]);
        assert_eq!(hints[0].weight, 4.0);
    }
}
