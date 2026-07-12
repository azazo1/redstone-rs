use std::collections::{BTreeMap, BTreeSet};

use redstone_core::{
    BlockEntityData, BlockPos, BlockStateId, EntityData, EntityId, EventContext, RulesError,
    SparseWorld,
};
use serde_json::Value;

use super::{ContainerRef, block_pos};
use crate::rules::Java26Rules;

#[derive(Clone)]
struct StateChange {
    state: BlockStateId,
    notify: bool,
}

#[derive(Clone, Default)]
pub(super) struct TransferSession {
    block_entities: BTreeMap<BlockPos, BlockEntityData>,
    entities: BTreeMap<EntityId, EntityData>,
    dirty_blocks: BTreeSet<BlockPos>,
    dirty_block_order: Vec<BlockPos>,
    dirty_entities: BTreeSet<EntityId>,
    dirty_entity_order: Vec<EntityId>,
    removed_entities: BTreeSet<EntityId>,
    states: BTreeMap<BlockPos, StateChange>,
    state_order: Vec<BlockPos>,
    pub(super) comparator_notifications: Vec<BlockPos>,
}

impl TransferSession {
    pub(super) fn block_entity<'a>(
        &'a mut self,
        world: &SparseWorld,
        pos: BlockPos,
    ) -> Option<&'a BlockEntityData> {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            self.block_entities.entry(pos)
        {
            entry.insert(world.block_entity(pos)?.clone());
        }
        self.block_entities.get(&pos)
    }

    pub(super) fn block_entity_mut<'a>(
        &'a mut self,
        world: &SparseWorld,
        pos: BlockPos,
    ) -> Option<&'a mut BlockEntityData> {
        self.block_entity(world, pos)?;
        if self.dirty_blocks.insert(pos) {
            self.dirty_block_order.push(pos);
        }
        self.block_entities.get_mut(&pos)
    }

    pub(super) fn entity<'a>(
        &'a mut self,
        world: &SparseWorld,
        id: EntityId,
    ) -> Option<&'a EntityData> {
        if let std::collections::btree_map::Entry::Vacant(entry) = self.entities.entry(id) {
            entry.insert(world.entity(id)?.clone());
        }
        self.entities.get(&id)
    }

    pub(super) fn entity_mut<'a>(
        &'a mut self,
        world: &SparseWorld,
        id: EntityId,
    ) -> Option<&'a mut EntityData> {
        self.entity(world, id)?;
        if self.dirty_entities.insert(id) {
            self.dirty_entity_order.push(id);
        }
        self.entities.get_mut(&id)
    }

    pub(super) fn remove_entity(&mut self, id: EntityId) {
        self.removed_entities.insert(id);
        self.dirty_entities.remove(&id);
    }

    pub(super) fn state(&self, world: &SparseWorld, pos: BlockPos) -> BlockStateId {
        self.states
            .get(&pos)
            .map_or_else(|| world.get_block(pos), |change| change.state)
    }

    pub(super) fn set_state(&mut self, pos: BlockPos, state: BlockStateId, notify: bool) {
        if !self.states.contains_key(&pos) {
            self.state_order.push(pos);
        }
        self.states.insert(pos, StateChange { state, notify });
    }

    pub(super) fn notify_container(
        &mut self,
        container: &ContainerRef,
        world: &SparseWorld,
    ) {
        match container {
            ContainerRef::Block(pos)
            | ContainerRef::Composter(pos)
            | ContainerRef::Unsupported(pos) => {
                self.comparator_notifications.push(*pos);
            }
            ContainerRef::DoubleChest { first, second } => {
                self.comparator_notifications.push(*first);
                self.comparator_notifications.push(*second);
            }
            ContainerRef::Entity(id) => {
                if let Some(entity) = world.entity(*id) {
                    self.comparator_notifications
                        .push(block_pos(entity.position));
                }
            }
        }
    }

    pub(super) fn set_block_i64(
        &mut self,
        world: &SparseWorld,
        pos: BlockPos,
        key: &str,
        value: i64,
    ) {
        if let Some(data) = self.block_entity_mut(world, pos) {
            data.fields.insert(key.to_owned(), Value::from(value));
        }
    }

    pub(super) fn block_i64(
        &mut self,
        world: &SparseWorld,
        pos: BlockPos,
        key: &str,
    ) -> Option<i64> {
        self.block_entity(world, pos)?
            .fields
            .get(key)
            .and_then(Value::as_i64)
    }

    pub(super) fn commit(
        mut self,
        rules: &mut Java26Rules,
        ctx: &mut EventContext<'_>,
    ) -> Result<(), RulesError> {
        for pos in self.dirty_block_order {
            if let Some(data) = self.block_entities.remove(&pos) {
                ctx.set_block_entity(pos, data);
            }
        }
        for id in self.dirty_entity_order {
            if self.removed_entities.contains(&id) {
                continue;
            }
            if let Some(entity) = self.entities.remove(&id)
                && let Some(fields) = ctx.world.entity_fields_mut(id)
            {
                *fields = entity.fields;
            }
        }
        for id in self.removed_entities {
            ctx.world.remove_entity(id);
        }
        for pos in self.state_order {
            let Some(change) = self.states.remove(&pos) else {
                continue;
            };
            if change.notify {
                rules.set_state_and_notify(ctx, pos, change.state, "container_state", None)?;
            } else {
                rules.set_block(ctx, pos, change.state, "container_state")?;
            }
        }
        for pos in self.comparator_notifications {
            rules.refresh_comparators_near(ctx, pos)?;
        }
        Ok(())
    }
}
