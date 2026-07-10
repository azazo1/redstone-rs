use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct BlockStateId(pub u32);

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct BlockKindId(pub u32);

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct GameTick(pub u64);

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct MicroStep(pub u64);

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct EntityId(pub u64);

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub const ZERO: Self = Self { x: 0, y: 0, z: 0 };

    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub const fn relative(self, direction: Direction) -> Self {
        let (x, y, z) = direction.step();
        Self::new(self.x + x, self.y + y, self.z + z)
    }

    pub const fn offset(self, x: i32, y: i32, z: i32) -> Self {
        Self::new(self.x + x, self.y + y, self.z + z)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    West,
    East,
    Down,
    Up,
    North,
    South,
}

impl Direction {
    pub const UPDATE_ORDER: [Self; 6] = [
        Self::West,
        Self::East,
        Self::Down,
        Self::Up,
        Self::North,
        Self::South,
    ];

    pub const HORIZONTAL: [Self; 4] = [Self::North, Self::East, Self::South, Self::West];

    pub const fn step(self) -> (i32, i32, i32) {
        match self {
            Self::West => (-1, 0, 0),
            Self::East => (1, 0, 0),
            Self::Down => (0, -1, 0),
            Self::Up => (0, 1, 0),
            Self::North => (0, 0, -1),
            Self::South => (0, 0, 1),
        }
    }

    pub const fn opposite(self) -> Self {
        match self {
            Self::West => Self::East,
            Self::East => Self::West,
            Self::Down => Self::Up,
            Self::Up => Self::Down,
            Self::North => Self::South,
            Self::South => Self::North,
        }
    }

    pub const fn clockwise(self) -> Self {
        match self {
            Self::North => Self::East,
            Self::East => Self::South,
            Self::South => Self::West,
            Self::West => Self::North,
            other => other,
        }
    }

    pub const fn counter_clockwise(self) -> Self {
        match self {
            Self::North => Self::West,
            Self::West => Self::South,
            Self::South => Self::East,
            Self::East => Self::North,
            other => other,
        }
    }

    pub const fn is_horizontal(self) -> bool {
        matches!(self, Self::West | Self::East | Self::North | Self::South)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RedstoneMode {
    #[default]
    Default,
    Experimental,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimulationPhase {
    PreTick,
    ScheduledTicks,
    BlockEvents,
    Entities,
    BlockEntities,
    PostTick,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BlockEntityData {
    pub kind: String,
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EntityData {
    pub kind: String,
    pub position: [f64; 3],
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    SetBlock { pos: BlockPos, state: BlockStateId },
    BreakBlock { pos: BlockPos },
    UseBlock { pos: BlockPos },
    PressButton { pos: BlockPos },
    PullLever { pos: BlockPos },
    SetBlockEntity {
        pos: BlockPos,
        data: BlockEntityData,
    },
    SpawnEntity {
        #[serde(default)]
        id: Option<EntityId>,
        data: EntityData,
    },
    MoveEntity {
        id: EntityId,
        position: [f64; 3],
    },
    RemoveEntity {
        id: EntityId,
    },
    SetEntityField {
        id: EntityId,
        field: String,
        value: serde_json::Value,
    },
    HitTarget {
        pos: BlockPos,
        face: Direction,
        location: [f64; 3],
        #[serde(default)]
        arrow: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Probe {
    Signal {
        pos: BlockPos,
        #[serde(default)]
        direction: Option<Direction>,
    },
    BlockState { pos: BlockPos },
    Property { pos: BlockPos, property: String },
    ContainerCount { pos: BlockPos },
    EntityCount { kind: Option<String> },
    EntityField { id: EntityId, field: String },
    EntityContainerCount { id: EntityId },
    EventCount { kind: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ProbeValue {
    Bool(bool),
    Integer(i64),
    String(String),
    State(BlockStateId),
    None,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbeSample {
    pub name: String,
    pub probe: Probe,
    pub value: ProbeValue,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Expectation {
    pub tick: GameTick,
    pub probe: String,
    pub equals: ProbeValue,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BlockChange {
    pub pos: BlockPos,
    pub old_state: BlockStateId,
    pub new_state: BlockStateId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorldDelta {
    pub tick: GameTick,
    pub changes: Vec<BlockChange>,
    pub probes: Vec<ProbeSample>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TraceEvent {
    pub tick: GameTick,
    pub micro_step: MicroStep,
    pub phase: SimulationPhase,
    #[serde(flatten)]
    pub kind: TraceKind,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TraceKind {
    Action { action: Action },
    BlockChanged {
        pos: BlockPos,
        old_state: BlockStateId,
        new_state: BlockStateId,
        cause: String,
    },
    NeighborUpdate {
        pos: BlockPos,
        source_pos: BlockPos,
        source_block: BlockKindId,
        moved_by_piston: bool,
        orientation: Option<u8>,
    },
    ScheduledTickQueued {
        pos: BlockPos,
        block: BlockKindId,
        trigger_tick: GameTick,
        priority: i8,
        sub_tick_order: i64,
    },
    ScheduledTickExecuted { pos: BlockPos, block: BlockKindId },
    BlockEventQueued {
        pos: BlockPos,
        block: BlockKindId,
        param_a: i32,
        param_b: i32,
    },
    BlockEventExecuted {
        pos: BlockPos,
        block: BlockKindId,
        param_a: i32,
        param_b: i32,
    },
    ProbeSample { sample: ProbeSample },
    UnsupportedTrigger { pos: BlockPos, behavior: String },
    Message { level: String, message: String },
}
