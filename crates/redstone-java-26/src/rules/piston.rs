use std::collections::{BTreeMap, BTreeSet, VecDeque};

use redstone_core::{
    BlockEntityData, BlockEvent, BlockPos, BlockStateId, Direction, EventContext, RulesError,
    SparseWorld,
};

use super::{
    BlockBehavior, Java26Rules, PushReaction, direction_index, direction_name,
};

impl Java26Rules {
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
            ctx.queue_block_event(BlockEvent {
                pos,
                block: state.kind,
                param_a: if should_extend { 0 } else { 1 },
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
        extend: bool,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
        let sticky = matches!(state.behavior, BlockBehavior::Piston { sticky: true });
        if extend {
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
            for (source, _, _, _) in snapshots.iter().rev() {
                self.set_state_and_notify_piston(
                    ctx,
                    *source,
                    self.registry.air_state(),
                    "piston_source_clear",
                )?;
                ctx.world.remove_block_entity(*source);
            }
            for (source, reaction, source_state, block_entity) in snapshots.into_iter().rev() {
                if reaction == PushReaction::Destroy {
                    continue;
                }
                let destination = source.relative(facing);
                self.set_state_and_notify_piston(ctx, destination, source_state, "piston_push")?;
                ctx.world.set_block_entity(
                    destination,
                    moving_block_entity(
                        source_state,
                        facing,
                        ctx.tick.0.saturating_add(1),
                        block_entity,
                    ),
                );
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
            self.set_state_and_notify_piston(ctx, pos.relative(facing), head, "piston_head")?;
            let extended = self.changed_state(state_id, "extended", "true")?;
            self.set_state_and_notify_piston(ctx, pos, extended, "piston_extend")?;
        } else {
            if self.is_quasi_powered(ctx.world, pos, facing) {
                return Ok(());
            }
            self.set_state_and_notify_piston(
                ctx,
                pos.relative(facing),
                self.registry.air_state(),
                "piston_head_remove",
            )?;
            if sticky {
                let source = pos.relative(facing).relative(facing);
                let source_state = ctx.world.get_block(source);
                let definition = self.state(source_state)?.clone();
                if source_state != self.registry.air_state()
                    && definition.push_reaction == PushReaction::Normal
                {
                    self.set_state_and_notify_piston(
                        ctx,
                        pos.relative(facing),
                        source_state,
                        "sticky_piston_pull",
                    )?;
                    self.set_state_and_notify_piston(
                        ctx,
                        source,
                        self.registry.air_state(),
                        "sticky_piston_pull_clear",
                    )?;
                    if let Some(data) = ctx.world.remove_block_entity(source) {
                        ctx.world.set_block_entity(pos.relative(facing), data);
                    }
                }
            }
            let retracted = self.changed_state(state_id, "extended", "false")?;
            self.set_state_and_notify_piston(ctx, pos, retracted, "piston_retract")?;
        }
        Ok(())
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
    moved_block_entity: Option<BlockEntityData>,
) -> BlockEntityData {
    let mut fields = BTreeMap::from([
            ("moved_state".to_owned(), serde_json::Value::from(moved_state.0)),
            ("direction".to_owned(), serde_json::Value::from(direction_name(direction))),
            ("settle_tick".to_owned(), serde_json::Value::from(settle_tick)),
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
