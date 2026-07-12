use redstone_core::{BlockPos, BlockStateId};

pub(crate) const SECTION_SIZE: i32 = 16;
pub(crate) const SECTION_VOLUME: usize = 16 * 16 * 16;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SectionPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectionPos {
    pub(crate) fn from_block(pos: BlockPos) -> Self {
        Self {
            x: pos.x.div_euclid(SECTION_SIZE),
            y: pos.y.div_euclid(SECTION_SIZE),
            z: pos.z.div_euclid(SECTION_SIZE),
        }
    }

    pub(crate) fn origin(self) -> BlockPos {
        BlockPos::new(
            self.x * SECTION_SIZE,
            self.y * SECTION_SIZE,
            self.z * SECTION_SIZE,
        )
    }

    pub(crate) fn offset(self, x: i32, y: i32, z: i32) -> Self {
        Self {
            x: self.x + x,
            y: self.y + y,
            z: self.z + z,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Section {
    blocks: Box<[BlockStateId; SECTION_VOLUME]>,
    non_air: usize,
}

impl Section {
    pub(crate) fn new(air: BlockStateId) -> Self {
        Self {
            blocks: Box::new([air; SECTION_VOLUME]),
            non_air: 0,
        }
    }

    pub(crate) fn get(&self, index: usize) -> BlockStateId {
        self.blocks[index]
    }

    pub(crate) fn set(&mut self, index: usize, state: BlockStateId, air: BlockStateId) {
        let old = self.blocks[index];
        if old == air && state != air {
            self.non_air += 1;
        } else if old != air && state == air {
            self.non_air -= 1;
        }
        self.blocks[index] = state;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.non_air == 0
    }
}

pub(crate) fn local_index(pos: BlockPos) -> usize {
    let x = pos.x.rem_euclid(SECTION_SIZE) as usize;
    let y = pos.y.rem_euclid(SECTION_SIZE) as usize;
    let z = pos.z.rem_euclid(SECTION_SIZE) as usize;
    index(x, y, z)
}

pub(crate) const fn index(x: usize, y: usize, z: usize) -> usize {
    x + z * 16 + y * 256
}

pub(crate) fn local_pos(section: SectionPos, index: usize) -> BlockPos {
    let x = index % 16;
    let z = (index / 16) % 16;
    let y = index / 256;
    let origin = section.origin();
    BlockPos::new(origin.x + x as i32, origin.y + y as i32, origin.z + z as i32)
}
