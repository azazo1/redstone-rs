use super::{BlockKind, Direction, Position};

#[derive(Clone, Debug)]
pub enum NeighborUpdate {
    Single {
        position: Position,
        changed_block: BlockKind,
        source: Position,
    },
    Multi {
        source: Position,
        changed_block: BlockKind,
        skip: Option<Direction>,
        next_index: usize,
    },
}

impl NeighborUpdate {
    pub fn take_next_event(&mut self) -> Option<(Position, BlockKind, Position)> {
        match self {
            Self::Single {
                position,
                changed_block,
                source,
            } => {
                let event = (*position, *changed_block, *source);
                *self = Self::Multi {
                    source: Position::default(),
                    changed_block: BlockKind::Air,
                    skip: None,
                    next_index: Direction::NEIGHBOR_ORDER.len(),
                };
                Some(event)
            }
            Self::Multi {
                source,
                changed_block,
                skip,
                next_index,
            } => {
                while *next_index < Direction::NEIGHBOR_ORDER.len() {
                    let direction = Direction::NEIGHBOR_ORDER[*next_index];
                    *next_index += 1;
                    if Some(direction) != *skip {
                        return Some((source.offset(direction), *changed_block, *source));
                    }
                }
                None
            }
        }
    }
}

#[derive(Debug)]
pub struct NeighborUpdater {
    limit: usize,
    count: usize,
    running: bool,
    stack: Vec<NeighborUpdate>,
    added_this_layer: Vec<NeighborUpdate>,
}

impl NeighborUpdater {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            count: 0,
            running: false,
            stack: Vec::new(),
            added_this_layer: Vec::new(),
        }
    }

    pub fn enqueue(&mut self, update: NeighborUpdate) -> bool {
        self.count = self.count.saturating_add(1);
        if self.count > self.limit {
            return false;
        }
        let should_start = !self.running;
        if self.running {
            self.added_this_layer.push(update);
        } else {
            self.stack.push(update);
        }
        should_start
    }

    pub fn begin(&mut self) {
        self.running = true;
    }

    pub fn begin_if_idle(&mut self) -> bool {
        if self.running {
            false
        } else {
            self.begin();
            true
        }
    }

    pub fn take_next_event(&mut self) -> Option<(Position, BlockKind, Position)> {
        loop {
            if !self.added_this_layer.is_empty() {
                for update in self.added_this_layer.drain(..).rev() {
                    self.stack.push(update);
                }
            }
            let mut update = self.stack.pop()?;
            if let Some(event) = update.take_next_event() {
                if matches!(update, NeighborUpdate::Multi { .. }) {
                    self.stack.push(update);
                }
                return Some(event);
            }
        }
    }

    pub fn finish(&mut self) {
        self.running = false;
        self.count = 0;
        self.stack.clear();
        self.added_this_layer.clear();
    }
}
