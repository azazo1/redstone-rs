use std::collections::BTreeSet;

use redstone_core::{
    BlockPos, BlockStateId, Direction, EntityId, EventContext, RulesError, SparseWorld,
};
use serde_json::Value;

use super::{ItemStack, block_entity_i64};
use crate::rules::{BlockBehavior, Java26Rules, StateDefinition};

mod session;
mod support;

use session::TransferSession;
use support::*;

const HOPPER_COOLDOWN: i64 = 8;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ContainerRef {
    Block(BlockPos),
    DoubleChest { first: BlockPos, second: BlockPos },
    Entity(EntityId),
    Composter(BlockPos),
    Unsupported(BlockPos),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct InsertionOutcome {
    inserted: i64,
    remaining: i64,
}

struct StackInsertion<'a> {
    item_id: &'a str,
    count: i64,
    components: Option<&'a Value>,
    face: Option<Direction>,
    source: Option<&'a ContainerRef>,
}

impl InsertionOutcome {
    fn fully_accepted(self) -> bool {
        self.inserted > 0 && self.remaining == 0
    }
}

impl Java26Rules {
    pub(in crate::rules) fn refresh_hopper_enabled(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?;
        if !matches!(state.behavior, BlockBehavior::Hopper) {
            return Ok(());
        }
        let enabled = !self.is_powered(ctx.world, pos);
        if state.bool_property("enabled") != enabled {
            let next = self.changed_state(state_id, "enabled", enabled.to_string())?;
            self.set_state_and_update_shapes(ctx, pos, next, "hopper_enabled")?;
        }
        Ok(())
    }

    pub(in crate::rules) fn tick_container(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
    ) -> Result<(), RulesError> {
        if !matches!(state.behavior, BlockBehavior::Hopper) {
            return Ok(());
        }
        self.hopper_last_tick.insert(pos, ctx.tick.0);
        self.active_hopper = Some(pos);
        let mut session = TransferSession::default();
        let cooldown = session.block_i64(ctx.world, pos, "cooldown").unwrap_or(0) - 1;
        session.set_block_i64(ctx.world, pos, "cooldown", cooldown.max(0));
        if cooldown > 0 || !state.bool_property("enabled") {
            self.active_hopper = None;
            return session.commit(self, ctx);
        }
        let moved = self.try_move_hopper_items(&mut session, ctx, pos, state, PullAction::Periodic)?;
        if moved {
            session.set_block_i64(ctx.world, pos, "cooldown", HOPPER_COOLDOWN);
            session.comparator_notifications.push(pos);
            *self
                .event_counts
                .entry("hopper_transfer".to_owned())
                .or_default() += 1;
        }
        self.active_hopper = None;
        session.commit(self, ctx)
    }

    pub(in crate::rules) fn tick_hopper_entity_collisions(
        &mut self,
        ctx: &mut EventContext<'_>,
        item_entities: &[EntityId],
    ) -> Result<(), RulesError> {
        for &entity_id in item_entities {
            let Some((min, max)) = item_entity_block_bounds(ctx.world, entity_id) else {
                continue;
            };
            'blocks: for z in min.z..=max.z {
                for y in min.y..=max.y {
                    for x in min.x..=max.x {
                        if ctx.world.entity(entity_id).is_none() {
                            break 'blocks;
                        }
                        let pos = BlockPos::new(x, y, z);
                        let state = self.state(ctx.world.get_block(pos))?.clone();
                        if !matches!(state.behavior, BlockBehavior::Hopper)
                            || !item_entity_intersects_hopper_suck_aabb(
                                ctx.world,
                                entity_id,
                                pos,
                            )
                            || !state.bool_property("enabled")
                            || block_entity_i64(ctx.world, pos, "cooldown").unwrap_or(0) > 0
                        {
                            continue;
                        }
                self.active_hopper = Some(pos);
                let mut session = TransferSession::default();
                let moved = self.try_move_hopper_items(
                    &mut session,
                    ctx,
                    pos,
                    &state,
                    PullAction::Entity(entity_id),
                )?;
                if moved {
                    session.set_block_i64(ctx.world, pos, "cooldown", HOPPER_COOLDOWN);
                    session.comparator_notifications.push(pos);
                    *self
                        .event_counts
                        .entry("hopper_transfer".to_owned())
                        .or_default() += 1;
                }
                self.active_hopper = None;
                session.commit(self, ctx)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub(in crate::rules) fn tick_hopper_minecart(
        &mut self,
        ctx: &mut EventContext<'_>,
        id: EntityId,
    ) -> Result<bool, RulesError> {
        let Some(entity) = ctx.world.entity(id).cloned() else {
            return Ok(false);
        };
        let rail_pos = block_pos(entity.position);
        let rail_state = self.state(ctx.world.get_block(rail_pos))?;
        let enabled = if rail_state.name.as_ref() == "minecraft:activator_rail" {
            !rail_state.bool_property("powered")
        } else {
            entity
                .fields
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        };
        if let Some(fields) = ctx.world.entity_fields_mut(id) {
            fields.insert("enabled".to_owned(), Value::Bool(enabled));
        }
        if !enabled {
            return Ok(false);
        }
        let mut session = TransferSession::default();
        let target = ContainerRef::Entity(id);
        let source_pos = BlockPos::new(
            entity.position[0].floor() as i32,
            (entity.position[1] + 1.5).floor() as i32,
            entity.position[2].floor() as i32,
        );
        let source = self.resolve_container(
            ctx,
            source_pos,
            [entity.position[0], entity.position[1] + 1.5, entity.position[2]],
        );
        let mut moved = if let Some(source) = source {
            let moved = self.transfer_one(
                &mut session,
                ctx,
                &source,
                &target,
                Some(Direction::Down),
                None,
            )?;
            if moved {
                session.notify_container(&source, ctx.world);
            }
            moved
        } else {
            self.absorb_entities_into(
                &mut session,
                ctx,
                &target,
                item_entities_for_minecart_suck(ctx.world, entity.position),
            )?
        };
        if !moved {
            moved = self.absorb_entities_into(
                &mut session,
                ctx,
                &target,
                item_entities_near_minecart(ctx.world, entity.position),
            )?;
        }
        session.commit(self, ctx)?;
        Ok(moved)
    }

    fn try_move_hopper_items(
        &mut self,
        session: &mut TransferSession,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
        action: PullAction,
    ) -> Result<bool, RulesError> {
        let hopper = ContainerRef::Block(pos);
        let facing = state
            .direction_property("facing")
            .unwrap_or(Direction::Down);
        let target = self.resolve_container(ctx, pos.relative(facing), block_center(pos.relative(facing)));
        let mut pushed = false;
        if !self.container_inventory(session, ctx.world, &hopper).is_empty()
            && let Some(target) = target
        {
            pushed = self.transfer_one(
                session,
                ctx,
                &hopper,
                &target,
                None,
                Some(facing.opposite()),
            )?;
        }
        let mut pulled = false;
        if !self.container_full(session, ctx.world, &hopper) {
            pulled = match action {
                PullAction::Periodic => self.periodic_pull(session, ctx, pos, &hopper)?,
                PullAction::Entity(id) => {
                    self.absorb_entities_into(session, ctx, &hopper, vec![id])?
                }
            };
        }
        Ok(pushed | pulled)
    }

    fn periodic_pull(
        &mut self,
        session: &mut TransferSession,
        ctx: &mut EventContext<'_>,
        hopper_pos: BlockPos,
        hopper: &ContainerRef,
    ) -> Result<bool, RulesError> {
        let source_pos = hopper_pos.relative(Direction::Up);
        if let Some(source) = self.resolve_container(ctx, source_pos, block_center(source_pos)) {
            let moved = self.transfer_one(
                session,
                ctx,
                &source,
                hopper,
                Some(Direction::Down),
                None,
            )?;
            if moved {
                session.notify_container(&source, ctx.world);
            }
            return Ok(moved);
        }
        let above = self.state(session.state(ctx.world, source_pos))?;
        if above.collision_full_block && !above.does_not_block_hoppers {
            return Ok(false);
        }
        self.absorb_entities_into(
            session,
            ctx,
            hopper,
            item_entities_in_suck_aabb(ctx.world, hopper_pos, false),
        )
    }

    fn absorb_entities_into(
        &mut self,
        session: &mut TransferSession,
        ctx: &mut EventContext<'_>,
        target: &ContainerRef,
        entity_ids: Vec<EntityId>,
    ) -> Result<bool, RulesError> {
        for id in entity_ids {
            let Some(entity) = session.entity(ctx.world, id).cloned() else {
                continue;
            };
            let Some(item_id) = entity.fields.get("item_id").and_then(Value::as_str) else {
                continue;
            };
            let count = entity
                .fields
                .get("item_count")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .max(0);
            if count == 0 {
                continue;
            }
            let components = entity
                .fields
                .get("item_components")
                .or_else(|| entity.fields.get("components"))
                .cloned();
            let outcome = self.insert_stack(
                session,
                ctx,
                target,
                StackInsertion {
                    item_id,
                    count,
                    components: components.as_ref(),
                    face: None,
                    source: None,
                },
            )?;
            if outcome.inserted == 0 {
                continue;
            }
            session.notify_container(target, ctx.world);
            if outcome.remaining == 0 {
                session.remove_entity(id);
            } else if let Some(entity) = session.entity_mut(ctx.world, id) {
                entity
                    .fields
                    .insert("item_count".to_owned(), Value::from(outcome.remaining));
            }
            if outcome.fully_accepted() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn transfer_one(
        &mut self,
        session: &mut TransferSession,
        ctx: &mut EventContext<'_>,
        source: &ContainerRef,
        target: &ContainerRef,
        source_face: Option<Direction>,
        target_face: Option<Direction>,
    ) -> Result<bool, RulesError> {
        let slots = self.slots_for_face(session, ctx.world, source, source_face);
        for slot in slots {
            let stacks = self.container_inventory(session, ctx.world, source);
            let Some(stack) = stacks.iter().find(|stack| stack.slot == slot).cloned() else {
                continue;
            };
            if !self.can_take(session, ctx.world, source, target, &stack, source_face) {
                continue;
            }
            let checkpoint = session.clone();
            let Some(item) = self.remove_from_slot(session, ctx, source, slot, 1)? else {
                continue;
            };
            let outcome = self.insert_stack(
                session,
                ctx,
                target,
                StackInsertion {
                    item_id: &item.item_id,
                    count: 1,
                    components: item.components.as_ref(),
                    face: target_face,
                    source: Some(source),
                },
            )?;
            if outcome.fully_accepted() {
                session.notify_container(target, ctx.world);
                return Ok(true);
            }
            *session = checkpoint;
        }
        Ok(false)
    }

    fn insert_stack(
        &mut self,
        session: &mut TransferSession,
        ctx: &mut EventContext<'_>,
        target: &ContainerRef,
        insertion: StackInsertion<'_>,
    ) -> Result<InsertionOutcome, RulesError> {
        let StackInsertion {
            item_id,
            count,
            components,
            face,
            source,
        } = insertion;
        if count <= 0 {
            return Ok(InsertionOutcome::default());
        }
        if let ContainerRef::Composter(pos) = target {
            return self.insert_composter(session, ctx, *pos, item_id, count, face);
        }
        let was_empty = self.container_inventory(session, ctx.world, target).is_empty();
        let slots = self.slots_for_face(session, ctx.world, target, face);
        let mut stacks = self.container_inventory(session, ctx.world, target);
        let mut remaining = count;
        for stack in stacks.iter_mut().filter(|stack| {
            stack.item_id == item_id
                && stack.components.as_ref() == components
                && slots.contains(&stack.slot)
        }) {
            if !self.can_place(session, ctx.world, target, stack.slot, item_id, components) {
                continue;
            }
            let limit = self.stack_limit(session, ctx.world, target, stack.slot, item_id, components);
            let moved = remaining.min(limit.saturating_sub(stack.count).max(0));
            stack.count += moved;
            remaining -= moved;
            if remaining == 0 {
                break;
            }
        }
        let mut used = stacks.iter().map(|stack| stack.slot).collect::<BTreeSet<_>>();
        while remaining > 0 {
            let Some(slot) = slots.iter().copied().find(|slot| {
                !used.contains(slot)
                    && self.can_place(session, ctx.world, target, *slot, item_id, components)
            }) else {
                break;
            };
            let moved = remaining.min(self.stack_limit(
                session,
                ctx.world,
                target,
                slot,
                item_id,
                components,
            ));
            if moved <= 0 {
                break;
            }
            stacks.push(ItemStack {
                slot,
                item_id: item_id.to_owned(),
                count: moved,
                components: components.cloned(),
            });
            used.insert(slot);
            remaining -= moved;
        }
        let inserted = count - remaining;
        if inserted > 0 {
            self.set_container_inventory(session, ctx, target, stacks)?;
            self.apply_incoming_hopper_cooldown(session, ctx, target, source, was_empty);
        }
        Ok(InsertionOutcome { inserted, remaining })
    }

    fn apply_incoming_hopper_cooldown(
        &self,
        session: &mut TransferSession,
        ctx: &EventContext<'_>,
        target: &ContainerRef,
        source: Option<&ContainerRef>,
        was_empty: bool,
    ) {
        let ContainerRef::Block(target_pos) = target else {
            return;
        };
        if !was_empty
            || session.block_i64(ctx.world, *target_pos, "cooldown").unwrap_or(-1)
                > HOPPER_COOLDOWN
            || session
                .block_entity(ctx.world, *target_pos)
                .is_none_or(|data| data.kind != "minecraft:hopper")
        {
            return;
        }
        let skip_tick = if let Some(ContainerRef::Block(source_pos)) = source
            && session
                .block_entity(ctx.world, *source_pos)
                .is_some_and(|data| data.kind == "minecraft:hopper")
        {
            let source_tick = self.hopper_last_tick.get(source_pos).copied().unwrap_or(0);
            self.hopper_last_tick
                .get(target_pos)
                .is_some_and(|target_tick| *target_tick >= source_tick)
        } else {
            false
        };
        session.set_block_i64(
            ctx.world,
            *target_pos,
            "cooldown",
            HOPPER_COOLDOWN - i64::from(skip_tick),
        );
    }

    fn remove_from_slot(
        &mut self,
        session: &mut TransferSession,
        ctx: &mut EventContext<'_>,
        source: &ContainerRef,
        slot: u32,
        count: i64,
    ) -> Result<Option<ItemStack>, RulesError> {
        if let ContainerRef::Composter(pos) = source {
            let state_id = session.state(ctx.world, *pos);
            let state = self.state(state_id)?;
            if state.int_property("level") == Some(8) && slot == 0 {
                let next = self.changed_state(state_id, "level", "0")?;
                session.set_state(*pos, next, true);
                return Ok(Some(ItemStack {
                    slot: 0,
                    item_id: "minecraft:bone_meal".to_owned(),
                    count: 1,
                    components: None,
                }));
            }
            return Ok(None);
        }
        let mut stacks = self.container_inventory(session, ctx.world, source);
        let Some(index) = stacks.iter().position(|stack| stack.slot == slot) else {
            return Ok(None);
        };
        let taken = count.max(0).min(stacks[index].count);
        if taken == 0 {
            return Ok(None);
        }
        let result = ItemStack {
            slot,
            item_id: stacks[index].item_id.clone(),
            count: taken,
            components: stacks[index].components.clone(),
        };
        stacks[index].count -= taken;
        self.set_container_inventory(session, ctx, source, stacks)?;
        Ok(Some(result))
    }

    fn insert_composter(
        &mut self,
        session: &mut TransferSession,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        item_id: &str,
        count: i64,
        face: Option<Direction>,
    ) -> Result<InsertionOutcome, RulesError> {
        let state_id = session.state(ctx.world, pos);
        let state = self.state(state_id)?;
        let level = state.int_property("level").unwrap_or(0);
        let Some(chance) = super::super::item::official_compost_chance(item_id) else {
            return Ok(InsertionOutcome {
                inserted: 0,
                remaining: count,
            });
        };
        if level >= 7 || face != Some(Direction::Up) {
            return Ok(InsertionOutcome {
                inserted: 0,
                remaining: count,
            });
        }
        let succeeds = level == 0 || ctx.random_bounded(1_000_000) < (chance * 1_000_000.0) as u32;
        if succeeds {
            let next = self.changed_state(state_id, "level", (level + 1).to_string())?;
            session.set_state(pos, next, true);
        }
        Ok(InsertionOutcome {
            inserted: 1,
            remaining: count - 1,
        })
    }

    fn container_inventory(
        &self,
        session: &mut TransferSession,
        world: &SparseWorld,
        container: &ContainerRef,
    ) -> Vec<ItemStack> {
        match container {
            ContainerRef::Block(pos) => session
                .block_entity(world, *pos)
                .map(read_inventory)
                .unwrap_or_default(),
            ContainerRef::DoubleChest { first, second } => {
                let mut stacks = session
                    .block_entity(world, *first)
                    .map(read_inventory)
                    .unwrap_or_default();
                stacks.extend(
                    session
                        .block_entity(world, *second)
                        .map(read_inventory)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|mut stack| {
                            stack.slot += 27;
                            stack
                        }),
                );
                stacks.sort_by_key(|stack| stack.slot);
                stacks
            }
            ContainerRef::Entity(id) => session
                .entity(world, *id)
                .map(|entity| read_inventory_fields(&entity.fields, &entity.kind))
                .unwrap_or_default(),
            ContainerRef::Composter(pos) => {
                let level = self
                    .state(session.state(world, *pos))
                    .ok()
                    .and_then(|state| state.int_property("level"))
                    .unwrap_or(0);
                if level == 8 {
                    vec![ItemStack {
                        slot: 0,
                        item_id: "minecraft:bone_meal".to_owned(),
                        count: 1,
                        components: None,
                    }]
                } else {
                    Vec::new()
                }
            }
            ContainerRef::Unsupported(_) => Vec::new(),
        }
    }

    fn set_container_inventory(
        &mut self,
        session: &mut TransferSession,
        ctx: &mut EventContext<'_>,
        container: &ContainerRef,
        mut stacks: Vec<ItemStack>,
    ) -> Result<(), RulesError> {
        stacks.retain(|stack| stack.count > 0);
        stacks.sort_by_key(|stack| stack.slot);
        match container {
            ContainerRef::Block(pos) => {
                let kind = session
                    .block_entity(ctx.world, *pos)
                    .map(|data| data.kind.clone())
                    .unwrap_or_default();
                let old = session
                    .block_entity(ctx.world, *pos)
                    .map(read_inventory)
                    .unwrap_or_default();
                if let Some(data) = session.block_entity_mut(ctx.world, *pos) {
                    write_inventory_fields(&mut data.fields, &kind, &stacks);
                    if kind == "minecraft:chiseled_bookshelf"
                        && let Some(slot) = changed_slot(&old, &stacks)
                    {
                        data.fields.insert(
                            "last_interacted_slot".to_owned(),
                            Value::from(i64::from(slot)),
                        );
                    }
                }
                self.sync_special_state(session, ctx, *pos, &kind, &stacks)?;
            }
            ContainerRef::DoubleChest { first, second } => {
                let mut first_stacks = Vec::new();
                let mut second_stacks = Vec::new();
                for mut stack in stacks {
                    if stack.slot < 27 {
                        first_stacks.push(stack);
                    } else {
                        stack.slot -= 27;
                        second_stacks.push(stack);
                    }
                }
                for (pos, half) in [(*first, first_stacks), (*second, second_stacks)] {
                    let kind = session
                        .block_entity(ctx.world, pos)
                        .map(|data| data.kind.clone())
                        .unwrap_or_else(|| "minecraft:chest".to_owned());
                    if let Some(data) = session.block_entity_mut(ctx.world, pos) {
                        write_inventory_fields(&mut data.fields, &kind, &half);
                    }
                }
            }
            ContainerRef::Entity(id) => {
                let kind = session
                    .entity(ctx.world, *id)
                    .map(|entity| entity.kind.clone())
                    .unwrap_or_default();
                if let Some(entity) = session.entity_mut(ctx.world, *id) {
                    write_inventory_fields(&mut entity.fields, &kind, &stacks);
                }
            }
            ContainerRef::Composter(_) => {}
            ContainerRef::Unsupported(_) => {}
        }
        Ok(())
    }

    fn sync_special_state(
        &mut self,
        session: &mut TransferSession,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        kind: &str,
        stacks: &[ItemStack],
    ) -> Result<(), RulesError> {
        let mut state_id = session.state(ctx.world, pos);
        if kind == "minecraft:jukebox" {
            state_id = self.changed_state(state_id, "has_record", (!stacks.is_empty()).to_string())?;
            session.set_state(pos, state_id, false);
        } else if kind == "minecraft:chiseled_bookshelf" {
            for slot in 0..6u32 {
                let occupied = stacks.iter().any(|stack| stack.slot == slot);
                state_id = self.changed_state(
                    state_id,
                    &format!("slot_{slot}_occupied"),
                    occupied.to_string(),
                )?;
            }
            session.set_state(pos, state_id, true);
        }
        Ok(())
    }

    fn container_full(
        &self,
        session: &mut TransferSession,
        world: &SparseWorld,
        container: &ContainerRef,
    ) -> bool {
        let stacks = self.container_inventory(session, world, container);
        let slots = self.slots_for_face(session, world, container, None);
        !slots.is_empty()
            && slots.iter().all(|slot| {
                stacks.iter().find(|stack| stack.slot == *slot).is_some_and(|stack| {
                    stack.count
                        >= self.stack_limit(
                            session,
                            world,
                            container,
                            *slot,
                            &stack.item_id,
                            stack.components.as_ref(),
                        )
                })
            })
    }

    fn slots_for_face(
        &self,
        session: &mut TransferSession,
        world: &SparseWorld,
        container: &ContainerRef,
        face: Option<Direction>,
    ) -> Vec<u32> {
        if let ContainerRef::Composter(pos) = container {
            let level = self
                .state(session.state(world, *pos))
                .ok()
                .and_then(|state| state.int_property("level"))
                .unwrap_or(0);
            return match (level, face) {
                (0..=6, Some(Direction::Up)) | (8, Some(Direction::Down)) => vec![0],
                _ => Vec::new(),
            };
        }
        let kind = self.container_kind(session, world, container);
        if let Some(face) = face {
            match kind.as_str() {
                "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker" => {
                    return match face {
                        Direction::Up => vec![0],
                        Direction::Down => vec![2, 1],
                        _ => vec![1],
                    };
                }
                "minecraft:brewing_stand" => {
                    return match face {
                        Direction::Up => vec![3],
                        Direction::Down => vec![0, 1, 2, 3],
                        _ => vec![0, 1, 2, 4],
                    };
                }
                _ => {}
            }
        }
        (0..self.container_slot_count(session, world, container)).collect()
    }

    fn container_slot_count(
        &self,
        session: &mut TransferSession,
        world: &SparseWorld,
        container: &ContainerRef,
    ) -> u32 {
        match container {
            ContainerRef::DoubleChest { .. } => 54,
            ContainerRef::Composter(_) => 1,
            ContainerRef::Unsupported(_) => 0,
            ContainerRef::Block(pos) => {
                let data = session.block_entity(world, *pos);
                data.and_then(|data| data.fields.get("slot_count").and_then(Value::as_u64))
                    .and_then(|count| u32::try_from(count).ok())
                    .or_else(|| data.and_then(|data| default_slots(&data.kind)))
                    .unwrap_or(1)
            }
            ContainerRef::Entity(id) => {
                let entity = session.entity(world, *id);
                entity
                    .and_then(|entity| entity.fields.get("slot_count").and_then(Value::as_u64))
                    .and_then(|count| u32::try_from(count).ok())
                    .or_else(|| entity.and_then(|entity| default_slots(&entity.kind)))
                    .unwrap_or(1)
            }
        }
    }

    fn container_kind(
        &self,
        session: &mut TransferSession,
        world: &SparseWorld,
        container: &ContainerRef,
    ) -> String {
        match container {
            ContainerRef::Block(pos) => session
                .block_entity(world, *pos)
                .map(|data| data.kind.clone())
                .unwrap_or_default(),
            ContainerRef::DoubleChest { first, .. } => session
                .block_entity(world, *first)
                .map(|data| data.kind.clone())
                .unwrap_or_else(|| "minecraft:chest".to_owned()),
            ContainerRef::Entity(id) => session
                .entity(world, *id)
                .map(|entity| entity.kind.clone())
                .unwrap_or_default(),
            ContainerRef::Composter(_) => "minecraft:composter".to_owned(),
            ContainerRef::Unsupported(_) => String::new(),
        }
    }

    fn can_place(
        &self,
        session: &mut TransferSession,
        world: &SparseWorld,
        container: &ContainerRef,
        slot: u32,
        item_id: &str,
        components: Option<&Value>,
    ) -> bool {
        let kind = self.container_kind(session, world, container);
        let stacks = self.container_inventory(session, world, container);
        let current = stacks.iter().find(|stack| stack.slot == slot);
        match kind.as_str() {
            "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker" => {
                match slot {
                    0 => true,
                    1 => super::is_furnace_fuel(item_id)
                        || item_id == "minecraft:bucket"
                            && current.is_none_or(|stack| stack.item_id != "minecraft:bucket"),
                    _ => false,
                }
            }
            "minecraft:brewing_stand" => match slot {
                0..=2 => super::is_brewing_bottle(item_id) && current.is_none(),
                3 => super::is_brewing_ingredient(item_id),
                4 => item_id == "minecraft:blaze_powder",
                _ => false,
            },
            "minecraft:jukebox" => {
                slot == 0 && current.is_none() && is_jukebox_playable(item_id, components)
            }
            "minecraft:chiseled_bookshelf" => {
                slot < 6 && current.is_none() && is_bookshelf_book(item_id)
            }
            "minecraft:crafter" => {
                let disabled = self.disabled_slots(session, world, container);
                if disabled.contains(&slot) {
                    return false;
                }
                let Some(current) = current else {
                    return true;
                };
                if current.item_id != item_id || current.count >= 64 {
                    return false;
                }
                !(slot + 1..9).any(|later_slot| {
                    !disabled.contains(&later_slot)
                        && stacks
                            .iter()
                            .find(|stack| stack.slot == later_slot)
                            .is_none_or(|later| {
                                later.item_id == item_id && later.count < current.count
                            })
                })
            }
            kind if super::is_shulker_box_container(kind) => {
                !super::is_shulker_box_item(item_id)
            }
            _ => true,
        }
    }

    fn can_take(
        &self,
        session: &mut TransferSession,
        world: &SparseWorld,
        source: &ContainerRef,
        target: &ContainerRef,
        stack: &ItemStack,
        face: Option<Direction>,
    ) -> bool {
        let kind = self.container_kind(session, world, source);
        if matches!(kind.as_str(), "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker")
            && face == Some(Direction::Down)
            && stack.slot == 1
            && !matches!(stack.item_id.as_str(), "minecraft:bucket" | "minecraft:water_bucket")
        {
            return false;
        }
        if kind == "minecraft:brewing_stand"
            && stack.slot == 3
            && stack.item_id != "minecraft:glass_bottle"
        {
            return false;
        }
        if kind == "minecraft:jukebox" {
            return self.container_has_empty_slot(session, world, target);
        }
        if kind == "minecraft:chiseled_bookshelf" {
            return !self.container_full(session, world, target);
        }
        true
    }

    fn disabled_slots(
        &self,
        session: &mut TransferSession,
        world: &SparseWorld,
        container: &ContainerRef,
    ) -> BTreeSet<u32> {
        let fields = match container {
            ContainerRef::Block(pos) => session.block_entity(world, *pos).map(|data| &data.fields),
            ContainerRef::Entity(id) => session.entity(world, *id).map(|entity| &entity.fields),
            ContainerRef::DoubleChest { .. }
            | ContainerRef::Composter(_)
            | ContainerRef::Unsupported(_) => None,
        };
        fields
            .and_then(|fields| fields.get("disabled_slots"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .filter_map(|slot| u32::try_from(slot).ok())
            .filter(|slot| *slot < 9)
            .collect()
    }

    fn stack_limit(
        &self,
        session: &mut TransferSession,
        world: &SparseWorld,
        container: &ContainerRef,
        slot: u32,
        item_id: &str,
        components: Option<&Value>,
    ) -> i64 {
        let item_limit = components
            .and_then(|components| components.get("minecraft:max_stack_size"))
            .and_then(Value::as_i64)
            .unwrap_or_else(|| super::super::item::official_item_max_stack_size(item_id))
            .max(1);
        match self.container_kind(session, world, container).as_str() {
            "minecraft:jukebox" | "minecraft:chiseled_bookshelf" => 1,
            "minecraft:brewing_stand" if slot <= 2 => 1,
            _ => item_limit,
        }
    }

    fn container_has_empty_slot(
        &self,
        session: &mut TransferSession,
        world: &SparseWorld,
        container: &ContainerRef,
    ) -> bool {
        let stacks = self.container_inventory(session, world, container);
        self.slots_for_face(session, world, container, None)
            .into_iter()
            .any(|slot| stacks.iter().all(|stack| stack.slot != slot))
    }

    fn resolve_container(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        center: [f64; 3],
    ) -> Option<ContainerRef> {
        let state = self.state(ctx.world.get_block(pos)).ok()?.clone();
        if state.name.as_ref() == "minecraft:composter" {
            return Some(ContainerRef::Composter(pos));
        }
        if let Some(data) = ctx.world.block_entity(pos)
            && is_block_container(data)
        {
            if data.fields.contains_key("LootTable") || data.fields.contains_key("loot_table") {
                ctx.unsupported(pos, "container_loot_table");
                return Some(ContainerRef::Unsupported(pos));
            }
            if matches!(data.kind.as_str(), "minecraft:chest" | "minecraft:trapped_chest")
                && let Some((first, second)) = double_chest(ctx.world, self, pos, &state)
            {
                return Some(ContainerRef::DoubleChest { first, second });
            }
            return Some(ContainerRef::Block(pos));
        }
        let candidates = ctx
            .world
            .entity_ids_in_aabb(
                [center[0] - 0.5, center[1] - 0.5, center[2] - 0.5],
                [center[0] + 0.5, center[1] + 0.5, center[2] + 0.5],
            )
            .into_iter()
            .filter(|id| ctx.world.entity(*id).is_some_and(is_container_entity))
            .collect::<Vec<_>>();
        (!candidates.is_empty()).then(|| {
            let index = ctx.random_bounded(candidates.len() as u32) as usize;
            ContainerRef::Entity(candidates[index])
        })
    }
}

#[derive(Clone, Copy)]
enum PullAction {
    Periodic,
    Entity(EntityId),
}
