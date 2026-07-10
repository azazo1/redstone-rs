use std::collections::BTreeMap;

use redstone_core::{
    BlockEntityData, BlockEvent, BlockPos, BlockStateId, DeferredRuleTask, Direction,
    EventContext, NeighborUpdate, RedstoneMode, RulesError, SparseWorld,
};

use crate::orientation::{Orientation, SideBias};

use super::{
    BlockBehavior, Java26Rules, PushReaction, StateDefinition, direction_index, direction_name,
};

pub(super) const CONTINUE_PISTON_RETRACTION: &str = "continue_piston_retraction";
pub(super) const SETTLE_MOVING_PISTON: &str = "settle_moving_piston";
pub(super) const MOVE_RETRACTED_STRUCTURE: &str = "move_retracted_piston_structure";

#[derive(Clone, Copy)]
struct PistonMovement {
    piston_pos: BlockPos,
    start: BlockPos,
    piston_facing: Direction,
    push_direction: Direction,
    extending: bool,
    head_sticky: Option<bool>,
}

#[derive(Clone, Copy)]
struct PistonResolution {
    piston_pos: BlockPos,
    start: BlockPos,
    push_direction: Direction,
    extending: bool,
}

struct PistonStructureResolver<'a> {
    world: &'a SparseWorld,
    piston_pos: BlockPos,
    push_direction: Direction,
    to_push: Vec<BlockPos>,
    to_destroy: Vec<BlockPos>,
}

impl Java26Rules {
    pub(super) fn refresh_piston_head(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
        update: NeighborUpdate,
    ) -> Result<(), RulesError> {
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
        let base_pos = pos.relative(facing.opposite());
        let base = self.state(ctx.world.get_block(base_pos))?;
        let expected_sticky = state.property("type") == Some("sticky");
        let matches_base = matches!(
            base.behavior,
            BlockBehavior::Piston { sticky } if sticky == expected_sticky
        ) && base.bool_property("extended")
            && base.direction_property("facing") == Some(facing);
        if matches_base {
            let orientation = update.orientation.map(|orientation| {
                Orientation::from_index(orientation)
                    .with_front(facing.opposite())
                    .index()
            });
            ctx.neighbor_changed(NeighborUpdate {
                pos: base_pos,
                source_pos: pos,
                source_block: update.source_block,
                orientation,
                moved_by_piston: false,
            });
        }
        Ok(())
    }

    pub(super) fn refresh_piston(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
        let should_extend = self.is_quasi_powered(ctx.world, pos, facing);
        let extended = state.bool_property("extended");
        if should_extend != extended {
            if should_extend
                && self
                    .resolve_push_structure(
                        ctx.world,
                        PistonResolution {
                            piston_pos: pos,
                            start: pos.relative(facing),
                            push_direction: facing,
                            extending: true,
                        },
                    )
                    .is_err()
            {
                return Ok(());
            }
            let event = if should_extend {
                0
            } else {
                let pushed_pos = pos.relative(facing).relative(facing);
                let zero_tick = ctx
                    .world
                    .block_entity(pushed_pos)
                    .filter(|data| data.kind == "minecraft:moving_piston")
                    .is_some_and(|data| {
                        data.fields
                            .get("direction")
                            .and_then(serde_json::Value::as_str)
                            == Some(direction_name(facing))
                            && data
                                .fields
                                .get("extending")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false)
                    });
                if zero_tick { 2 } else { 1 }
            };
            ctx.queue_block_event(BlockEvent {
                pos,
                block: state.kind,
                param_a: event,
                param_b: direction_index(facing) as i32,
            });
        }
        Ok(())
    }

    pub(super) fn move_piston(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
        event: i32,
    ) -> Result<bool, RulesError> {
        let state = self.state(state_id)?.clone();
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
        let sticky = matches!(state.behavior, BlockBehavior::Piston { sticky: true });
        if event == 0 {
            if !self.is_quasi_powered(ctx.world, pos, facing) {
                return Ok(false);
            }
            if !self.move_piston_structure(
                ctx,
                PistonMovement {
                    piston_pos: pos,
                    start: pos.relative(facing),
                    piston_facing: facing,
                    push_direction: facing,
                    extending: true,
                    head_sticky: Some(sticky),
                },
            )? {
                return Ok(false);
            }
            let extended = self.changed_state(state_id, "extended", "true")?;
            ctx.set_block_and_update_neighbors_after_neighbors(
                pos,
                extended,
                "piston_extend",
                state.kind,
            );
            self.queue_observer_shape_updates(ctx, pos);
        } else {
            if self.is_quasi_powered(ctx.world, pos, facing) {
                return Ok(false);
            }
            let head_pos = pos.relative(facing);
            let head_finalized = self.settle_moving_piston(ctx, head_pos, true)?;
            if head_finalized {
                ctx.run_rule_task_after_neighbors(DeferredRuleTask {
                    kind: CONTINUE_PISTON_RETRACTION,
                    pos,
                    param_a: event,
                    param_b: direction_index(facing) as i32,
                });
                return Ok(true);
            }
            self.continue_piston_retraction(ctx, pos, state_id, event)?;
        }
        Ok(true)
    }

    fn move_piston_structure(
        &mut self,
        ctx: &mut EventContext<'_>,
        movement: PistonMovement,
    ) -> Result<bool, RulesError> {
        if !movement.extending {
            let head_pos = movement.piston_pos.relative(movement.piston_facing);
            if self
                .state(ctx.world.get_block(head_pos))
                .is_ok_and(|state| state.name == "minecraft:piston_head")
            {
                self.set_piston_state(
                    ctx,
                    head_pos,
                    self.registry.air_state(),
                    "piston_head_clear",
                    false,
                    false,
                )?;
            }
        }
        let structure = match self.resolve_push_structure(
            ctx.world,
            PistonResolution {
                piston_pos: movement.piston_pos,
                start: movement.start,
                push_direction: movement.push_direction,
                extending: movement.extending,
            },
        ) {
            Ok(structure) => structure,
            Err(_) => return Ok(false),
        };
        let snapshots = structure
            .iter()
            .map(|(source, reaction)| {
                (
                    *source,
                    *reaction,
                    ctx.world.get_block(*source),
                    ctx.world.block_entity(*source).cloned(),
                )
            })
            .collect::<Vec<_>>();
        let mut delete_after_move = snapshots
            .iter()
            .filter(|(_, reaction, _, _)| *reaction != PushReaction::Destroy)
            .map(|(source, _, _, _)| *source)
            .collect::<Vec<_>>();
        for (source, reaction, _, _) in snapshots.iter().rev() {
            ctx.remove_block_entity(*source);
            if *reaction != PushReaction::Destroy {
                continue;
            }
            self.set_piston_state(
                ctx,
                *source,
                self.registry.air_state(),
                "piston_destroy",
                false,
                false,
            )?;
        }
        for (source, reaction, source_state, block_entity) in snapshots.iter().rev() {
            if *reaction == PushReaction::Destroy {
                continue;
            }
            let destination = source.relative(movement.push_direction);
            let moving = self
                .registry
                .state_by_name(
                    "minecraft:moving_piston",
                    [
                        ("facing", direction_name(movement.piston_facing)),
                        ("type", "normal"),
                    ],
                )
                .map_err(|error| RulesError::Message(error.to_string()))?;
            self.set_piston_state(
                ctx,
                destination,
                moving,
                "piston_moving_block",
                true,
                true,
            )?;
            ctx.set_block_entity(
                destination,
                moving_block_entity(
                    self.state(*source_state)?,
                    movement.piston_facing,
                    ctx.tick.0.saturating_add(2),
                    movement.extending,
                    false,
                    block_entity.clone(),
                ),
            );
            delete_after_move.retain(|source| *source != destination);
        }
        let mut head_update = None;
        if let Some(sticky) = movement.head_sticky {
            let head_state = self
                .registry
                .state_by_name(
                    "minecraft:piston_head",
                    [
                        ("facing", direction_name(movement.piston_facing)),
                        ("type", if sticky { "sticky" } else { "normal" }),
                        ("short", "false"),
                    ],
                )
                .map_err(|error| RulesError::Message(error.to_string()))?;
            let moving_head = self
                .registry
                .state_by_name(
                    "minecraft:moving_piston",
                    [
                        ("facing", direction_name(movement.piston_facing)),
                        ("type", if sticky { "sticky" } else { "normal" }),
                    ],
                )
                .map_err(|error| RulesError::Message(error.to_string()))?;
            let head_pos = movement.piston_pos.relative(movement.piston_facing);
            self.set_piston_state(
                ctx,
                head_pos,
                moving_head,
                "piston_moving_head",
                true,
                true,
            )?;
            ctx.set_block_entity(
                head_pos,
                moving_block_entity(
                    self.state(head_state)?,
                    movement.piston_facing,
                    ctx.tick.0.saturating_add(2),
                    true,
                    true,
                    None,
                ),
            );
            delete_after_move.retain(|source| *source != head_pos);
            head_update = Some((head_pos, head_state));
        }
        delete_after_move.sort_by_key(|pos| java_hash_bucket(*pos));
        let cleared_sources = delete_after_move.into_iter().collect::<Vec<_>>();
        for source in &cleared_sources {
            self.set_piston_state(
                ctx,
                *source,
                self.registry.air_state(),
                "piston_source_clear",
                false,
                true,
            )?;
        }
        for source in cleared_sources {
            self.notify_observers_of_shape_change(ctx, source)?;
        }
        let orientation = piston_update_orientation(ctx, movement.push_direction);
        for (source, reaction, source_state, _) in snapshots.iter().rev() {
            if *reaction == PushReaction::Destroy {
                continue;
            }
            let source_kind = self.state(*source_state)?.kind;
            ctx.update_neighbors(*source, source_kind, None, orientation);
        }
        if let Some((head_pos, head_state)) = head_update {
            ctx.update_neighbors(head_pos, self.state(head_state)?.kind, None, orientation);
        }
        Ok(true)
    }

    pub(super) fn continue_piston_retraction(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
        event: i32,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
        let sticky = matches!(state.behavior, BlockBehavior::Piston { sticky: true });
        let head_pos = pos.relative(facing);
        let head_propagates = self
            .state(ctx.world.get_block(head_pos))
            .is_ok_and(|head| head.name == "minecraft:piston_head");
        let retracted = self.changed_state(state_id, "extended", "false")?;
        let moving_base = self
            .registry
            .state_by_name(
                "minecraft:moving_piston",
                [
                    ("facing", direction_name(facing)),
                    ("type", if sticky { "sticky" } else { "normal" }),
                ],
            )
            .map_err(|error| RulesError::Message(error.to_string()))?;
        self.set_piston_state(
            ctx,
            pos,
            moving_base,
            "piston_retracting_base",
            false,
            false,
        )?;
        ctx.set_block_entity(
            pos,
            moving_block_entity(
                self.state(retracted)?,
                facing,
                ctx.tick.0.saturating_add(2),
                false,
                true,
                None,
            ),
        );
        let moving_kind = self.state(moving_base)?.kind;
        for direction in Direction::UPDATE_ORDER {
            ctx.neighbor_changed(NeighborUpdate {
                pos: pos.relative(direction),
                source_pos: pos,
                source_block: moving_kind,
                orientation: None,
                moved_by_piston: false,
            });
            if head_propagates && direction == facing {
                ctx.neighbor_changed(NeighborUpdate {
                    pos,
                    source_pos: head_pos,
                    source_block: moving_kind,
                    orientation: None,
                    moved_by_piston: false,
                });
            }
        }
        self.queue_observer_shape_updates(ctx, pos);
        let mut pulled = false;
        if sticky && event == 2 {
            let source = head_pos.relative(facing);
            ctx.run_rule_task_after_neighbors(DeferredRuleTask {
                kind: SETTLE_MOVING_PISTON,
                pos: source,
                param_a: 1,
                param_b: 0,
            });
        } else if sticky {
            let source = pos.relative(facing).relative(facing);
            let source_state = ctx.world.get_block(source);
            let definition = self.state(source_state)?.clone();
            if source_state != self.registry.air_state()
                && definition.push_reaction == PushReaction::Normal
                && self.piston_can_push(&definition, facing.opposite(), false, facing)
            {
                ctx.run_rule_task_after_neighbors(DeferredRuleTask {
                    kind: MOVE_RETRACTED_STRUCTURE,
                    pos,
                    param_a: 0,
                    param_b: 0,
                });
                pulled = true;
            }
        }
        if !pulled && ctx.world.get_block(head_pos) != self.registry.air_state() {
            let old = ctx.set_block(head_pos, self.registry.air_state(), "piston_head_remove")?;
            self.sync_entity_sensor(head_pos, old, self.registry.air_state());
            ctx.update_neighbors(head_pos, self.state(old)?.kind, None, None);
        }
        Ok(())
    }

    pub(super) fn move_retracted_piston_structure(
        &mut self,
        ctx: &mut EventContext<'_>,
        piston_pos: BlockPos,
    ) -> Result<(), RulesError> {
        let moving_base = self.state(ctx.world.get_block(piston_pos))?.clone();
        let facing = moving_base
            .direction_property("facing")
            .unwrap_or(Direction::North);
        let start = piston_pos.relative(facing).relative(facing);
        self.move_piston_structure(
            ctx,
            PistonMovement {
                piston_pos,
                start,
                piston_facing: facing,
                push_direction: facing.opposite(),
                extending: false,
                head_sticky: None,
            },
        )?;
        Ok(())
    }

    pub(super) fn settle_moving_piston(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        early: bool,
    ) -> Result<bool, RulesError> {
        let Some(data) = ctx
            .world
            .block_entity(pos)
            .filter(|data| data.kind == "minecraft:moving_piston")
            .cloned()
        else {
            return Ok(false);
        };
        let Some(moved_state) = data
            .fields
            .get("moved_state")
            .and_then(serde_json::Value::as_u64)
            .and_then(|state| u32::try_from(state).ok())
            .map(BlockStateId)
        else {
            return Ok(false);
        };
        let moving_state = self.state(ctx.world.get_block(pos))?.clone();
        let direction = moving_state
            .direction_property("facing")
            .unwrap_or(Direction::North);
        let extending = data
            .fields
            .get("extending")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let source = data
            .fields
            .get("source")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let final_state = if early && source {
            self.registry.air_state()
        } else {
            moved_state
        };
        let moved_block_entity = data
            .fields
            .get("moved_block_entity")
            .and_then(|value| serde_json::from_value::<BlockEntityData>(value.clone()).ok());
        self.update_moved_observer_from_neighbor_shapes(ctx, pos, final_state)?;
        ctx.remove_block_entity(pos);
        let old = ctx.set_block(pos, final_state, "piston_movement_settle")?;
        let final_state = self.apply_observer_lifecycle(
            ctx,
            pos,
            old,
            final_state,
            true,
            true,
        )?;
        self.sync_entity_sensor(pos, old, final_state);
        ctx.update_neighbors(pos, self.state(old)?.kind, None, None);
        self.queue_observer_shape_updates(ctx, pos);
        let push_direction = if extending {
            direction
        } else {
            direction.opposite()
        };
        let orientation = piston_update_orientation(ctx, push_direction);
        ctx.neighbor_changed(NeighborUpdate {
            pos,
            source_pos: pos,
            source_block: self.state(final_state)?.kind,
            orientation,
            moved_by_piston: false,
        });
        if final_state != self.registry.air_state()
            && let Some(data) = moved_block_entity
        {
            ctx.set_block_entity(pos, data);
        }
        Ok(true)
    }

    fn set_piston_state(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: BlockStateId,
        cause: &str,
        update_shapes: bool,
        moved_by_piston: bool,
    ) -> Result<bool, RulesError> {
        let old = ctx.set_block(pos, state, cause)?;
        if old == state {
            return Ok(false);
        }
        let state = self.apply_observer_lifecycle(
            ctx,
            pos,
            old,
            state,
            moved_by_piston,
            true,
        )?;
        self.sync_entity_sensor(pos, old, state);
        if update_shapes {
            self.notify_observers_of_shape_change(ctx, pos)?;
        }
        Ok(true)
    }

    fn resolve_push_structure(
        &self,
        world: &SparseWorld,
        resolution: PistonResolution,
    ) -> Result<Vec<(BlockPos, PushReaction)>, RulesError> {
        let mut resolver = PistonStructureResolver {
            world,
            piston_pos: resolution.piston_pos,
            push_direction: resolution.push_direction,
            to_push: Vec::new(),
            to_destroy: Vec::new(),
        };
        let first = self.state(world.get_block(resolution.start))?;
        if !self.piston_can_push(
            first,
            resolution.push_direction,
            false,
            resolution.push_direction,
        ) {
            if resolution.extending && first.push_reaction == PushReaction::Destroy {
                return Ok(vec![(resolution.start, PushReaction::Destroy)]);
            }
            return Err(RulesError::Message(format!(
                "piston cannot push {} at {:?}",
                first.name,
                resolution.start
            )));
        }
        if !self.add_piston_block_line(
            &mut resolver,
            resolution.start,
            resolution.push_direction,
        )? {
            return Err(RulesError::Message("piston structure cannot move".to_owned()));
        }
        let mut index = 0;
        while index < resolver.to_push.len() {
            let pos = resolver.to_push[index];
            if is_sticky_block(&self.state(resolver.world.get_block(pos))?.name)
                && !self.add_piston_branches(&mut resolver, pos)?
            {
                return Err(RulesError::Message("piston sticky branch cannot move".to_owned()));
            }
            index += 1;
        }
        let PistonStructureResolver {
            world,
            to_push,
            to_destroy,
            ..
        } = resolver;
        let mut result = to_push
            .into_iter()
            .map(|pos| (pos, self.state(world.get_block(pos)).map(|state| state.push_reaction)))
            .map(|(pos, reaction)| reaction.map(|reaction| (pos, reaction)))
            .collect::<Result<Vec<_>, _>>()?;
        result.extend(to_destroy.into_iter().map(|pos| (pos, PushReaction::Destroy)));
        Ok(result)
    }

    fn add_piston_block_line(
        &self,
        resolver: &mut PistonStructureResolver<'_>,
        start: BlockPos,
        connection_direction: Direction,
    ) -> Result<bool, RulesError> {
        let mut next = self.state(resolver.world.get_block(start))?;
        if next.id == self.registry.air_state()
            || !self.piston_can_push(
                next,
                resolver.push_direction,
                false,
                connection_direction,
            )
            || start == resolver.piston_pos
            || resolver.to_push.contains(&start)
        {
            return Ok(true);
        }
        let mut block_count = 1usize;
        if block_count + resolver.to_push.len() > 12 {
            return Ok(false);
        }
        while is_sticky_block(&next.name) {
            let pos = relative_n(start, resolver.push_direction.opposite(), block_count);
            let previous = next;
            next = self.state(resolver.world.get_block(pos))?;
            if next.id == self.registry.air_state()
                || !sticks_to(&previous.name, &next.name)
                || !self.piston_can_push(
                    next,
                    resolver.push_direction,
                    false,
                    resolver.push_direction.opposite(),
                )
                || pos == resolver.piston_pos
            {
                break;
            }
            block_count += 1;
            if block_count + resolver.to_push.len() > 12 {
                return Ok(false);
            }
        }
        for offset in (0..block_count).rev() {
            resolver
                .to_push
                .push(relative_n(start, resolver.push_direction.opposite(), offset));
        }
        let mut blocks_added = block_count;
        let mut offset = 1usize;
        loop {
            let pos = relative_n(start, resolver.push_direction, offset);
            if let Some(collision) = resolver
                .to_push
                .iter()
                .position(|candidate| *candidate == pos)
            {
                reorder_piston_blocks(&mut resolver.to_push, blocks_added, collision);
                let end = collision + blocks_added;
                for index in 0..=end {
                    let branch_pos = resolver.to_push[index];
                    if is_sticky_block(&self.state(resolver.world.get_block(branch_pos))?.name)
                        && !self.add_piston_branches(resolver, branch_pos)?
                    {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            next = self.state(resolver.world.get_block(pos))?;
            if next.id == self.registry.air_state() {
                return Ok(true);
            }
            if !self.piston_can_push(
                next,
                resolver.push_direction,
                true,
                resolver.push_direction,
            ) || pos == resolver.piston_pos
            {
                return Ok(false);
            }
            if next.push_reaction == PushReaction::Destroy {
                resolver.to_destroy.push(pos);
                return Ok(true);
            }
            if resolver.to_push.len() >= 12 {
                return Ok(false);
            }
            resolver.to_push.push(pos);
            blocks_added += 1;
            offset += 1;
        }
    }

    fn add_piston_branches(
        &self,
        resolver: &mut PistonStructureResolver<'_>,
        from_pos: BlockPos,
    ) -> Result<bool, RulesError> {
        let from = self.state(resolver.world.get_block(from_pos))?;
        for direction in VANILLA_DIRECTION_ORDER {
            if same_axis(direction, resolver.push_direction) {
                continue;
            }
            let neighbor_pos = from_pos.relative(direction);
            let neighbor = self.state(resolver.world.get_block(neighbor_pos))?;
            if sticks_to(&neighbor.name, &from.name)
                && !self.add_piston_block_line(
                    resolver,
                    neighbor_pos,
                    direction,
                )?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn piston_can_push(
        &self,
        state: &StateDefinition,
        push_direction: Direction,
        allow_destroy: bool,
        connection_direction: Direction,
    ) -> bool {
        match state.push_reaction {
            PushReaction::Normal => !state.has_block_entity,
            PushReaction::Block => false,
            PushReaction::Destroy => allow_destroy,
            PushReaction::PushOnly => push_direction == connection_direction,
        }
    }
}

fn piston_update_orientation(ctx: &mut EventContext<'_>, facing: Direction) -> Option<u8> {
    match ctx.mode {
        RedstoneMode::Default => None,
        RedstoneMode::Experimental => Some(
            Orientation::from_index(ctx.random_bounded(48) as u8)
                .with_side_bias(SideBias::Left)
                .with_front(facing)
                .index(),
        ),
    }
}

fn is_sticky_block(name: &str) -> bool {
    matches!(name, "minecraft:slime_block" | "minecraft:honey_block")
}

fn sticks_to(left: &str, right: &str) -> bool {
    is_sticky_block(right)
        && !matches!(
            (left, right),
            ("minecraft:slime_block", "minecraft:honey_block")
                | ("minecraft:honey_block", "minecraft:slime_block")
        )
        || !is_sticky_block(right)
}

const VANILLA_DIRECTION_ORDER: [Direction; 6] = [
    Direction::Down,
    Direction::Up,
    Direction::North,
    Direction::South,
    Direction::West,
    Direction::East,
];

fn relative_n(pos: BlockPos, direction: Direction, distance: usize) -> BlockPos {
    let (x, y, z) = direction.step();
    let distance = i32::try_from(distance).unwrap_or(i32::MAX);
    pos.offset(x * distance, y * distance, z * distance)
}

fn reorder_piston_blocks(to_push: &mut Vec<BlockPos>, blocks_added: usize, collision: usize) {
    let split = to_push.len() - blocks_added;
    let mut reordered = Vec::with_capacity(to_push.len());
    reordered.extend_from_slice(&to_push[..collision]);
    reordered.extend_from_slice(&to_push[split..]);
    reordered.extend_from_slice(&to_push[collision..split]);
    *to_push = reordered;
}

fn same_axis(left: Direction, right: Direction) -> bool {
    let (left_x, left_y, left_z) = left.step();
    let (right_x, right_y, right_z) = right.step();
    (left_x != 0 && right_x != 0)
        || (left_y != 0 && right_y != 0)
        || (left_z != 0 && right_z != 0)
}

fn java_hash_bucket(pos: BlockPos) -> u32 {
    let hash = pos
        .z
        .wrapping_mul(31)
        .wrapping_add(pos.y)
        .wrapping_mul(31)
        .wrapping_add(pos.x) as u32;
    (hash ^ (hash >> 16)) & 15
}

fn moving_block_entity(
    moved_state: &StateDefinition,
    direction: Direction,
    settle_tick: u64,
    extending: bool,
    source: bool,
    moved_block_entity: Option<BlockEntityData>,
) -> BlockEntityData {
    let mut fields = BTreeMap::from([
        ("moved_state".to_owned(), serde_json::Value::from(moved_state.id.0)),
        (
            "moved_state_name".to_owned(),
            serde_json::Value::String(moved_state.name.clone()),
        ),
        (
            "moved_state_properties".to_owned(),
            serde_json::to_value(&moved_state.properties).unwrap_or_default(),
        ),
        ("direction".to_owned(), serde_json::Value::from(direction_name(direction))),
        ("settle_tick".to_owned(), serde_json::Value::from(settle_tick)),
        ("extending".to_owned(), serde_json::Value::Bool(extending)),
        ("source".to_owned(), serde_json::Value::Bool(source)),
    ]);
    if let Some(data) = moved_block_entity
        && let Ok(value) = serde_json::to_value(data)
    {
        fields.insert("moved_block_entity".to_owned(), value);
    }
    BlockEntityData {
        kind: "minecraft:moving_piston".to_owned(),
        fields,
    }
}
