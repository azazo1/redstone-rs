use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Position {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub const fn offset(self, direction: Direction) -> Self {
        Self::new(
            self.x + direction.step_x(),
            self.y + direction.step_y(),
            self.z + direction.step_z(),
        )
    }

    pub const fn relative(self, direction: Direction, distance: i32) -> Self {
        Self::new(
            self.x + direction.step_x() * distance,
            self.y + direction.step_y() * distance,
            self.z + direction.step_z() * distance,
        )
    }
}

impl fmt::Display for Position {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {} {}", self.x, self.y, self.z)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Direction {
    #[default]
    North = 0,
    South = 1,
    West = 2,
    East = 3,
    Down = 4,
    Up = 5,
}

impl Direction {
    pub const NEIGHBOR_ORDER: [Self; 6] = [
        Self::West,
        Self::East,
        Self::Down,
        Self::Up,
        Self::North,
        Self::South,
    ];

    pub const ALL: [Self; 6] = [
        Self::North,
        Self::South,
        Self::West,
        Self::East,
        Self::Down,
        Self::Up,
    ];

    pub const HORIZONTAL: [Self; 4] = [Self::North, Self::South, Self::West, Self::East];

    pub const fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::West => Self::East,
            Self::East => Self::West,
            Self::Down => Self::Up,
            Self::Up => Self::Down,
        }
    }

    pub const fn step_x(self) -> i32 {
        match self {
            Self::West => -1,
            Self::East => 1,
            _ => 0,
        }
    }

    pub const fn step_y(self) -> i32 {
        match self {
            Self::Down => -1,
            Self::Up => 1,
            _ => 0,
        }
    }

    pub const fn step_z(self) -> i32 {
        match self {
            Self::North => -1,
            Self::South => 1,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum BlockKind {
    #[default]
    Air,
    Solid,
    Glass,
    Immovable,
    RedstoneBlock,
    RedstoneWire,
    RedstoneTorch,
    Lever,
    Button,
    PressurePlate,
    Repeater,
    Comparator,
    Observer,
    Lamp,
    CopperBulb,
    DaylightDetector,
    Target,
    Door,
    Trapdoor,
    FenceGate,
    NoteBlock,
    Dropper,
    Dispenser,
    Crafter,
    Tnt,
    Container,
    Piston,
    StickyPiston,
    PistonHead,
    MovingPiston,
    SlimeBlock,
    HoneyBlock,
}

impl BlockKind {
    pub const fn is_piston(self) -> bool {
        matches!(self, Self::Piston | Self::StickyPiston)
    }

    pub const fn is_conductor(self) -> bool {
        matches!(
            self,
            Self::Solid
                | Self::Immovable
                | Self::RedstoneBlock
                | Self::Container
                | Self::Piston
                | Self::StickyPiston
                | Self::SlimeBlock
                | Self::HoneyBlock
        )
    }

    pub const fn is_sticky(self) -> bool {
        matches!(self, Self::SlimeBlock | Self::HoneyBlock)
    }

    pub const fn is_immovable(self) -> bool {
        matches!(self, Self::Immovable | Self::PistonHead | Self::MovingPiston)
    }

    pub const fn is_piston_destroyable(self) -> bool {
        matches!(
            self,
            Self::RedstoneWire | Self::RedstoneTorch | Self::Lever | Self::Button | Self::PressurePlate
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ComparatorMode {
    #[default]
    Compare,
    Subtract,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlockState {
    pub kind: BlockKind,
    bits: u32,
}

impl BlockState {
    const FACING_MASK: u32 = 0b111;
    const POWERED: u32 = 1 << 3;
    const POWER_SHIFT: u32 = 4;
    const POWER_MASK: u32 = 0b1111 << Self::POWER_SHIFT;
    const DELAY_SHIFT: u32 = 8;
    const DELAY_MASK: u32 = 0b11 << Self::DELAY_SHIFT;
    const LOCKED: u32 = 1 << 10;
    const SUBTRACT: u32 = 1 << 11;
    const EXTENDED: u32 = 1 << 12;
    const LIT: u32 = 1 << 13;

    pub const fn new(kind: BlockKind) -> Self {
        let bits = if matches!(kind, BlockKind::RedstoneTorch) {
            Self::POWERED
        } else {
            0
        };
        Self { kind, bits }
    }

    pub const fn with_facing(mut self, facing: Direction) -> Self {
        self.bits = (self.bits & !Self::FACING_MASK) | facing as u32;
        self
    }

    pub const fn facing(self) -> Direction {
        match self.bits & Self::FACING_MASK {
            0 => Direction::North,
            1 => Direction::South,
            2 => Direction::West,
            3 => Direction::East,
            4 => Direction::Down,
            _ => Direction::Up,
        }
    }

    pub const fn with_powered(mut self, powered: bool) -> Self {
        if powered {
            self.bits |= Self::POWERED;
        } else {
            self.bits &= !Self::POWERED;
        }
        self
    }

    pub const fn powered(self) -> bool {
        self.bits & Self::POWERED != 0
    }

    pub const fn with_power(mut self, power: u8) -> Self {
        let power = if power > 15 { 15 } else { power };
        self.bits = (self.bits & !Self::POWER_MASK) | ((power as u32) << Self::POWER_SHIFT);
        self
    }

    pub const fn power(self) -> u8 {
        ((self.bits & Self::POWER_MASK) >> Self::POWER_SHIFT) as u8
    }

    pub const fn with_delay(mut self, delay: u8) -> Self {
        let delay = if delay < 1 {
            1
        } else if delay > 4 {
            4
        } else {
            delay
        };
        self.bits = (self.bits & !Self::DELAY_MASK) | (((delay - 1) as u32) << Self::DELAY_SHIFT);
        self
    }

    pub const fn delay(self) -> u8 {
        ((self.bits & Self::DELAY_MASK) >> Self::DELAY_SHIFT) as u8 + 1
    }

    pub const fn with_locked(mut self, locked: bool) -> Self {
        if locked {
            self.bits |= Self::LOCKED;
        } else {
            self.bits &= !Self::LOCKED;
        }
        self
    }

    pub const fn locked(self) -> bool {
        self.bits & Self::LOCKED != 0
    }

    pub const fn with_mode(mut self, mode: ComparatorMode) -> Self {
        if matches!(mode, ComparatorMode::Subtract) {
            self.bits |= Self::SUBTRACT;
        } else {
            self.bits &= !Self::SUBTRACT;
        }
        self
    }

    pub const fn mode(self) -> ComparatorMode {
        if self.bits & Self::SUBTRACT != 0 {
            ComparatorMode::Subtract
        } else {
            ComparatorMode::Compare
        }
    }

    pub const fn with_extended(mut self, extended: bool) -> Self {
        if extended {
            self.bits |= Self::EXTENDED;
        } else {
            self.bits &= !Self::EXTENDED;
        }
        self
    }

    pub const fn extended(self) -> bool {
        self.bits & Self::EXTENDED != 0
    }

    pub const fn with_lit(mut self, lit: bool) -> Self {
        if lit {
            self.bits |= Self::LIT;
        } else {
            self.bits &= !Self::LIT;
        }
        self
    }

    pub const fn lit(self) -> bool {
        self.bits & Self::LIT != 0
    }
}
