use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use redstone_core::{BlockPos, BlockStateId};
use redstone_java_26::Java26Registry;
use redstone_replay_26::{
    RenderTrace, RenderTraceBlock, RenderTracePiston, RenderTracePistonEvent,
};

mod mesh;
mod models;

pub(crate) use mesh::Vertex;

pub(crate) struct Scene {
    air: BlockStateId,
    chunks: BTreeMap<(i32, i32), BTreeMap<BlockPos, BlockStateId>>,
    dirty: BTreeSet<(i32, i32)>,
    visible: BTreeSet<(i32, i32)>,
    pistons: BTreeMap<BlockPos, RenderTracePiston>,
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
            pistons: trace
                .initial_pistons
                .iter()
                .map(|piston| (piston.pos, *piston))
                .collect(),
        };
        for block in &trace.initial_blocks {
            scene.set(*block);
        }
        scene
    }

    pub(crate) fn apply(
        &mut self,
        changes: &[RenderTraceBlock],
        pistons: &[RenderTracePistonEvent],
    ) {
        for change in changes {
            self.set(*change);
        }
        for event in pistons {
            match event {
                RenderTracePistonEvent::Upsert(piston) => {
                    self.pistons.insert(piston.pos, *piston);
                }
                RenderTracePistonEvent::Remove { pos } => {
                    self.pistons.remove(pos);
                }
            }
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
                    if state.name.as_ref() == "minecraft:moving_piston" {
                        continue;
                    }
                    let visible = self.visible_faces(pos, state.collision_full_block, registry)?;
                    models::append_block(&mut vertices, pos, &state, visible);
                }
            }
            updates.push(ChunkMeshUpdate { chunk, vertices });
            self.dirty.remove(&chunk);
        }
        self.visible = next_visible;
        Ok(updates)
    }

    pub(crate) fn dynamic_vertices(
        &self,
        replay_ms: i32,
        camera_position: [f64; 3],
        view_distance: i32,
        registry: &mut Java26Registry,
    ) -> Result<Vec<Vertex>> {
        let camera_chunk = (
            floor_to_i32(camera_position[0]).div_euclid(16),
            floor_to_i32(camera_position[2]).div_euclid(16),
        );
        let mut vertices = Vec::new();
        for piston in self.pistons.values() {
            let chunk = chunk_pos(piston.pos);
            if (chunk.0 - camera_chunk.0).abs() > view_distance
                || (chunk.1 - camera_chunk.1).abs() > view_distance
            {
                continue;
            }
            let moved_state = registry
                .resolve_state_id(piston.moved_state)
                .with_context(|| {
                    format!(
                        "解析 moving piston 方块状态失败: {}",
                        piston.moved_state.0
                    )
                })?
                .clone();
            let progress = piston_progress(piston, replay_ms);
            models::append_moving_piston(&mut vertices, piston, progress, &moved_state);
        }
        Ok(vertices)
    }

    fn visible_faces(
        &self,
        pos: BlockPos,
        full_block: bool,
        registry: &mut Java26Registry,
    ) -> Result<[bool; 6]> {
        if !full_block {
            return Ok([true; 6]);
        }
        let neighbors = [
            pos.offset(-1, 0, 0),
            pos.offset(1, 0, 0),
            pos.offset(0, -1, 0),
            pos.offset(0, 1, 0),
            pos.offset(0, 0, -1),
            pos.offset(0, 0, 1),
        ];
        let mut visible = [true; 6];
        for (index, neighbor) in neighbors.into_iter().enumerate() {
            let Some(state_id) = self
                .chunks
                .get(&chunk_pos(neighbor))
                .and_then(|blocks| blocks.get(&neighbor))
            else {
                continue;
            };
            visible[index] = !registry
                .resolve_state_id(*state_id)
                .with_context(|| format!("解析相邻渲染方块状态失败: {}", state_id.0))?
                .collision_full_block;
        }
        Ok(visible)
    }
}

fn chunk_pos(pos: BlockPos) -> (i32, i32) {
    (pos.x.div_euclid(16), pos.z.div_euclid(16))
}

fn floor_to_i32(value: f64) -> i32 {
    value.floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

fn piston_progress(piston: &RenderTracePiston, replay_ms: i32) -> f32 {
    let duration = (piston.settle_timestamp_ms - piston.start_timestamp_ms) as f32;
    ((replay_ms - piston.start_timestamp_ms) as f32 / duration).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use redstone_java_26::StateResolver;

    use super::*;

    #[test]
    fn piston_progress_matches_two_tick_interpolation() {
        let piston = RenderTracePiston {
            pos: BlockPos::ZERO,
            moved_state: BlockStateId(1),
            direction: redstone_core::Direction::East,
            extending: true,
            source: false,
            start_timestamp_ms: 100,
            settle_timestamp_ms: 200,
        };

        assert_eq!(piston_progress(&piston, 75), 0.0);
        assert_eq!(piston_progress(&piston, 100), 0.0);
        assert_eq!(piston_progress(&piston, 125), 0.25);
        assert_eq!(piston_progress(&piston, 150), 0.5);
        assert_eq!(piston_progress(&piston, 175), 0.75);
        assert_eq!(piston_progress(&piston, 200), 1.0);
        assert_eq!(piston_progress(&piston, 225), 1.0);
    }

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
            initial_pistons: Vec::new(),
            frames: Vec::new(),
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

    #[test]
    fn thin_redstone_device_does_not_occlude_a_full_cube_face() {
        let mut registry = Java26Registry::new();
        let stone = state_id(&mut registry, "minecraft:stone", []);
        let wire = state_id(&mut registry, "minecraft:redstone_wire", []);
        let trace = RenderTrace {
            simulation_walltime: Duration::from_secs(1),
            recorded_ticks: 1,
            initial_blocks: vec![
                RenderTraceBlock {
                    pos: BlockPos::ZERO,
                    state: stone,
                },
                RenderTraceBlock {
                    pos: BlockPos::new(1, 0, 0),
                    state: wire,
                },
            ],
            initial_pistons: Vec::new(),
            frames: Vec::new(),
        };
        let mut scene = Scene::from_trace(&trace, registry.air_state());
        let updates = scene
            .rebuild_visible([0.0, 2.0, 0.0], 2, &mut registry)
            .unwrap();
        let east_face_vertices = updates[0]
            .vertices
            .iter()
            .filter(|vertex| {
                vertex.position[0] == 1.0 && vertex.normal == [1.0, 0.0, 0.0]
            })
            .count();

        assert_eq!(east_face_vertices, 6);
    }

    fn state_id<const N: usize>(
        registry: &mut Java26Registry,
        name: &str,
        overrides: [(&str, &str); N],
    ) -> BlockStateId {
        let overrides = overrides
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect::<BTreeMap<_, _>>();
        let properties = registry
            .complete_state_properties(name, &overrides)
            .unwrap();
        registry.resolve_state(name, &properties).unwrap()
    }
}
