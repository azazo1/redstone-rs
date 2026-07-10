use std::collections::{BTreeMap, BTreeSet, VecDeque};

use redstone_core::{
    BlockEntityData, BlockEvent, BlockPos, BlockStateId, DeferredBlockChange,
    DeferredBlockEntityUpdate, Direction, EventContext, NeighborTask, NeighborUpdate,
    RedstoneMode, RulesError, SparseWorld,
};

use crate::orientation::{Orientation, SideBias};

use super::{
    BlockBehavior, Java26Rules, PushReaction, StateDefinition, direction_index, direction_name,
};

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
                    .resolve_push_structure(ctx.world, pos.relative(facing), facing)
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
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
        let sticky = matches!(state.behavior, BlockBehavior::Piston { sticky: true });
        if event == 0 {
            if !self.is_quasi_powered(ctx.world, pos, facing) {
                return Ok(());
            }
            let structure = self.resolve_push_structure(ctx.world, pos.relative(facing), facing)?;
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
                .collect::<BTreeSet<_>>();
            for (source, reaction, _, _) in snapshots.iter().rev() {
                ctx.world.remove_block_entity(*source);
                if *reaction != PushReaction::Destroy {
                    continue;
                }
                self.set_piston_state_silent(
                    ctx,
                    *source,
                    self.registry.air_state(),
                    "piston_destroy",
                )?;
            }
            for (source, reaction, source_state, block_entity) in snapshots.iter().rev() {
                if *reaction == PushReaction::Destroy {
                    continue;
                }
                let destination = source.relative(facing);
                let moving = self
                    .registry
                    .state_by_name(
                        "minecraft:moving_piston",
                        [("facing", direction_name(facing)), ("type", "normal")],
                    )
                    .map_err(|error| RulesError::Message(error.to_string()))?;
                self.set_piston_state_silent(
                    ctx,
                    destination,
                    moving,
                    "piston_moving_block",
                )?;
                ctx.world.set_block_entity(
                    destination,
                    moving_block_entity(
                        *source_state,
                        facing,
                        ctx.tick.0.saturating_add(2),
                        true,
                        false,
                        block_entity.clone(),
                    ),
                );
                delete_after_move.remove(&destination);
            }
            let head = self
                .registry
                .state_by_name(
                    "minecraft:piston_head",
                    [
                        ("facing", direction_name(facing)),
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
                        ("facing", direction_name(facing)),
                        ("type", if sticky { "sticky" } else { "normal" }),
                    ],
                )
                .map_err(|error| RulesError::Message(error.to_string()))?;
            let head_pos = pos.relative(facing);
            self.set_piston_state_silent(
                ctx,
                head_pos,
                moving_head,
                "piston_moving_head",
            )?;
            ctx.world.set_block_entity(
                head_pos,
                moving_block_entity(
                    head,
                    facing,
                    ctx.tick.0.saturating_add(2),
                    true,
                    true,
                    None,
                ),
            );
            delete_after_move.remove(&head_pos);
            for source in delete_after_move {
                self.set_piston_state_silent(
                    ctx,
                    source,
                    self.registry.air_state(),
                    "piston_source_clear",
                )?;
            }
            let orientation = piston_update_orientation(ctx, facing);
            for (source, reaction, source_state, _) in snapshots.iter().rev() {
                if *reaction == PushReaction::Destroy {
                    continue;
                }
                let source_kind = self.state(*source_state)?.kind;
                ctx.update_neighbors(*source, source_kind, None, orientation);
            }
            ctx.update_neighbors(head_pos, self.state(head)?.kind, None, orientation);
            let extended = self.changed_state(state_id, "extended", "true")?;
            ctx.set_block_and_update_neighbors_after_neighbors(
                pos,
                extended,
                "piston_extend",
                state.kind,
            );
        } else {
            if self.is_quasi_powered(ctx.world, pos, facing) {
                return Ok(());
            }
            let head_pos = pos.relative(facing);
            let head_finalized = self.settle_moving_piston(ctx, head_pos, true)?;
            if event == 2 && head_finalized {
                ctx.queue_block_event(BlockEvent {
                    pos,
                    block: state.kind,
                    param_a: 2,
                    param_b: direction_index(facing) as i32,
                });
            }
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
            self.set_piston_state_silent(ctx, pos, moving_base, "piston_retracting_base")?;
            ctx.world.set_block_entity(
                pos,
                moving_block_entity(
                    retracted,
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
            let mut pulled = false;
            if sticky && event == 2 {
                let source = head_pos.relative(facing);
                self.settle_moving_piston(ctx, source, true)?;
            } else if sticky {
                let source = pos.relative(facing).relative(facing);
                let source_state = ctx.world.get_block(source);
                let definition = self.state(source_state)?.clone();
                if source_state != self.registry.air_state()
                    && definition.push_reaction == PushReaction::Normal
                {
                    let moved_block_entity = ctx.world.block_entity(source).cloned();
                    let moving = self
                        .registry
                        .state_by_name(
                            "minecraft:moving_piston",
                            [("facing", direction_name(facing)), ("type", "normal")],
                        )
                        .map_err(|error| RulesError::Message(error.to_string()))?;
                    let orientation = piston_update_orientation(ctx, facing.opposite());
                    let moving_block_entity = moving_block_entity(
                        source_state,
                        facing,
                        ctx.tick.0.saturating_add(2),
                        false,
                        false,
                        moved_block_entity,
                    );
                    ctx.apply_block_changes_after_neighbors(
                        vec![
                            DeferredBlockChange {
                                pos: head_pos,
                                state: self.registry.air_state(),
                                cause: "sticky_piston_head_clear".to_owned(),
                                block_entity: DeferredBlockEntityUpdate::Remove,
                            },
                            DeferredBlockChange {
                                pos: head_pos,
                                state: moving,
                                cause: "sticky_piston_moving_pull".to_owned(),
                                block_entity: DeferredBlockEntityUpdate::Set(moving_block_entity),
                            },
                            DeferredBlockChange {
                                pos: source,
                                state: self.registry.air_state(),
                                cause: "sticky_piston_pull_clear".to_owned(),
                                block_entity: DeferredBlockEntityUpdate::Remove,
                            },
                        ],
                        vec![NeighborTask::Multi {
                            source_pos: source,
                            source_block: definition.kind,
                            skip_direction: None,
                            orientation,
                            next_index: 0,
                        }],
                    );
                    pulled = true;
                }
            }
            if !pulled && ctx.world.get_block(head_pos) != self.registry.air_state() {
                let old = ctx.set_block(head_pos, self.registry.air_state(), "piston_head_remove")?;
                self.sync_entity_sensor(head_pos, old, self.registry.air_state());
                ctx.update_neighbors(head_pos, self.state(old)?.kind, None, None);
            }
        }
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
        ctx.world.remove_block_entity(pos);
        let old = ctx.set_block(pos, final_state, "piston_movement_settle")?;
        self.sync_entity_sensor(pos, old, final_state);
        ctx.update_neighbors(pos, self.state(old)?.kind, None, None);
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
            ctx.world.set_block_entity(pos, data);
        }
        Ok(true)
    }

    fn set_piston_state_silent(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: BlockStateId,
        cause: &str,
    ) -> Result<bool, RulesError> {
        let old = ctx.set_block(pos, state, cause)?;
        if old == state {
            return Ok(false);
        }
        self.sync_entity_sensor(pos, old, state);
        Ok(true)
    }

    fn resolve_push_structure(
        &self,
        world: &SparseWorld,
        start: BlockPos,
        direction: Direction,
    ) -> Result<Vec<(BlockPos, PushReaction)>, RulesError> {
        let mut result = Vec::new();
        let mut queued = BTreeSet::from([start]);
        let mut queue = VecDeque::from([start]);
        while let Some(cursor) = queue.pop_front() {
            let state_id = world.get_block(cursor);
            if state_id == self.registry.air_state() {
                continue;
            }
            let definition = self.state(state_id)?;
            match definition.push_reaction {
                PushReaction::Block | PushReaction::PushOnly => {
                    return Err(RulesError::Message(format!(
                        "活塞无法推动 {} at {cursor:?}",
                        definition.name
                    )));
                }
                reaction => result.push((cursor, reaction)),
            }
            if result.len() > 12 {
                return Err(RulesError::Message("活塞超过 12 方块推动限制".to_owned()));
            }
            if definition.push_reaction == PushReaction::Destroy {
                continue;
            }
            let forward = cursor.relative(direction);
            if queued.insert(forward) {
                queue.push_back(forward);
            }
            if is_sticky_block(&definition.name) {
                for branch in Direction::UPDATE_ORDER {
                    if branch == direction || branch == direction.opposite() {
                        continue;
                    }
                    let candidate = cursor.relative(branch);
                    let candidate_state = world.get_block(candidate);
                    if candidate_state == self.registry.air_state() {
                        continue;
                    }
                    let candidate_definition = self.state(candidate_state)?;
                    if sticks_to(&definition.name, &candidate_definition.name)
                        && queued.insert(candidate)
                    {
                        queue.push_back(candidate);
                    }
                }
            }
        }
        result.sort_by_key(|(pos, _)| projection(*pos, direction));
        Ok(result)
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

fn projection(pos: BlockPos, direction: Direction) -> i64 {
    let (x, y, z) = direction.step();
    i64::from(pos.x) * i64::from(x)
        + i64::from(pos.y) * i64::from(y)
        + i64::from(pos.z) * i64::from(z)
}

fn moving_block_entity(
    moved_state: BlockStateId,
    direction: Direction,
    settle_tick: u64,
    extending: bool,
    source: bool,
    moved_block_entity: Option<BlockEntityData>,
) -> BlockEntityData {
    let mut fields = BTreeMap::from([
            ("moved_state".to_owned(), serde_json::Value::from(moved_state.0)),
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
