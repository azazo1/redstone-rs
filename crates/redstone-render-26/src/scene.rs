use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use redstone_core::{BlockPos, BlockStateId, Direction};
use redstone_java_26::{BlockBehavior, Java26Registry, StateDefinition};
use redstone_replay_26::{RenderTrace, RenderTraceBlock};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
}

impl Vertex {
    pub(crate) const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];

    pub(crate) fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

pub(crate) struct Scene {
    air: BlockStateId,
    chunks: BTreeMap<(i32, i32), BTreeMap<BlockPos, BlockStateId>>,
    dirty: BTreeSet<(i32, i32)>,
    visible: BTreeSet<(i32, i32)>,
}

pub(crate) struct ChunkMeshUpdate {
    pub chunk: (i32, i32),
    pub vertices: Vec<Vertex>,
}

impl Scene {
    pub(crate) fn from_trace(trace: &RenderTrace, air: BlockStateId) -> Self {
        let mut scene = Self {
            air,
            chunks: BTreeMap::new(),
            dirty: BTreeSet::new(),
            visible: BTreeSet::new(),
        };
        for block in &trace.initial_blocks {
            scene.set(*block);
        }
        scene
    }

    pub(crate) fn apply(&mut self, changes: &[RenderTraceBlock]) {
        for change in changes {
            self.set(*change);
        }
    }

    fn set(&mut self, block: RenderTraceBlock) {
        let chunk = chunk_pos(block.pos);
        if block.state == self.air {
            if let Some(blocks) = self.chunks.get_mut(&chunk) {
                blocks.remove(&block.pos);
                if blocks.is_empty() {
                    self.chunks.remove(&chunk);
                }
            }
        } else {
            self.chunks
                .entry(chunk)
                .or_default()
                .insert(block.pos, block.state);
        }
        self.dirty.insert(chunk);
        self.dirty.insert((chunk.0 - 1, chunk.1));
        self.dirty.insert((chunk.0 + 1, chunk.1));
        self.dirty.insert((chunk.0, chunk.1 - 1));
        self.dirty.insert((chunk.0, chunk.1 + 1));
    }

    pub(crate) fn rebuild_visible(
        &mut self,
        camera_position: [f64; 3],
        view_distance: i32,
        registry: &mut Java26Registry,
    ) -> Result<Vec<ChunkMeshUpdate>> {
        let camera_chunk = (
            floor_to_i32(camera_position[0]).div_euclid(16),
            floor_to_i32(camera_position[2]).div_euclid(16),
        );
        let next_visible = self
            .chunks
            .keys()
            .copied()
            .filter(|chunk| {
                (chunk.0 - camera_chunk.0).abs() <= view_distance
                    && (chunk.1 - camera_chunk.1).abs() <= view_distance
            })
            .collect::<BTreeSet<_>>();
        let mut updates = self
            .visible
            .difference(&next_visible)
            .copied()
            .map(|chunk| ChunkMeshUpdate {
                chunk,
                vertices: Vec::new(),
            })
            .collect::<Vec<_>>();
        let rebuild = next_visible
            .iter()
            .filter(|chunk| self.dirty.contains(chunk) || !self.visible.contains(chunk))
            .copied()
            .collect::<Vec<_>>();
        for chunk in rebuild {
            let mut vertices = Vec::new();
            if let Some(blocks) = self.chunks.get(&chunk) {
                for (&pos, &state_id) in blocks {
                    let state = registry
                        .resolve_state_id(state_id)
                        .with_context(|| format!("解析渲染方块状态失败: {}", state_id.0))?
                        .clone();
                    append_block(&mut vertices, self, pos, &state);
                }
            }
            updates.push(ChunkMeshUpdate { chunk, vertices });
            self.dirty.remove(&chunk);
        }
        self.visible = next_visible;
        Ok(updates)
    }

    fn occupied(&self, pos: BlockPos) -> bool {
        self.chunks
            .get(&chunk_pos(pos))
            .is_some_and(|blocks| blocks.contains_key(&pos))
    }
}

fn append_block(output: &mut Vec<Vertex>, scene: &Scene, pos: BlockPos, state: &StateDefinition) {
    let color = block_color(state);
    match state.behavior {
        BlockBehavior::Wire => append_wire(output, pos, state, color),
        BlockBehavior::Repeater | BlockBehavior::Comparator => {
            append_cuboid(output, pos, [0.0, 0.0, 0.0], [1.0, 0.125, 1.0], color, None);
            append_direction_marker(output, pos, state.facing, color);
        }
        BlockBehavior::Torch { wall: false } => {
            append_cuboid(output, pos, [0.42, 0.0, 0.42], [0.58, 0.72, 0.58], color, None);
        }
        BlockBehavior::Lever
        | BlockBehavior::Button { .. }
        | BlockBehavior::PressurePlate { .. }
        | BlockBehavior::PoweredRail
        | BlockBehavior::DetectorRail => {
            append_cuboid(output, pos, [0.08, 0.0, 0.08], [0.92, 0.1, 0.92], color, None);
            append_direction_marker(output, pos, state.facing, color);
        }
        BlockBehavior::Piston { .. } => {
            append_full_cube(output, scene, pos, color);
            append_direction_marker(output, pos, state.facing, [0.85, 0.75, 0.45, color[3]]);
        }
        _ if state.is_rail => {
            append_cuboid(output, pos, [0.02, 0.0, 0.02], [0.98, 0.06, 0.98], color, None);
        }
        _ => append_full_cube(output, scene, pos, color),
    }
}

fn append_wire(output: &mut Vec<Vertex>, pos: BlockPos, state: &StateDefinition, color: [f32; 4]) {
    append_cuboid(output, pos, [0.38, 0.0, 0.38], [0.62, 0.035, 0.62], color, None);
    for (name, min, max) in [
        ("north", [0.44, 0.0, 0.0], [0.56, 0.035, 0.5]),
        ("south", [0.44, 0.0, 0.5], [0.56, 0.035, 1.0]),
        ("west", [0.0, 0.0, 0.44], [0.5, 0.035, 0.56]),
        ("east", [0.5, 0.0, 0.44], [1.0, 0.035, 0.56]),
    ] {
        if state.property(name).is_some_and(|value| value != "none") {
            append_cuboid(output, pos, min, max, color, None);
        }
    }
}

fn append_direction_marker(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    facing: Option<Direction>,
    color: [f32; 4],
) {
    let (min, max) = match facing.unwrap_or(Direction::North) {
        Direction::North => ([0.44, 0.13, 0.08], [0.56, 0.22, 0.52]),
        Direction::South => ([0.44, 0.13, 0.48], [0.56, 0.22, 0.92]),
        Direction::West => ([0.08, 0.13, 0.44], [0.52, 0.22, 0.56]),
        Direction::East => ([0.48, 0.13, 0.44], [0.92, 0.22, 0.56]),
        Direction::Down | Direction::Up => ([0.4, 0.13, 0.4], [0.6, 0.22, 0.6]),
    };
    append_cuboid(output, pos, min, max, color, None);
}

fn append_full_cube(output: &mut Vec<Vertex>, scene: &Scene, pos: BlockPos, color: [f32; 4]) {
    let visible = [
        !scene.occupied(pos.offset(-1, 0, 0)),
        !scene.occupied(pos.offset(1, 0, 0)),
        !scene.occupied(pos.offset(0, -1, 0)),
        !scene.occupied(pos.offset(0, 1, 0)),
        !scene.occupied(pos.offset(0, 0, -1)),
        !scene.occupied(pos.offset(0, 0, 1)),
    ];
    append_cuboid(output, pos, [0.0; 3], [1.0; 3], color, Some(visible));
}

fn append_cuboid(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    min: [f32; 3],
    max: [f32; 3],
    color: [f32; 4],
    visible: Option<[bool; 6]>,
) {
    let base = [pos.x as f32, pos.y as f32, pos.z as f32];
    let point = |x: f32, y: f32, z: f32| [base[0] + x, base[1] + y, base[2] + z];
    let faces = [
        ([-1.0, 0.0, 0.0], [point(min[0], min[1], min[2]), point(min[0], min[1], max[2]), point(min[0], max[1], max[2]), point(min[0], max[1], min[2])]),
        ([1.0, 0.0, 0.0], [point(max[0], min[1], max[2]), point(max[0], min[1], min[2]), point(max[0], max[1], min[2]), point(max[0], max[1], max[2])]),
        ([0.0, -1.0, 0.0], [point(min[0], min[1], max[2]), point(min[0], min[1], min[2]), point(max[0], min[1], min[2]), point(max[0], min[1], max[2])]),
        ([0.0, 1.0, 0.0], [point(min[0], max[1], min[2]), point(min[0], max[1], max[2]), point(max[0], max[1], max[2]), point(max[0], max[1], min[2])]),
        ([0.0, 0.0, -1.0], [point(max[0], min[1], min[2]), point(min[0], min[1], min[2]), point(min[0], max[1], min[2]), point(max[0], max[1], min[2])]),
        ([0.0, 0.0, 1.0], [point(min[0], min[1], max[2]), point(max[0], min[1], max[2]), point(max[0], max[1], max[2]), point(min[0], max[1], max[2])]),
    ];
    for (index, (normal, corners)) in faces.into_iter().enumerate() {
        if visible.is_some_and(|visible| !visible[index]) {
            continue;
        }
        for corner in [corners[0], corners[1], corners[2], corners[0], corners[2], corners[3]] {
            output.push(Vertex {
                position: corner,
                normal,
                color,
            });
        }
    }
}

fn block_color(state: &StateDefinition) -> [f32; 4] {
    let active = state.power > 0 || state.powered || state.lit;
    let emission = if active { 0.72 } else { 0.0 };
    let path = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    let rgb = match state.behavior {
        BlockBehavior::Wire => if active { [0.95, 0.06, 0.03] } else { [0.32, 0.03, 0.02] },
        BlockBehavior::RedstoneBlock => [0.72, 0.03, 0.03],
        BlockBehavior::Torch { .. } => [0.96, 0.28, 0.08],
        BlockBehavior::Repeater | BlockBehavior::Comparator => [0.83, 0.78, 0.68],
        BlockBehavior::Lamp | BlockBehavior::CopperBulb => if state.lit { [1.0, 0.68, 0.16] } else { [0.35, 0.24, 0.12] },
        BlockBehavior::Piston { sticky: true } => [0.34, 0.58, 0.24],
        BlockBehavior::Piston { sticky: false } => [0.62, 0.48, 0.27],
        BlockBehavior::Lever | BlockBehavior::Button { .. } => [0.58, 0.52, 0.43],
        _ if path.contains("glass") => [0.42, 0.68, 0.72],
        _ if path.contains("copper") => [0.62, 0.42, 0.24],
        _ if path.contains("quartz") => [0.84, 0.82, 0.76],
        _ if path.contains("wood") || path.contains("planks") || path.contains("log") => [0.48, 0.32, 0.16],
        _ if path.contains("stone") || path.contains("deepslate") => [0.42, 0.43, 0.44],
        _ => [0.52, 0.56, 0.58],
    };
    [rgb[0], rgb[1], rgb[2], emission]
}

fn chunk_pos(pos: BlockPos) -> (i32, i32) {
    (pos.x.div_euclid(16), pos.z.div_euclid(16))
}

fn floor_to_i32(value: f64) -> i32 {
    value.floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use redstone_replay_26::RenderTraceFrame;

    use super::*;

    #[test]
    fn scene_only_builds_chunks_near_the_camera() {
        let trace = RenderTrace {
            simulation_walltime: Duration::from_secs(1),
            recorded_ticks: 1,
            initial_blocks: vec![
                RenderTraceBlock {
                    pos: BlockPos::new(0, 0, 0),
                    state: BlockStateId(1),
                },
                RenderTraceBlock {
                    pos: BlockPos::new(1600, 0, 0),
                    state: BlockStateId(1),
                },
            ],
            frames: Vec::<RenderTraceFrame>::new(),
        };
        let mut registry = Java26Registry::new();
        let mut scene = Scene::from_trace(&trace, registry.air_state());

        let near = scene
            .rebuild_visible([0.0, 10.0, 0.0], 2, &mut registry)
            .unwrap();
        assert_eq!(near.len(), 1);
        assert_eq!(near[0].chunk, (0, 0));
        assert!(!near[0].vertices.is_empty());

        let far = scene
            .rebuild_visible([1600.0, 10.0, 0.0], 2, &mut registry)
            .unwrap();
        assert_eq!(far.len(), 2);
        assert!(far.iter().any(|update| update.chunk == (0, 0) && update.vertices.is_empty()));
        assert!(far.iter().any(|update| update.chunk == (100, 0) && !update.vertices.is_empty()));
    }
}
