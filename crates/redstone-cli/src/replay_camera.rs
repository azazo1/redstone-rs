use anyhow::{Result, bail};
use redstone_core::{BlockPos, Probe, SparseWorld};
use redstone_io::{Scenario, ScenarioReplayCameraInterpolation, ScenarioReplayCameraKeyframe};
use redstone_java_26::Java26Registry;
use redstone_replay_26::{
    ReplayCameraHint, ReplayCameraInterpolation, ReplayCameraKeyframe, ReplayCameraOptions,
    ReplayCameraPath, ReplayFovKeyframe, ReplayRenderOptions,
};

mod interpolation;

use interpolation::{sample_vector, unwrap_rotations};

#[derive(Clone, Copy)]
struct SparseCameraKeyframe {
    time_ms: i64,
    position: Option<[f64; 3]>,
    rotation: Option<[f64; 3]>,
}

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

pub(crate) fn path(scenario: &Scenario, duration_ms: i64) -> Result<Option<ReplayCameraPath>> {
    let Some(camera) = scenario.replay.as_ref().map(|replay| &replay.camera) else {
        return Ok(None);
    };
    if camera.keyframes.is_empty() {
        return Ok(None);
    }
    validate_keyframe_times(camera, duration_ms)?;
    let interpolation = replay_interpolation(camera.interpolation);
    let sparse = camera
        .keyframes
        .iter()
        .enumerate()
        .map(|(index, keyframe)| resolve_sparse_keyframe(index, keyframe))
        .collect::<Result<Vec<_>>>()?;
    if sparse
        .iter()
        .all(|keyframe| keyframe.position.is_none() && keyframe.rotation.is_none())
    {
        return Ok(None);
    }
    if camera.position.is_some() || camera.yaw.is_some() || camera.pitch.is_some() {
        bail!("[replay.camera] 使用 pose keyframes 时不能同时设置静态 position/yaw/pitch");
    }
    let position_samples = sparse
        .iter()
        .filter_map(|keyframe| Some((keyframe.time_ms, keyframe.position?)))
        .collect::<Vec<_>>();
    let mut rotation_samples = sparse
        .iter()
        .filter_map(|keyframe| Some((keyframe.time_ms, keyframe.rotation?)))
        .collect::<Vec<_>>();
    if position_samples.is_empty() || rotation_samples.is_empty() {
        bail!("摄像机 pose 路径至少需要一个显式 position 和一组显式 yaw/pitch");
    }
    unwrap_rotations(&mut rotation_samples);
    let keyframes = sparse
        .iter()
        .map(|keyframe| {
            let position = sample_vector(&position_samples, keyframe.time_ms, interpolation);
            let rotation = sample_vector(&rotation_samples, keyframe.time_ms, interpolation);
            ReplayCameraKeyframe {
                time_ms: keyframe.time_ms,
                position,
                yaw: rotation[0] as f32,
                pitch: rotation[1] as f32,
                roll: rotation[2] as f32,
            }
        })
        .collect::<Vec<_>>();
    ReplayCameraPath::new(interpolation, keyframes, duration_ms)
        .map(Some)
        .map_err(Into::into)
}

pub(crate) fn render_options(scenario: &Scenario, duration_ms: i64) -> Result<ReplayRenderOptions> {
    let Some(camera) = scenario.replay.as_ref().map(|replay| &replay.camera) else {
        return Ok(ReplayRenderOptions::default());
    };
    if !camera.keyframes.is_empty() {
        validate_keyframe_times(camera, duration_ms)?;
    }
    let mut fov_keyframes = camera
        .keyframes
        .iter()
        .filter_map(|keyframe| Some((keyframe.time_ms, keyframe.fov?)))
        .map(|(time_ms, fov_degrees)| {
            Ok(ReplayFovKeyframe {
                time_ms: i64::try_from(time_ms)
                    .map_err(|_| anyhow::anyhow!("FOV 关键帧 time_ms 超出 ReplayMod long 范围"))?,
                fov_degrees,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if !fov_keyframes.is_empty() {
        let fallback = camera.fov.unwrap_or(70.0);
        if fov_keyframes[0].time_ms != 0 {
            fov_keyframes.insert(
                0,
                ReplayFovKeyframe {
                    time_ms: 0,
                    fov_degrees: fallback,
                },
            );
        }
        if fov_keyframes.last().unwrap().time_ms != duration_ms {
            fov_keyframes.push(ReplayFovKeyframe {
                time_ms: duration_ms,
                fov_degrees: fallback,
            });
        }
    }
    Ok(ReplayRenderOptions {
        fov_degrees: camera.fov,
        fov_interpolation: replay_interpolation(camera.interpolation),
        fov_keyframes,
    })
}

fn validate_keyframe_times(
    camera: &redstone_io::ScenarioReplayCamera,
    duration_ms: i64,
) -> Result<()> {
    if camera.keyframes.len() < 2 {
        bail!("摄像机关键帧至少需要两帧");
    }
    let times = camera
        .keyframes
        .iter()
        .map(|keyframe| {
            i64::try_from(keyframe.time_ms)
                .map_err(|_| anyhow::anyhow!("摄像机关键帧 time_ms 超出 ReplayMod long 范围"))
        })
        .collect::<Result<Vec<_>>>()?;
    if times[0] != 0 || *times.last().unwrap() != duration_ms {
        bail!("摄像机关键帧必须从 0 ms 覆盖到 {duration_ms} ms");
    }
    if let Some(pair) = times.windows(2).find(|pair| pair[1] <= pair[0]) {
        bail!("摄像机关键帧时间必须严格递增: {} -> {}", pair[0], pair[1]);
    }
    Ok(())
}

fn replay_interpolation(
    interpolation: ScenarioReplayCameraInterpolation,
) -> ReplayCameraInterpolation {
    match interpolation {
        ScenarioReplayCameraInterpolation::Linear => ReplayCameraInterpolation::Linear,
        ScenarioReplayCameraInterpolation::Cubic => ReplayCameraInterpolation::Cubic,
        ScenarioReplayCameraInterpolation::CatmullRom => ReplayCameraInterpolation::CatmullRom,
    }
}

fn resolve_sparse_keyframe(
    index: usize,
    keyframe: &ScenarioReplayCameraKeyframe,
) -> Result<SparseCameraKeyframe> {
    let (position, rotation) = match (
        keyframe.f3.as_deref(),
        keyframe.position,
        keyframe.yaw,
        keyframe.pitch,
    ) {
        (Some(command), None, None, None) if keyframe.roll == 0.0 => {
            let (position, yaw, pitch) = parse_f3_command(command)?;
            (Some(position), Some([f64::from(yaw), f64::from(pitch), 0.0]))
        }
        (None, position, Some(yaw), Some(pitch)) => {
            (position, Some([f64::from(yaw), f64::from(pitch), f64::from(keyframe.roll)]))
        }
        (Some(_), _, _, _) => bail!(
            "[replay.camera].keyframes[{index}] 的 f3 不能与 position/yaw/pitch/roll 混用"
        ),
        (None, position, None, None) if keyframe.roll == 0.0 => (position, None),
        _ => bail!("[replay.camera].keyframes[{index}] 的 yaw 和 pitch 必须同时设置"),
    };
    let time_ms = i64::try_from(keyframe.time_ms)
        .map_err(|_| anyhow::anyhow!("位置关键帧 time_ms 超出 ReplayMod long 范围"))?;
    Ok(SparseCameraKeyframe {
        time_ms,
        position,
        rotation,
    })
}

fn parse_f3_command(command: &str) -> Result<([f64; 3], f32, f32)> {
    let command = command.trim().strip_prefix('/').unwrap_or(command.trim());
    let parts = command.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 11
        || parts[0] != "execute"
        || parts[1] != "in"
        || parts[2] != "minecraft:overworld"
        || parts[3] != "run"
        || parts[4] != "tp"
        || parts[5] != "@s"
    {
        bail!(
            "F3+C 命令必须是 /execute in minecraft:overworld run tp @s x y z yaw pitch"
        );
    }
    let parse_f64 = |index: usize, name: &str| -> Result<f64> {
        parts[index]
            .parse::<f64>()
            .map_err(|_| anyhow::anyhow!("F3+C {name} 必须是绝对浮点坐标"))
    };
    let parse_f32 = |index: usize, name: &str| -> Result<f32> {
        parts[index]
            .parse::<f32>()
            .map_err(|_| anyhow::anyhow!("F3+C {name} 必须是浮点角度"))
    };
    Ok((
        [
            parse_f64(6, "x")?,
            parse_f64(7, "y")?,
            parse_f64(8, "z")?,
        ],
        parse_f32(9, "yaw")?,
        parse_f32(10, "pitch")?,
    ))
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
    fn f3_command_preserves_position_and_rotation() {
        let (position, yaw, pitch) = parse_f3_command(
            "/execute in minecraft:overworld run tp @s -195.81 75.78 38.54 -65.90 16.93",
        )
        .unwrap();

        assert_eq!(position, [-195.81, 75.78, 38.54]);
        assert_eq!(yaw, -65.9);
        assert_eq!(pitch, 16.93);
    }

    #[test]
    fn f3_command_rejects_other_dimensions_and_relative_coordinates() {
        assert!(parse_f3_command(
            "/execute in minecraft:the_nether run tp @s 1 2 3 4 5"
        )
        .is_err());
        assert!(parse_f3_command(
            "/execute in minecraft:overworld run tp @s ~1 2 3 4 5"
        )
        .is_err());
    }

    #[test]
    fn sparse_camera_keyframes_interpolate_missing_pose_components() {
        let scenario = toml::from_str::<Scenario>(
            r#"
version = "26.1.2"
mode = "default"
max_ticks = 20

[replay.camera]
interpolation = "linear"

[[replay.camera.keyframes]]
time_ms = 0
position = [0.0, 10.0, 20.0]
yaw = 0.0
pitch = 0.0
fov = 80.0

[[replay.camera.keyframes]]
time_ms = 250
yaw = 45.0
pitch = 5.0

[[replay.camera.keyframes]]
time_ms = 500
fov = 70.0

[[replay.camera.keyframes]]
time_ms = 1000
position = [10.0, 20.0, 30.0]
yaw = 180.0
pitch = 20.0
fov = 60.0

[source]
path = "machine.nbt"
"#,
        )
        .unwrap();

        let path = path(&scenario, 1000).unwrap().unwrap();
        assert_eq!(path.keyframes[1].position, [2.5, 12.5, 22.5]);
        assert_eq!(path.keyframes[2].position, [5.0, 15.0, 25.0]);
        assert_eq!(path.keyframes[2].yaw, 90.0);
        assert_eq!(path.keyframes[2].pitch, 10.0);

        let render = render_options(&scenario, 1000).unwrap();
        assert_eq!(
            render
                .fov_keyframes
                .iter()
                .map(|keyframe| keyframe.time_ms)
                .collect::<Vec<_>>(),
            [0, 500, 1000]
        );
    }

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
