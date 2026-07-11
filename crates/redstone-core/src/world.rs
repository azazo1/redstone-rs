use std::collections::BTreeMap;

use indexmap::IndexSet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BlockEntityData, BlockPos, BlockStateId, EntityData, EntityId};

pub const SECTION_EDGE: i32 = 16;
const SECTION_VOLUME: usize = 16 * 16 * 16;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SectionPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectionPos {
    pub fn from_block(pos: BlockPos) -> Self {
        Self {
            x: pos.x >> 4,
            y: pos.y >> 4,
            z: pos.z >> 4,
        }
    }

    pub fn local_index(pos: BlockPos) -> usize {
        let x = (pos.x & 15) as usize;
        let y = (pos.y & 15) as usize;
        let z = (pos.z & 15) as usize;
        (y * 16 + z) * 16 + x
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PaletteSection {
    palette: Vec<BlockStateId>,
    indices: Vec<u16>,
    non_air: usize,
}

impl PaletteSection {
    pub fn new(air: BlockStateId) -> Self {
        Self {
            palette: vec![air],
            indices: vec![0; SECTION_VOLUME],
            non_air: 0,
        }
    }

    pub fn get(&self, index: usize) -> BlockStateId {
        self.palette[self.indices[index] as usize]
    }

    pub fn set(
        &mut self,
        index: usize,
        state: BlockStateId,
        air: BlockStateId,
    ) -> Result<BlockStateId, WorldError> {
        let old = self.get(index);
        if old == state {
            return Ok(old);
        }

        let palette_index = match self
            .palette
            .iter()
            .position(|candidate| *candidate == state)
        {
            Some(index) => index,
            None => {
                if self.palette.len() > u16::MAX as usize {
                    return Err(WorldError::PaletteOverflow);
                }
                self.palette.push(state);
                self.palette.len() - 1
            }
        };
        self.indices[index] = palette_index as u16;

        match (old == air, state == air) {
            (true, false) => self.non_air += 1,
            (false, true) => self.non_air -= 1,
            _ => {}
        }
        Ok(old)
    }

    pub fn is_empty(&self) -> bool {
        self.non_air == 0
    }

    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SparseWorld {
    air: BlockStateId,
    sections: BTreeMap<SectionPos, PaletteSection>,
    dense_sections: Option<DenseSections>,
    block_entities: BTreeMap<BlockPos, BlockEntityData>,
    block_entity_order: IndexSet<BlockPos>,
    entities: BTreeMap<EntityId, EntityData>,
    #[serde(skip)]
    entity_sections: BTreeMap<SectionPos, IndexSet<EntityId>>,
    next_entity_id: u64,
}

impl SparseWorld {
    pub fn new(air: BlockStateId) -> Self {
        Self {
            air,
            sections: BTreeMap::new(),
            dense_sections: None,
            block_entities: BTreeMap::new(),
            block_entity_order: IndexSet::new(),
            entities: BTreeMap::new(),
            entity_sections: BTreeMap::new(),
            next_entity_id: 1,
        }
    }

    pub const fn air(&self) -> BlockStateId {
        self.air
    }

    pub fn get_block(&self, pos: BlockPos) -> BlockStateId {
        let section_pos = SectionPos::from_block(pos);
        self.section(section_pos).map_or(self.air, |section| {
            section.get(SectionPos::local_index(pos))
        })
    }

    pub fn set_block(
        &mut self,
        pos: BlockPos,
        state: BlockStateId,
    ) -> Result<BlockStateId, WorldError> {
        let section_pos = SectionPos::from_block(pos);
        if state == self.air && self.section(section_pos).is_none() {
            return Ok(self.air);
        }
        let air = self.air;
        let section = if let Some(dense) = self
            .dense_sections
            .as_mut()
            .and_then(|dense| dense.slot_mut(section_pos))
        {
            dense.get_or_insert_with(|| PaletteSection::new(air))
        } else {
            self.sections
                .entry(section_pos)
                .or_insert_with(|| PaletteSection::new(air))
        };
        let old = section.set(SectionPos::local_index(pos), state, self.air)?;
        if section.is_empty() {
            if let Some(slot) = self
                .dense_sections
                .as_mut()
                .and_then(|dense| dense.slot_mut(section_pos))
            {
                *slot = None;
            } else {
                self.sections.remove(&section_pos);
            }
        }
        if state == self.air {
            self.block_entities.remove(&pos);
            self.block_entity_order.shift_remove(&pos);
        }
        Ok(old)
    }

    pub fn set_block_entity(&mut self, pos: BlockPos, data: BlockEntityData) {
        self.block_entity_order.insert(pos);
        self.block_entities.insert(pos, data);
    }

    pub fn block_entity(&self, pos: BlockPos) -> Option<&BlockEntityData> {
        self.block_entities.get(&pos)
    }

    pub fn block_entity_mut(&mut self, pos: BlockPos) -> Option<&mut BlockEntityData> {
        self.block_entities.get_mut(&pos)
    }

    pub fn remove_block_entity(&mut self, pos: BlockPos) -> Option<BlockEntityData> {
        self.block_entity_order.shift_remove(&pos);
        self.block_entities.remove(&pos)
    }

    pub fn block_entities(&self) -> impl Iterator<Item = (&BlockPos, &BlockEntityData)> {
        self.block_entity_order
            .iter()
            .filter_map(|pos| self.block_entities.get_key_value(pos))
    }

    pub fn spawn_entity(&mut self, data: EntityData) -> EntityId {
        let id = EntityId(self.next_entity_id);
        self.next_entity_id = self.next_entity_id.saturating_add(1);
        self.insert_entity(id, data);
        id
    }

    pub fn spawn_entity_with_id(
        &mut self,
        id: EntityId,
        data: EntityData,
    ) -> Result<(), WorldError> {
        if self.entities.contains_key(&id) {
            return Err(WorldError::EntityAlreadyExists(id));
        }
        self.next_entity_id = self.next_entity_id.max(id.0.saturating_add(1));
        self.insert_entity(id, data);
        Ok(())
    }

    fn insert_entity(&mut self, id: EntityId, data: EntityData) {
        self.entity_sections
            .entry(section_for_point(data.position))
            .or_default()
            .insert(id);
        self.entities.insert(id, data);
    }

    pub fn remove_entity(&mut self, id: EntityId) -> Option<EntityData> {
        let entity = self.entities.remove(&id)?;
        let section = section_for_point(entity.position);
        if let Some(ids) = self.entity_sections.get_mut(&section) {
            ids.shift_remove(&id);
            if ids.is_empty() {
                self.entity_sections.remove(&section);
            }
        }
        Some(entity)
    }

    pub fn entities(&self) -> impl Iterator<Item = (&EntityId, &EntityData)> {
        self.entities.iter()
    }

    pub fn entity(&self, id: EntityId) -> Option<&EntityData> {
        self.entities.get(&id)
    }

    pub fn entity_fields_mut(
        &mut self,
        id: EntityId,
    ) -> Option<&mut BTreeMap<String, serde_json::Value>> {
        self.entities.get_mut(&id).map(|entity| &mut entity.fields)
    }

    pub fn move_entity(&mut self, id: EntityId, position: [f64; 3]) -> bool {
        let Some(entity) = self.entities.get_mut(&id) else {
            return false;
        };
        let old_section = section_for_point(entity.position);
        let new_section = section_for_point(position);
        entity.position = position;
        if old_section == new_section {
            return true;
        }
        if let Some(ids) = self.entity_sections.get_mut(&old_section) {
            ids.shift_remove(&id);
            if ids.is_empty() {
                self.entity_sections.remove(&old_section);
            }
        }
        self.entity_sections
            .entry(new_section)
            .or_default()
            .insert(id);
        true
    }

    pub fn entity_ids_in_aabb(&self, min: [f64; 3], max: [f64; 3]) -> Vec<EntityId> {
        if min.iter().zip(max).any(|(minimum, maximum)| {
            !minimum.is_finite() || !maximum.is_finite() || *minimum >= maximum
        }) {
            return Vec::new();
        }
        let min_section = section_for_point(min);
        let max_section = section_for_point([
            previous_float(max[0]),
            previous_float(max[1]),
            previous_float(max[2]),
        ]);
        let mut ids = Vec::new();
        for x in min_section.x..=max_section.x {
            for y in min_section.y..=max_section.y {
                for z in min_section.z..=max_section.z {
                    let section = SectionPos { x, y, z };
                    if let Some(section_ids) = self.entity_sections.get(&section) {
                        ids.extend(section_ids.iter().copied());
                    }
                }
            }
        }
        ids.sort();
        ids.dedup();
        ids.retain(|id| {
            self.entities.get(id).is_some_and(|entity| {
                entity.position[0] >= min[0]
                    && entity.position[0] < max[0]
                    && entity.position[1] >= min[1]
                    && entity.position[1] < max[1]
                    && entity.position[2] >= min[2]
                    && entity.position[2] < max[2]
            })
        });
        ids
    }

    pub fn optimize_section_access(&mut self) {
        if self.dense_sections.is_some() || self.sections.is_empty() {
            return;
        }
        let mut positions = self.sections.keys().copied();
        let first = positions.next().expect("sections 非空");
        let (min, max) = positions.fold((first, first), |(min, max), pos| {
            (
                SectionPos {
                    x: min.x.min(pos.x),
                    y: min.y.min(pos.y),
                    z: min.z.min(pos.z),
                },
                SectionPos {
                    x: max.x.max(pos.x),
                    y: max.y.max(pos.y),
                    z: max.z.max(pos.z),
                },
            )
        });
        let Some(volume) = DenseSections::volume(min, max) else {
            return;
        };
        let density_limit = self.sections.len().saturating_mul(8).max(64);
        if volume > density_limit || volume > 1_000_000 {
            return;
        }
        let mut dense = DenseSections::new(min, max, volume);
        for (pos, section) in std::mem::take(&mut self.sections) {
            *dense
                .slot_mut(pos)
                .expect("section bounds were used to size dense storage") = Some(section);
        }
        self.dense_sections = Some(dense);
    }

    pub fn sections(&self) -> impl Iterator<Item = (SectionPos, &PaletteSection)> {
        let mut sections = self
            .sections
            .iter()
            .map(|(pos, section)| (*pos, section))
            .chain(
                self.dense_sections
                    .iter()
                    .flat_map(DenseSections::iter),
            )
            .collect::<Vec<_>>();
        sections.sort_unstable_by_key(|(pos, _)| *pos);
        sections.into_iter()
    }

    pub fn section_count(&self) -> usize {
        self.sections.len()
            + self
                .dense_sections
                .as_ref()
                .map_or(0, DenseSections::section_count)
    }

    pub fn non_air_blocks(&self) -> usize {
        self.sections().map(|(_, section)| section.non_air).sum()
    }

    pub fn iter_blocks(&self) -> impl Iterator<Item = (BlockPos, BlockStateId)> + '_ {
        self.sections()
            .flat_map(move |(section_pos, section)| {
                (0..SECTION_VOLUME).filter_map(move |index| {
                    let state = section.get(index);
                    if state == self.air {
                        return None;
                    }
                    let x = index % 16;
                    let z = (index / 16) % 16;
                    let y = index / 256;
                    Some((
                        BlockPos::new(
                            section_pos.x * 16 + x as i32,
                            section_pos.y * 16 + y as i32,
                            section_pos.z * 16 + z as i32,
                        ),
                        state,
                    ))
                })
            })
    }

    fn section(&self, pos: SectionPos) -> Option<&PaletteSection> {
        if let Some(slot) = self
            .dense_sections
            .as_ref()
            .and_then(|dense| dense.slot(pos))
        {
            return slot.as_ref();
        }
        self.sections.get(&pos)
    }
}

#[derive(Clone, Debug, Serialize)]
struct DenseSections {
    min: SectionPos,
    size_x: usize,
    size_y: usize,
    size_z: usize,
    sections: Vec<Option<PaletteSection>>,
}

impl DenseSections {
    fn volume(min: SectionPos, max: SectionPos) -> Option<usize> {
        let size_x = usize::try_from(max.x.checked_sub(min.x)?.checked_add(1)?).ok()?;
        let size_y = usize::try_from(max.y.checked_sub(min.y)?.checked_add(1)?).ok()?;
        let size_z = usize::try_from(max.z.checked_sub(min.z)?.checked_add(1)?).ok()?;
        size_x.checked_mul(size_y)?.checked_mul(size_z)
    }

    fn new(min: SectionPos, max: SectionPos, volume: usize) -> Self {
        Self {
            min,
            size_x: (max.x - min.x + 1) as usize,
            size_y: (max.y - min.y + 1) as usize,
            size_z: (max.z - min.z + 1) as usize,
            sections: vec![None; volume],
        }
    }

    fn index(&self, pos: SectionPos) -> Option<usize> {
        let x = i64::from(pos.x) - i64::from(self.min.x);
        let y = i64::from(pos.y) - i64::from(self.min.y);
        let z = i64::from(pos.z) - i64::from(self.min.z);
        if x < 0
            || y < 0
            || z < 0
            || x >= self.size_x as i64
            || y >= self.size_y as i64
            || z >= self.size_z as i64
        {
            return None;
        }
        Some((y as usize * self.size_z + z as usize) * self.size_x + x as usize)
    }

    fn slot(&self, pos: SectionPos) -> Option<&Option<PaletteSection>> {
        self.sections.get(self.index(pos)?)
    }

    fn slot_mut(&mut self, pos: SectionPos) -> Option<&mut Option<PaletteSection>> {
        let index = self.index(pos)?;
        self.sections.get_mut(index)
    }

    fn iter(&self) -> impl Iterator<Item = (SectionPos, &PaletteSection)> {
        self.sections
            .iter()
            .enumerate()
            .filter_map(|(index, section)| {
                let section = section.as_ref()?;
                let x = index % self.size_x;
                let z = index / self.size_x % self.size_z;
                let y = index / self.size_x / self.size_z;
                Some((
                    SectionPos {
                        x: self.min.x + x as i32,
                        y: self.min.y + y as i32,
                        z: self.min.z + z as i32,
                    },
                    section,
                ))
            })
    }

    fn section_count(&self) -> usize {
        self.sections.iter().filter(|section| section.is_some()).count()
    }
}

fn section_for_point(point: [f64; 3]) -> SectionPos {
    SectionPos {
        x: block_coordinate(point[0]).div_euclid(SECTION_EDGE),
        y: block_coordinate(point[1]).div_euclid(SECTION_EDGE),
        z: block_coordinate(point[2]).div_euclid(SECTION_EDGE),
    }
}

fn block_coordinate(value: f64) -> i32 {
    value.floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

fn previous_float(value: f64) -> f64 {
    if value == f64::NEG_INFINITY {
        return value;
    }
    let bits = value.to_bits();
    if value > 0.0 {
        f64::from_bits(bits - 1)
    } else if value == 0.0 {
        -f64::MIN_POSITIVE
    } else {
        f64::from_bits(bits + 1)
    }
}

#[derive(Debug, Error)]
pub enum WorldError {
    #[error("区段调色板超过 u16 容量")]
    PaletteOverflow,
    #[error("实体 ID 已存在: {0:?}")]
    EntityAlreadyExists(EntityId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_coordinates_use_euclidean_sections() {
        let air = BlockStateId(0);
        let solid = BlockStateId(1);
        let mut world = SparseWorld::new(air);
        world.set_block(BlockPos::new(-1, -1, -1), solid).unwrap();
        world
            .set_block(BlockPos::new(-16, -16, -16), solid)
            .unwrap();
        world
            .set_block(BlockPos::new(-17, -17, -17), solid)
            .unwrap();

        assert_eq!(world.section_count(), 2);
        assert_eq!(world.get_block(BlockPos::new(-1, -1, -1)), solid);
        assert_eq!(world.get_block(BlockPos::new(-17, -17, -17)), solid);
    }

    #[test]
    fn empty_sections_are_released() {
        let air = BlockStateId(0);
        let solid = BlockStateId(1);
        let mut world = SparseWorld::new(air);
        world.set_block(BlockPos::ZERO, solid).unwrap();
        assert_eq!(world.section_count(), 1);
        world.set_block(BlockPos::ZERO, air).unwrap();
        assert_eq!(world.section_count(), 0);
    }

    #[test]
    fn dense_section_access_preserves_blocks_and_accepts_fallback_sections() {
        let air = BlockStateId(0);
        let solid = BlockStateId(1);
        let mut world = SparseWorld::new(air);
        let inside = BlockPos::new(-17, 20, 33);
        let edge = BlockPos::new(31, 47, 63);
        world.set_block(inside, solid).unwrap();
        world.set_block(edge, solid).unwrap();
        world.optimize_section_access();

        assert_eq!(world.get_block(inside), solid);
        assert_eq!(world.get_block(edge), solid);
        assert_eq!(world.non_air_blocks(), 2);

        let outside = BlockPos::new(1_000, -1_000, 2_000);
        world.set_block(outside, solid).unwrap();
        assert_eq!(world.get_block(outside), solid);
        world.set_block(inside, air).unwrap();
        assert_eq!(world.get_block(inside), air);
        assert_eq!(world.non_air_blocks(), 2);
    }

    #[test]
    fn block_entities_tick_in_registration_order() {
        let mut world = SparseWorld::new(BlockStateId(0));
        let later_coordinate = BlockPos::new(20, 0, 0);
        let earlier_coordinate = BlockPos::new(-20, 0, 0);
        for pos in [later_coordinate, earlier_coordinate] {
            world.set_block_entity(
                pos,
                BlockEntityData {
                    kind: "minecraft:test".to_owned(),
                    fields: BTreeMap::new(),
                },
            );
        }

        assert_eq!(
            world
                .block_entities()
                .map(|(pos, _)| *pos)
                .collect::<Vec<_>>(),
            vec![later_coordinate, earlier_coordinate]
        );

        world.remove_block_entity(later_coordinate);
        world.set_block_entity(
            later_coordinate,
            BlockEntityData {
                kind: "minecraft:test".to_owned(),
                fields: BTreeMap::new(),
            },
        );
        assert_eq!(
            world
                .block_entities()
                .map(|(pos, _)| *pos)
                .collect::<Vec<_>>(),
            vec![earlier_coordinate, later_coordinate]
        );
    }

    #[test]
    fn entity_spatial_index_tracks_spawn_move_and_remove() {
        let mut world = SparseWorld::new(BlockStateId(0));
        let id = world.spawn_entity(EntityData {
            kind: "minecraft:item".to_owned(),
            position: [-0.5, 0.0, 0.5],
            fields: BTreeMap::new(),
        });
        assert_eq!(
            world.entity_ids_in_aabb([-1.0, 0.0, 0.0], [0.0, 1.0, 1.0]),
            vec![id]
        );

        assert!(world.move_entity(id, [16.5, 0.0, 0.5]));
        assert!(
            world
                .entity_ids_in_aabb([-1.0, 0.0, 0.0], [0.0, 1.0, 1.0])
                .is_empty()
        );
        assert_eq!(
            world.entity_ids_in_aabb([16.0, 0.0, 0.0], [17.0, 1.0, 1.0]),
            vec![id]
        );

        world.remove_entity(id);
        assert!(
            world
                .entity_ids_in_aabb([16.0, 0.0, 0.0], [17.0, 1.0, 1.0])
                .is_empty()
        );
    }
}
