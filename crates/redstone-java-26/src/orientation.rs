use redstone_core::Direction;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Orientation {
    up: Direction,
    front: Direction,
    side_bias: SideBias,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SideBias {
    Left,
    Right,
}

impl Orientation {
    pub fn from_index(index: u8) -> Self {
        for up in JAVA_DIRECTIONS {
            for front in JAVA_DIRECTIONS {
                if same_axis(up, front) {
                    continue;
                }
                for side_bias in [SideBias::Left, SideBias::Right] {
                    let orientation = Self {
                        up,
                        front,
                        side_bias,
                    };
                    if orientation.index() == index {
                        return orientation;
                    }
                }
            }
        }
        panic!("invalid orientation index: {index}");
    }

    pub fn index(self) -> u8 {
        let front_axis_key = if axis(self.up) == Axis::Y {
            u8::from(axis(self.front) == Axis::X)
        } else {
            u8::from(axis(self.front) == Axis::Y)
        };
        let front_key = front_axis_key << 1 | axis_direction(self.front);
        (((java_ordinal(self.up) << 2) + front_key) << 1) + self.side_bias.ordinal()
    }

    pub fn with_up(self, up: Direction) -> Self {
        let mut front = self.front;
        if up == self.front {
            front = self.up.opposite();
        }
        if up == self.front.opposite() {
            front = self.up;
        }
        Self {
            up,
            front,
            side_bias: self.side_bias,
        }
    }

    pub fn with_front(self, front: Direction) -> Self {
        let mut up = self.up;
        if front == self.up {
            up = self.front.opposite();
        }
        if front == self.up.opposite() {
            up = self.front;
        }
        Self {
            up,
            front,
            side_bias: self.side_bias,
        }
    }

    pub fn with_front_preserve_up(self, front: Direction) -> Self {
        if same_axis(front, self.up) {
            self
        } else {
            self.with_front(front)
        }
    }

    pub fn with_side_bias(self, side_bias: SideBias) -> Self {
        Self { side_bias, ..self }
    }

    pub fn directions(self) -> [Direction; 6] {
        let side = self.side();
        [
            self.front.opposite(),
            self.front,
            side,
            side.opposite(),
            self.up.opposite(),
            self.up,
        ]
    }

    pub fn horizontal_directions(self) -> [Direction; 4] {
        let directions = self.directions();
        [directions[0], directions[1], directions[2], directions[3]]
    }

    pub fn vertical_directions(self) -> [Direction; 2] {
        [self.up.opposite(), self.up]
    }

    fn side(self) -> Direction {
        let cross = cross(vector(self.front), vector(self.up));
        let side = direction_from_vector(cross);
        match self.side_bias {
            SideBias::Right => side,
            SideBias::Left => side.opposite(),
        }
    }
}

impl SideBias {
    const fn ordinal(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Axis {
    X,
    Y,
    Z,
}

const JAVA_DIRECTIONS: [Direction; 6] = [
    Direction::Down,
    Direction::Up,
    Direction::North,
    Direction::South,
    Direction::West,
    Direction::East,
];

const fn axis(direction: Direction) -> Axis {
    match direction {
        Direction::West | Direction::East => Axis::X,
        Direction::Down | Direction::Up => Axis::Y,
        Direction::North | Direction::South => Axis::Z,
    }
}

const fn same_axis(left: Direction, right: Direction) -> bool {
    matches!(
        (left, right),
        (Direction::West | Direction::East, Direction::West | Direction::East)
            | (Direction::Down | Direction::Up, Direction::Down | Direction::Up)
            | (
                Direction::North | Direction::South,
                Direction::North | Direction::South
            )
    )
}

const fn axis_direction(direction: Direction) -> u8 {
    match direction {
        Direction::Down | Direction::North | Direction::West => 0,
        Direction::Up | Direction::South | Direction::East => 1,
    }
}

const fn java_ordinal(direction: Direction) -> u8 {
    match direction {
        Direction::Down => 0,
        Direction::Up => 1,
        Direction::North => 2,
        Direction::South => 3,
        Direction::West => 4,
        Direction::East => 5,
    }
}

const fn vector(direction: Direction) -> [i32; 3] {
    let (x, y, z) = direction.step();
    [x, y, z]
}

const fn cross(left: [i32; 3], right: [i32; 3]) -> [i32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

const fn direction_from_vector(vector: [i32; 3]) -> Direction {
    match vector {
        [-1, 0, 0] => Direction::West,
        [1, 0, 0] => Direction::East,
        [0, -1, 0] => Direction::Down,
        [0, 1, 0] => Direction::Up,
        [0, 0, -1] => Direction::North,
        [0, 0, 1] => Direction::South,
        _ => panic!("invalid direction vector"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_orientation_indices_round_trip() {
        for index in 0..48 {
            assert_eq!(Orientation::from_index(index).index(), index);
        }
    }

    #[test]
    fn vanilla_seed_orientation_has_expected_order() {
        let orientation = Orientation::from_index(20).with_side_bias(SideBias::Left);
        assert_eq!(orientation.directions().len(), 6);
        assert_ne!(orientation.directions()[0], orientation.directions()[1]);
    }
}
