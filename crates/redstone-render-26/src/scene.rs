use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use anyhow::{Context, Result};
use redstone_core::{BlockPos, BlockStateId};
use redstone_java_26::{BlockBehavior, Java26Registry, StateDefinition};
use redstone_replay_26::{
    RenderTraceBlock, RenderTracePiston, RenderTracePistonEvent,
};
use crate::timeline::CameraPose;
#[cfg(test)]
use redstone_replay_26::RenderTrace;

mod greedy;
mod mesh;
mod models;
mod palette;
pub(crate) mod section;

pub(crate) use mesh::Vertex;
use greedy::{ColorKey, append_full_cubes};
use section::{SECTION_VOLUME, Section, SectionPos, local_index, local_pos};

type SectionMeshCache = BTreeMap<SectionPos, VecDeque<(u64, Arc<[Vertex]>)>>;

pub(crate) struct Scene {
    air: BlockStateId,
    sections: BTreeMap<SectionPos, Section>,
    columns: BTreeMap<(i32, i32), BTreeSet<i32>>,
    dirty: BTreeSet<SectionPos>,
    visible: BTreeSet<SectionPos>,
    visible_key: Option<ViewKey>,
    pistons: BTreeMap<BlockPos, RenderTracePiston>,
    state_cache: BTreeMap<BlockStateId, StateDefinition>,
    model_cache: BTreeMap<(BlockStateId, u8), Arc<[Vertex]>>,
    mesh_cache: SectionMeshCache,
    mesh_cache_bytes: usize,
    generation: u64,
}

pub(crate) struct SectionMeshUpdate {
    pub section: SectionPos,
    pub vertices: Arc<[Vertex]>,
    pub models: Vec<ModelBatch>,
}

#[derive(Clone)]
pub(crate) struct ModelBatch {
    pub key: (BlockStateId, u8),
    pub template: Arc<[Vertex]>,
    pub positions: Arc<[[f32; 3]]>,
}

#[derive(Clone, Copy)]
pub(crate) struct SceneView {
    pub pose: CameraPose,
    pub view_distance: i32,
    pub fov_degrees: f32,
    pub aspect_ratio: f32,
    pub shadows: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ViewKey {
    position: [u64; 3],
    rotation: [u32; 3],
    view_distance: i32,
    shadows: bool,
}

impl Scene {
    pub(crate) fn new(air: BlockStateId) -> Self {
        Self {
            air,
            sections: BTreeMap::new(),
            columns: BTreeMap::new(),
            dirty: BTreeSet::new(),
            visible: BTreeSet::new(),
            visible_key: None,
            pistons: BTreeMap::new(),
            state_cache: BTreeMap::new(),
            model_cache: BTreeMap::new(),
            mesh_cache: BTreeMap::new(),
            mesh_cache_bytes: 0,
            generation: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_trace(trace: &RenderTrace, air: BlockStateId) -> Self {
        let mut scene = Self::new(air);
        for block in &trace.initial_blocks {
            scene.set(*block);
        }
        scene.set_initial_pistons(&trace.initial_pistons);
        scene
    }

    pub(crate) fn insert_initial(&mut self, block: RenderTraceBlock) {
        self.set(block);
    }

    pub(crate) fn set_initial_pistons(&mut self, pistons: &[RenderTracePiston]) {
        self.pistons = pistons
            .iter()
            .map(|piston| (piston.pos, *piston))
            .collect();
    }

    pub(crate) fn apply(
        &mut self,
        changes: &[RenderTraceBlock],
        pistons: &[RenderTracePistonEvent],
    ) {
        if !changes.is_empty() || !pistons.is_empty() {
            self.generation = self.generation.wrapping_add(1);
        }
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

    pub(crate) fn visual_sample(&self, replay_ms: i32) -> (u64, Option<i32>) {
        let animating = self.pistons.values().any(|piston| {
            replay_ms > piston.start_timestamp_ms && replay_ms < piston.settle_timestamp_ms
        });
        (self.generation, animating.then_some(replay_ms))
    }

    fn set(&mut self, block: RenderTraceBlock) {
        let section_pos = SectionPos::from_block(block.pos);
        if block.state == self.air {
            let remove_section = self.sections.get_mut(&section_pos).is_some_and(|section| {
                section.set(local_index(block.pos), block.state, self.air);
                section.is_empty()
            });
            if remove_section {
                self.sections.remove(&section_pos);
                let column = (section_pos.x, section_pos.z);
                if let Some(levels) = self.columns.get_mut(&column) {
                    levels.remove(&section_pos.y);
                    if levels.is_empty() {
                        self.columns.remove(&column);
                    }
                }
            }
        } else {
            if !self.sections.contains_key(&section_pos) {
                self.columns
                    .entry((section_pos.x, section_pos.z))
                    .or_default()
                    .insert(section_pos.y);
            }
            self.sections
                .entry(section_pos)
                .or_insert_with(|| Section::new(self.air))
                .set(local_index(block.pos), block.state, self.air);
        }
        self.dirty.insert(section_pos);
        let local_x = block.pos.x.rem_euclid(16);
        let local_y = block.pos.y.rem_euclid(16);
        let local_z = block.pos.z.rem_euclid(16);
        if local_x == 0 {
            self.dirty.insert(section_pos.offset(-1, 0, 0));
        } else if local_x == 15 {
            self.dirty.insert(section_pos.offset(1, 0, 0));
        }
        if local_y == 0 {
            self.dirty.insert(section_pos.offset(0, -1, 0));
        } else if local_y == 15 {
            self.dirty.insert(section_pos.offset(0, 1, 0));
        }
        if local_z == 0 {
            self.dirty.insert(section_pos.offset(0, 0, -1));
        } else if local_z == 15 {
            self.dirty.insert(section_pos.offset(0, 0, 1));
        }
    }

    pub(crate) fn rebuild_visible(
        &mut self,
        view: SceneView,
        registry: &mut Java26Registry,
    ) -> Result<Vec<SectionMeshUpdate>> {
        let camera_position = view.pose.position;
        let view_distance = view.view_distance;
        let camera_chunk = (
            floor_to_i32(camera_position[0]).div_euclid(16),
            floor_to_i32(camera_position[2]).div_euclid(16),
        );
        let view_key = ViewKey {
            position: camera_position.map(f64::to_bits),
            rotation: view.pose.rotation.map(f32::to_bits),
            view_distance,
            shadows: view.shadows,
        };
        let next_visible = if self.visible_key == Some(view_key) {
            self.visible.clone()
        } else {
            let mut next = BTreeSet::new();
            for x in camera_chunk.0 - view_distance..=camera_chunk.0 + view_distance {
                for z in camera_chunk.1 - view_distance..=camera_chunk.1 + view_distance {
                    if let Some(levels) = self.columns.get(&(x, z)) {
                        next.extend(
                            levels
                                .iter()
                                .map(|&y| SectionPos { x, y, z })
                                .filter(|section| section_required(*section, view)),
                        );
                    }
                }
            }
            next
        };
        let mut updates = self
            .visible
            .difference(&next_visible)
            .copied()
            .map(|section| SectionMeshUpdate {
                section,
                vertices: Arc::from([]),
                models: Vec::new(),
            })
            .collect::<Vec<_>>();
        let rebuild = next_visible
            .iter()
            .filter(|section| self.dirty.contains(section) || !self.visible.contains(section))
            .copied()
            .collect::<Vec<_>>();
        for section in rebuild {
            let (vertices, models) = self.build_section(section, registry)?;
            updates.push(SectionMeshUpdate {
                section,
                vertices,
                models,
            });
            self.dirty.remove(&section);
        }
        self.visible = next_visible;
        self.visible_key = Some(view_key);
        Ok(updates)
    }

    fn build_section(
        &mut self,
        section_pos: SectionPos,
        registry: &mut Java26Registry,
    ) -> Result<(Arc<[Vertex]>, Vec<ModelBatch>)> {
        let Some(section) = self.sections.get(&section_pos).cloned() else {
            return Ok((Arc::from([]), Vec::new()));
        };
        let cacheable = self.visible.contains(&section_pos);
        let mesh_hash = self.section_mesh_hash(section_pos, &section);
        let cached_vertices = self
            .mesh_cache
            .get(&section_pos)
            .and_then(|entries| entries.iter().find(|(hash, _)| *hash == mesh_hash))
            .map(|(_, vertices)| Arc::clone(vertices));
        if let Some(vertices) = cached_vertices {
            let models = self.build_model_batches(section_pos, &section, registry)?;
            return Ok((vertices, models));
        }
        let mut colors = Box::new([None; SECTION_VOLUME]);
        let mut occluding = Box::new([false; SECTION_VOLUME]);
        for index in 0..SECTION_VOLUME {
            let state_id = section.get(index);
            if state_id == self.air {
                continue;
            }
            let state = self.resolve_state(state_id, registry)?;
            if state.name.as_ref() == "minecraft:moving_piston" {
                continue;
            }
            occluding[index] = state.collision_full_block;
            if is_greedy_cube(&state) {
                colors[index] = Some(ColorKey::new(palette::block_color(&state)));
            }
        }
        let mut outside_occluding = [[false; 256]; 6];
        for (face, outside) in outside_occluding.iter_mut().enumerate() {
            for v in 0..16 {
                for u in 0..16 {
                    let pos = boundary_neighbor(section_pos, face, u, v);
                    outside[u + v * 16] = self.is_occluding(pos, registry)?;
                }
            }
        }
        let mut vertices = Vec::new();
        append_full_cubes(
            &mut vertices,
            section_pos,
            &colors,
            &occluding,
            &outside_occluding,
        );
        let vertices = Arc::<[Vertex]>::from(vertices);
        let bytes = vertices.len() * std::mem::size_of::<Vertex>();
        if cacheable && self.mesh_cache_bytes.saturating_add(bytes) <= 256 * 1024 * 1024 {
            let entries = self.mesh_cache.entry(section_pos).or_default();
            entries.push_back((mesh_hash, Arc::clone(&vertices)));
            self.mesh_cache_bytes += bytes;
            if entries.len() > 4
                && let Some((_, removed)) = entries.pop_front()
            {
                self.mesh_cache_bytes -= removed.len() * std::mem::size_of::<Vertex>();
            }
        }
        let models = self.build_model_batches(section_pos, &section, registry)?;
        Ok((vertices, models))
    }

    fn section_mesh_hash(&self, section_pos: SectionPos, section: &Section) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for index in 0..SECTION_VOLUME {
            hash = hash_state(hash, section.get(index));
        }
        for face in 0..6 {
            for v in 0..16 {
                for u in 0..16 {
                    let pos = boundary_neighbor(section_pos, face, u, v);
                    let state = self
                        .sections
                        .get(&SectionPos::from_block(pos))
                        .map_or(self.air, |section| section.get(local_index(pos)));
                    hash = hash_state(hash, state);
                }
            }
        }
        hash
    }

    fn build_model_batches(
        &mut self,
        section_pos: SectionPos,
        section: &Section,
        registry: &mut Java26Registry,
    ) -> Result<Vec<ModelBatch>> {
        let mut batches = BTreeMap::<(BlockStateId, u8), Vec<[f32; 3]>>::new();
        for index in 0..SECTION_VOLUME {
            let state_id = section.get(index);
            if state_id == self.air {
                continue;
            }
            let state = self.resolve_state(state_id, registry)?;
            if state.name.as_ref() == "minecraft:moving_piston" || is_greedy_cube(&state) {
                continue;
            }
            let pos = local_pos(section_pos, index);
            let visible = self.visible_faces(pos, state.collision_full_block, registry)?;
            let visible_key = visible
                .into_iter()
                .enumerate()
                .fold(0u8, |key, (index, visible)| {
                    key | (u8::from(visible) << index)
                });
            let key = (state_id, visible_key);
            self.model_cache.entry(key).or_insert_with(|| {
                let mut vertices = Vec::new();
                models::append_block(&mut vertices, BlockPos::ZERO, &state, visible);
                Arc::from(vertices)
            });
            batches
                .entry(key)
                .or_default()
                .push([pos.x as f32, pos.y as f32, pos.z as f32]);
        }
        Ok(batches
            .into_iter()
            .map(|(key, positions)| ModelBatch {
                key,
                template: Arc::clone(&self.model_cache[&key]),
                positions: Arc::from(positions),
            })
            .collect())
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
            let section = SectionPos::from_block(piston.pos);
            if (section.x - camera_chunk.0).abs() > view_distance
                || (section.z - camera_chunk.1).abs() > view_distance
            {
                continue;
            }
            let moved_state = self
                .state_cache
                .get(&piston.moved_state)
                .cloned()
                .or_else(|| registry.resolve_state_id(piston.moved_state).ok().cloned())
                .with_context(|| {
                    format!("解析 moving piston 方块状态失败: {}", piston.moved_state.0)
                })?;
            let progress = piston_progress(piston, replay_ms);
            models::append_moving_piston(&mut vertices, piston, progress, &moved_state);
        }
        Ok(vertices)
    }

    fn visible_faces(
        &mut self,
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
            visible[index] = !self.is_occluding(neighbor, registry)?;
        }
        Ok(visible)
    }

    fn is_occluding(
        &mut self,
        pos: BlockPos,
        registry: &mut Java26Registry,
    ) -> Result<bool> {
        let section = SectionPos::from_block(pos);
        let Some(state_id) = self
            .sections
            .get(&section)
            .map(|blocks| blocks.get(local_index(pos)))
        else {
            return Ok(false);
        };
        if state_id == self.air {
            return Ok(false);
        }
        Ok(self.resolve_state(state_id, registry)?.collision_full_block)
    }

    fn resolve_state(
        &mut self,
        state_id: BlockStateId,
        registry: &mut Java26Registry,
    ) -> Result<StateDefinition> {
        if let Some(state) = self.state_cache.get(&state_id) {
            return Ok(state.clone());
        }
        let state = registry
            .resolve_state_id(state_id)
            .with_context(|| format!("解析渲染方块状态失败: {}", state_id.0))?
            .clone();
        self.state_cache.insert(state_id, state.clone());
        Ok(state)
    }
}

fn is_greedy_cube(state: &StateDefinition) -> bool {
    state.collision_full_block
        && matches!(
            state.behavior,
            BlockBehavior::Static
                | BlockBehavior::UnsupportedActive
                | BlockBehavior::RedstoneBlock
        )
}

fn hash_state(hash: u64, state: BlockStateId) -> u64 {
    state
        .0
        .to_le_bytes()
        .into_iter()
        .fold(hash, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
}

fn boundary_neighbor(section: SectionPos, face: usize, u: usize, v: usize) -> BlockPos {
    let origin = section.origin();
    match face {
        0 => origin.offset(-1, v as i32, u as i32),
        1 => origin.offset(16, v as i32, u as i32),
        2 => origin.offset(u as i32, -1, v as i32),
        3 => origin.offset(u as i32, 16, v as i32),
        4 => origin.offset(u as i32, v as i32, -1),
        5 => origin.offset(u as i32, v as i32, 16),
        _ => unreachable!(),
    }
}

fn floor_to_i32(value: f64) -> i32 {
    value.floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

fn section_required(section: SectionPos, view: SceneView) -> bool {
    let center = section.origin();
    let center = [
        f64::from(center.x) + 8.0,
        f64::from(center.y) + 8.0,
        f64::from(center.z) + 8.0,
    ];
    let eye = [
        view.pose.position[0],
        view.pose.position[1] + 1.62,
        view.pose.position[2],
    ];
    let delta = [center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]];
    let distance_squared = delta.iter().map(|value| value * value).sum::<f64>();
    if distance_squared <= 16.0 * 16.0 {
        return true;
    }
    let yaw = f64::from(view.pose.rotation[0]).to_radians();
    let pitch = f64::from(view.pose.rotation[1]).to_radians();
    let forward = [
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    ];
    let dot = delta[0] * forward[0] + delta[1] * forward[1] + delta[2] * forward[2];
    let vertical = f64::from(view.fov_degrees).to_radians() * 0.5;
    let horizontal = (vertical.tan() * f64::from(view.aspect_ratio)).atan();
    let diagonal = (vertical.tan().hypot(horizontal.tan())).atan() + 0.18;
    let in_camera = dot / distance_squared.sqrt() >= diagonal.cos();
    if in_camera || !view.shadows {
        return in_camera;
    }
    let focus = [
        eye[0] + forward[0] * 64.0,
        eye[1] + forward[1] * 64.0,
        eye[2] + forward[2] * 64.0,
    ];
    let shadow_distance_squared = center
        .into_iter()
        .zip(focus)
        .map(|(value, focus)| (value - focus).powi(2))
        .sum::<f64>();
    shadow_distance_squared <= 210.0 * 210.0
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

    fn scene_view(position: [f64; 3]) -> SceneView {
        SceneView {
            pose: CameraPose {
                position,
                rotation: [0.0; 3],
            },
            view_distance: 2,
            fov_degrees: 70.0,
            aspect_ratio: 16.0 / 9.0,
            shadows: false,
        }
    }

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
    fn section_boundary_change_invalidates_all_adjacent_sections() {
        let mut registry = Java26Registry::new();
        let stone = state_id(&mut registry, "minecraft:stone", []);
        let mut scene = Scene::new(registry.air_state());
        scene.insert_initial(RenderTraceBlock {
            pos: BlockPos::new(15, 15, 15),
            state: stone,
        });
        scene.dirty.clear();
        scene.apply(
            &[RenderTraceBlock {
                pos: BlockPos::new(15, 15, 15),
                state: registry.air_state(),
            }],
            &[],
        );
        let center = SectionPos { x: 0, y: 0, z: 0 };
        for adjacent in [
            center,
            center.offset(1, 0, 0),
            center.offset(0, 1, 0),
            center.offset(0, 0, 1),
        ] {
            assert!(scene.dirty.contains(&adjacent));
        }
        for untouched in [
            center.offset(-1, 0, 0),
            center.offset(0, -1, 0),
            center.offset(0, 0, -1),
        ] {
            assert!(!scene.dirty.contains(&untouched));
        }
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
            .rebuild_visible(scene_view([0.0, 10.0, 0.0]), &mut registry)
            .unwrap();
        assert_eq!(near.len(), 1);
        assert_eq!(near[0].section, SectionPos { x: 0, y: 0, z: 0 });
        assert!(!near[0].vertices.is_empty());

        let far = scene
            .rebuild_visible(scene_view([1600.0, 10.0, 0.0]), &mut registry)
            .unwrap();
        assert_eq!(far.len(), 2);
        assert!(far.iter().any(|update| update.section == SectionPos { x: 0, y: 0, z: 0 } && update.vertices.is_empty()));
        assert!(far.iter().any(|update| update.section == SectionPos { x: 100, y: 0, z: 0 } && !update.vertices.is_empty()));
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
            .rebuild_visible(scene_view([0.0, 2.0, 0.0]), &mut registry)
            .unwrap();
        let east_face_vertices = updates[0]
            .vertices
            .iter()
            .filter(|vertex| {
                vertex.position[0] == 1.0 && vertex.normal == [127, 0, 0, 0]
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
