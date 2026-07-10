use redstone_core::{BlockPos, BlockStateId, Direction, EventContext, RulesError, SparseWorld};

use super::{BlockBehavior, Java26Rules, StateDefinition};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShapeFamily {
    Wire,
    Tripwire,
    Fence { wooden: bool },
    Pane,
    Wall,
    Rail { straight: bool },
}

impl Java26Rules {
    pub(super) fn repair_shape(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        notify: bool,
    ) -> Result<BlockStateId, RulesError> {
        let state_id = ctx.world.get_block(pos);
        let state = self.state(state_id)?.clone();
        let Some(family) = shape_family(&state.name) else {
            return Ok(state_id);
        };
        if let ShapeFamily::Rail { straight } = family {
            return self.repair_rail_shape(ctx, pos, &state, straight, notify);
        }
        let repaired = match family {
            ShapeFamily::Wire => self.wire_shape(ctx.world, pos, &state)?,
            ShapeFamily::Tripwire => self.tripwire_shape(ctx.world, pos, &state)?,
            ShapeFamily::Fence { wooden } => {
                self.cross_shape(ctx.world, pos, &state, CrossKind::Fence { wooden })?
            }
            ShapeFamily::Pane => self.cross_shape(ctx.world, pos, &state, CrossKind::Pane)?,
            ShapeFamily::Wall => self.wall_shape(ctx.world, pos, &state)?,
            ShapeFamily::Rail { .. } => unreachable!("rail shapes return above"),
        };
        if repaired == state_id {
            return Ok(state_id);
        }
        if notify {
            self.set_state_and_notify(ctx, pos, repaired, "shape_update", None)?;
        } else {
            ctx.set_block(pos, repaired, "shape_initialize")?;
        }
        Ok(repaired)
    }

    fn repair_rail_shape(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
        straight: bool,
        notify: bool,
    ) -> Result<BlockStateId, RulesError> {
        let default_shape = RailShape::parse(state.property("shape").unwrap_or("north_south"));
        let shape = self.placed_rail_shape(ctx.world, pos, straight, default_shape);
        let repaired = self.changed_state(state.id, "shape", shape.name())?;
        self.write_shape_state(ctx, pos, repaired, notify, "rail_shape")?;

        for connection in shape.connections(pos) {
            let Some((neighbor_pos, neighbor_state)) = self.find_rail(ctx.world, connection)
            else {
                continue;
            };
            let soft_connections = self.soft_rail_connections(ctx.world, neighbor_pos, &neighbor_state);
            if !contains_rail_column(&soft_connections, pos) && soft_connections.len() == 2 {
                continue;
            }
            let neighbor_straight = rail_is_straight(&neighbor_state.name);
            let neighbor_shape = self.connected_rail_shape(
                ctx.world,
                neighbor_pos,
                neighbor_straight,
                soft_connections,
                pos,
            );
            let next = self.changed_state(neighbor_state.id, "shape", neighbor_shape.name())?;
            self.write_shape_state(ctx, neighbor_pos, next, notify, "rail_connect")?;
        }
        Ok(ctx.world.get_block(pos))
    }

    fn write_shape_state(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: BlockStateId,
        notify: bool,
        cause: &str,
    ) -> Result<(), RulesError> {
        if notify {
            self.set_state_and_notify(ctx, pos, state, cause, None)?;
        } else {
            ctx.set_block(pos, state, cause)?;
        }
        Ok(())
    }

    fn placed_rail_shape(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        straight: bool,
        default_shape: RailShape,
    ) -> RailShape {
        let north = self.has_neighbor_rail(world, pos, Direction::North);
        let south = self.has_neighbor_rail(world, pos, Direction::South);
        let west = self.has_neighbor_rail(world, pos, Direction::West);
        let east = self.has_neighbor_rail(world, pos, Direction::East);
        let north_or_south = north || south;
        let west_or_east = west || east;
        let south_and_east = south && east;
        let south_and_west = south && west;
        let north_and_east = north && east;
        let north_and_west = north && west;
        let mut shape = if north_or_south && !west_or_east {
            Some(RailShape::NorthSouth)
        } else if west_or_east && !north_or_south {
            Some(RailShape::EastWest)
        } else {
            None
        };
        if !straight {
            if south_and_east && !north && !west {
                shape = Some(RailShape::SouthEast);
            }
            if south_and_west && !north && !east {
                shape = Some(RailShape::SouthWest);
            }
            if north_and_west && !south && !east {
                shape = Some(RailShape::NorthWest);
            }
            if north_and_east && !south && !west {
                shape = Some(RailShape::NorthEast);
            }
        }
        if shape.is_none() {
            shape = if north_or_south && west_or_east {
                Some(default_shape)
            } else if north_or_south {
                Some(RailShape::NorthSouth)
            } else if west_or_east {
                Some(RailShape::EastWest)
            } else {
                None
            };
            if !straight {
                if self.is_powered(world, pos) {
                    if south_and_east {
                        shape = Some(RailShape::SouthEast);
                    }
                    if south_and_west {
                        shape = Some(RailShape::SouthWest);
                    }
                    if north_and_east {
                        shape = Some(RailShape::NorthEast);
                    }
                    if north_and_west {
                        shape = Some(RailShape::NorthWest);
                    }
                } else {
                    if north_and_west {
                        shape = Some(RailShape::NorthWest);
                    }
                    if north_and_east {
                        shape = Some(RailShape::NorthEast);
                    }
                    if south_and_west {
                        shape = Some(RailShape::SouthWest);
                    }
                    if south_and_east {
                        shape = Some(RailShape::SouthEast);
                    }
                }
            }
        }
        self.ascending_rail_shape(world, pos, shape.unwrap_or(default_shape))
    }

    fn connected_rail_shape(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        straight: bool,
        mut connections: Vec<BlockPos>,
        rail: BlockPos,
    ) -> RailShape {
        connections.push(rail);
        let north = contains_rail_column(&connections, pos.relative(Direction::North));
        let south = contains_rail_column(&connections, pos.relative(Direction::South));
        let west = contains_rail_column(&connections, pos.relative(Direction::West));
        let east = contains_rail_column(&connections, pos.relative(Direction::East));
        let mut shape = None;
        if north || south {
            shape = Some(RailShape::NorthSouth);
        }
        if west || east {
            shape = Some(RailShape::EastWest);
        }
        if !straight {
            if south && east && !north && !west {
                shape = Some(RailShape::SouthEast);
            }
            if south && west && !north && !east {
                shape = Some(RailShape::SouthWest);
            }
            if north && west && !south && !east {
                shape = Some(RailShape::NorthWest);
            }
            if north && east && !south && !west {
                shape = Some(RailShape::NorthEast);
            }
        }
        self.ascending_rail_shape(world, pos, shape.unwrap_or(RailShape::NorthSouth))
    }

    fn ascending_rail_shape(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        mut shape: RailShape,
    ) -> RailShape {
        if shape == RailShape::NorthSouth {
            if self.is_rail_at(world, pos.relative(Direction::North).relative(Direction::Up)) {
                shape = RailShape::AscendingNorth;
            }
            if self.is_rail_at(world, pos.relative(Direction::South).relative(Direction::Up)) {
                shape = RailShape::AscendingSouth;
            }
        }
        if shape == RailShape::EastWest {
            if self.is_rail_at(world, pos.relative(Direction::East).relative(Direction::Up)) {
                shape = RailShape::AscendingEast;
            }
            if self.is_rail_at(world, pos.relative(Direction::West).relative(Direction::Up)) {
                shape = RailShape::AscendingWest;
            }
        }
        shape
    }

    fn has_neighbor_rail(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        direction: Direction,
    ) -> bool {
        let Some((neighbor_pos, neighbor_state)) = self.find_rail(world, pos.relative(direction))
        else {
            return false;
        };
        let connections = self.soft_rail_connections(world, neighbor_pos, &neighbor_state);
        contains_rail_column(&connections, pos) || connections.len() != 2
    }

    fn soft_rail_connections(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        state: &StateDefinition,
    ) -> Vec<BlockPos> {
        RailShape::parse(state.property("shape").unwrap_or("north_south"))
            .connections(pos)
            .into_iter()
            .filter_map(|connection| {
                let (rail_pos, rail_state) = self.find_rail(world, connection)?;
                let rail_shape = RailShape::parse(
                    rail_state.property("shape").unwrap_or("north_south"),
                );
                rail_shape
                    .connections(rail_pos)
                    .into_iter()
                    .any(|candidate| same_rail_column(candidate, pos))
                    .then_some(rail_pos)
            })
            .collect()
    }

    fn find_rail(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
    ) -> Option<(BlockPos, StateDefinition)> {
        [pos, pos.relative(Direction::Up), pos.relative(Direction::Down)]
            .into_iter()
            .find_map(|candidate| {
                let state = self.state_or_air(world, candidate);
                is_rail_name(&state.name).then(|| (candidate, state.clone()))
            })
    }

    fn is_rail_at(&self, world: &SparseWorld, pos: BlockPos) -> bool {
        is_rail_name(&self.state_or_air(world, pos).name)
    }

    fn wire_shape(
        &mut self,
        world: &SparseWorld,
        pos: BlockPos,
        state: &StateDefinition,
    ) -> Result<BlockStateId, RulesError> {
        let was_dot = Direction::HORIZONTAL
            .into_iter()
            .all(|direction| state.property(direction_name(direction)) == Some("none"));
        let mut connections = Direction::HORIZONTAL.map(|direction| {
            self.wire_connection(world, pos, direction)
        });
        let connected = connections.map(|connection| connection != "none");
        if !(was_dot && connected.into_iter().all(|connected| !connected)) {
            let north_south_empty = !connected[0] && !connected[2];
            let east_west_empty = !connected[1] && !connected[3];
            if !connected[3] && north_south_empty {
                connections[3] = "side";
            }
            if !connected[1] && north_south_empty {
                connections[1] = "side";
            }
            if !connected[0] && east_west_empty {
                connections[0] = "side";
            }
            if !connected[2] && east_west_empty {
                connections[2] = "side";
            }
        }
        self.with_horizontal_properties(state.id, connections)
    }

    fn wire_connection(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        direction: Direction,
    ) -> &'static str {
        let side = pos.relative(direction);
        let side_state = self.state_or_air(world, side);
        let above_open = !self.state_or_air(world, pos.relative(Direction::Up)).redstone_conductor;
        if above_open
            && (side_state.name.ends_with("_trapdoor")
                || side_state.sturdy(Direction::Up)
                || matches!(side_state.behavior, BlockBehavior::Hopper))
            && self.wire_connects_to(
                self.state_or_air(world, side.relative(Direction::Up)),
                None,
            )
        {
            return if side_state.sturdy(direction.opposite()) {
                "up"
            } else {
                "side"
            };
        }
        if self.wire_connects_to(side_state, Some(direction))
            || (!side_state.redstone_conductor
                && self.wire_connects_to(
                    self.state_or_air(world, side.relative(Direction::Down)),
                    None,
                ))
        {
            "side"
        } else {
            "none"
        }
    }

    fn wire_connects_to(
        &self,
        state: &StateDefinition,
        direction: Option<Direction>,
    ) -> bool {
        match state.behavior {
            BlockBehavior::Wire => true,
            BlockBehavior::Repeater | BlockBehavior::Comparator => direction.is_some_and(|direction| {
                let facing = state.direction_property("facing").unwrap_or(Direction::North);
                facing == direction || facing.opposite() == direction
            }),
            BlockBehavior::Observer => {
                direction.is_some_and(|direction| state.direction_property("facing") == Some(direction))
            }
            _ => direction.is_some() && is_signal_source(&state.behavior),
        }
    }

    fn tripwire_shape(
        &mut self,
        world: &SparseWorld,
        pos: BlockPos,
        state: &StateDefinition,
    ) -> Result<BlockStateId, RulesError> {
        let connections = Direction::HORIZONTAL.map(|direction| {
            let neighbor = self.state_or_air(world, pos.relative(direction));
            matches!(neighbor.behavior, BlockBehavior::Tripwire)
                || matches!(neighbor.behavior, BlockBehavior::TripwireHook)
                    && neighbor.direction_property("facing") == Some(direction.opposite())
        });
        self.with_horizontal_properties(
            state.id,
            connections.map(|connected| if connected { "true" } else { "false" }),
        )
    }

    fn cross_shape(
        &mut self,
        world: &SparseWorld,
        pos: BlockPos,
        state: &StateDefinition,
        kind: CrossKind,
    ) -> Result<BlockStateId, RulesError> {
        let connections = Direction::HORIZONTAL.map(|direction| {
            let neighbor = self.state_or_air(world, pos.relative(direction));
            let connected = match kind {
                CrossKind::Fence { wooden } => {
                    fence_connects(neighbor, direction.opposite(), wooden)
                }
                CrossKind::Pane => pane_connects(neighbor, direction.opposite()),
            };
            if connected { "true" } else { "false" }
        });
        self.with_horizontal_properties(state.id, connections)
    }

    fn wall_shape(
        &mut self,
        world: &SparseWorld,
        pos: BlockPos,
        state: &StateDefinition,
    ) -> Result<BlockStateId, RulesError> {
        let above = self.state_or_air(world, pos.relative(Direction::Up));
        let covered = above.redstone_conductor;
        let above_wall_post = above.name.ends_with("_wall") && above.bool_property("up");
        let connections = Direction::HORIZONTAL.map(|direction| {
            let neighbor = self.state_or_air(world, pos.relative(direction));
            wall_connects(neighbor, direction.opposite())
        });
        let sides = connections.map(|connected| {
            if !connected {
                "none"
            } else if covered {
                "tall"
            } else {
                "low"
            }
        });
        let mut next = self.with_horizontal_properties(state.id, sides)?;
        let north_none = !connections[0];
        let east_none = !connections[1];
        let south_none = !connections[2];
        let west_none = !connections[3];
        let has_corner = north_none && east_none && south_none && west_none
            || north_none != south_none
            || west_none != east_none;
        let straight_tall = covered
            && (connections[0] && connections[2] || connections[1] && connections[3]);
        let raise_post = above_wall_post || has_corner || !straight_tall && covered;
        next = self.changed_state(next, "up", raise_post.to_string())?;
        Ok(next)
    }

    fn with_horizontal_properties(
        &mut self,
        state: BlockStateId,
        values: [&str; 4],
    ) -> Result<BlockStateId, RulesError> {
        Direction::HORIZONTAL
            .into_iter()
            .zip(values)
            .try_fold(state, |state, (direction, value)| {
                self.changed_state(state, direction_name(direction), value)
            })
    }

    fn state_or_air<'a>(&'a self, world: &SparseWorld, pos: BlockPos) -> &'a StateDefinition {
        self.registry
            .state(world.get_block(pos))
            .or_else(|| self.registry.state(self.registry.air_state()))
            .expect("air state must be loaded")
    }
}

#[derive(Clone, Copy)]
enum CrossKind {
    Fence { wooden: bool },
    Pane,
}

fn shape_family(name: &str) -> Option<ShapeFamily> {
    let path = name.strip_prefix("minecraft:").unwrap_or(name);
    if path == "redstone_wire" {
        Some(ShapeFamily::Wire)
    } else if path == "tripwire" {
        Some(ShapeFamily::Tripwire)
    } else if path.ends_with("_fence") {
        Some(ShapeFamily::Fence {
            wooden: path != "nether_brick_fence",
        })
    } else if path.contains("glass_pane") || path.ends_with("_bars") {
        Some(ShapeFamily::Pane)
    } else if path.ends_with("_wall") {
        Some(ShapeFamily::Wall)
    } else if path == "rail" || path.ends_with("_rail") {
        Some(ShapeFamily::Rail {
            straight: path != "rail",
        })
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RailShape {
    NorthSouth,
    EastWest,
    AscendingEast,
    AscendingWest,
    AscendingNorth,
    AscendingSouth,
    SouthEast,
    SouthWest,
    NorthWest,
    NorthEast,
}

impl RailShape {
    fn parse(value: &str) -> Self {
        match value {
            "east_west" => Self::EastWest,
            "ascending_east" => Self::AscendingEast,
            "ascending_west" => Self::AscendingWest,
            "ascending_north" => Self::AscendingNorth,
            "ascending_south" => Self::AscendingSouth,
            "south_east" => Self::SouthEast,
            "south_west" => Self::SouthWest,
            "north_west" => Self::NorthWest,
            "north_east" => Self::NorthEast,
            _ => Self::NorthSouth,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::NorthSouth => "north_south",
            Self::EastWest => "east_west",
            Self::AscendingEast => "ascending_east",
            Self::AscendingWest => "ascending_west",
            Self::AscendingNorth => "ascending_north",
            Self::AscendingSouth => "ascending_south",
            Self::SouthEast => "south_east",
            Self::SouthWest => "south_west",
            Self::NorthWest => "north_west",
            Self::NorthEast => "north_east",
        }
    }

    fn connections(self, pos: BlockPos) -> [BlockPos; 2] {
        match self {
            Self::NorthSouth => [
                pos.relative(Direction::North),
                pos.relative(Direction::South),
            ],
            Self::EastWest => [
                pos.relative(Direction::West),
                pos.relative(Direction::East),
            ],
            Self::AscendingEast => [
                pos.relative(Direction::West),
                pos.relative(Direction::East).relative(Direction::Up),
            ],
            Self::AscendingWest => [
                pos.relative(Direction::West).relative(Direction::Up),
                pos.relative(Direction::East),
            ],
            Self::AscendingNorth => [
                pos.relative(Direction::North).relative(Direction::Up),
                pos.relative(Direction::South),
            ],
            Self::AscendingSouth => [
                pos.relative(Direction::North),
                pos.relative(Direction::South).relative(Direction::Up),
            ],
            Self::SouthEast => [
                pos.relative(Direction::East),
                pos.relative(Direction::South),
            ],
            Self::SouthWest => [
                pos.relative(Direction::West),
                pos.relative(Direction::South),
            ],
            Self::NorthWest => [
                pos.relative(Direction::West),
                pos.relative(Direction::North),
            ],
            Self::NorthEast => [
                pos.relative(Direction::East),
                pos.relative(Direction::North),
            ],
        }
    }
}

fn is_rail_name(name: &str) -> bool {
    let path = name.strip_prefix("minecraft:").unwrap_or(name);
    path == "rail" || path.ends_with("_rail")
}

fn rail_is_straight(name: &str) -> bool {
    name.strip_prefix("minecraft:").unwrap_or(name) != "rail"
}

fn contains_rail_column(positions: &[BlockPos], pos: BlockPos) -> bool {
    positions
        .iter()
        .any(|candidate| same_rail_column(*candidate, pos))
}

fn same_rail_column(left: BlockPos, right: BlockPos) -> bool {
    left.x == right.x && left.z == right.z
}

fn fence_connects(state: &StateDefinition, direction: Direction, wooden: bool) -> bool {
    let path = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    let same_fence = path.ends_with("_fence")
        && (path != "nether_brick_fence") == wooden;
    let gate = path.ends_with("_fence_gate")
        && state
            .direction_property("facing")
            .is_some_and(|facing| facing_axis(facing) != facing_axis(direction));
    same_fence || gate || state.sturdy(direction) && !connection_exception(path)
}

fn pane_connects(state: &StateDefinition, direction: Direction) -> bool {
    let path = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    path.contains("glass_pane")
        || path.ends_with("_bars")
        || path.ends_with("_wall")
        || state.sturdy(direction) && !connection_exception(path)
}

fn wall_connects(state: &StateDefinition, direction: Direction) -> bool {
    let path = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    path.ends_with("_wall")
        || path.contains("glass_pane")
        || path.ends_with("_bars")
        || path.ends_with("_fence_gate")
            && state
                .direction_property("facing")
                .is_some_and(|facing| facing_axis(facing) != facing_axis(direction))
        || state.sturdy(direction) && !connection_exception(path)
}

fn connection_exception(path: &str) -> bool {
    path.ends_with("_leaves")
        || path.ends_with("_shulker_box")
        || matches!(
            path,
            "barrier" | "carved_pumpkin" | "jack_o_lantern" | "melon" | "pumpkin"
        )
}

fn is_signal_source(behavior: &BlockBehavior) -> bool {
    matches!(
        behavior,
        BlockBehavior::Lever
            | BlockBehavior::Button { .. }
            | BlockBehavior::RedstoneBlock
            | BlockBehavior::Torch { .. }
            | BlockBehavior::Target
            | BlockBehavior::PressurePlate { .. }
            | BlockBehavior::TripwireHook
            | BlockBehavior::DetectorRail
            | BlockBehavior::DaylightDetector
            | BlockBehavior::Lectern
            | BlockBehavior::TrappedChest
    )
}

fn facing_axis(direction: Direction) -> u8 {
    match direction {
        Direction::West | Direction::East => 0,
        Direction::Down | Direction::Up => 1,
        Direction::North | Direction::South => 2,
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::West => "west",
        Direction::East => "east",
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
    }
}
