use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Quat, Vec3};
use redstone_core::{BlockPos, Direction};

pub(crate) type Color = [f32; 4];

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct Vertex {
    pub position: [f32; 3],
    pub normal: [i8; 4],
    pub color: [u8; 4],
}

impl Vertex {
    pub(crate) const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Snorm8x4, 2 => Unorm8x4];

    pub(crate) fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }

    pub(crate) fn new(position: [f32; 3], normal: [f32; 3], color: Color) -> Self {
        Self {
            position,
            normal: [
                pack_snorm(normal[0]),
                pack_snorm(normal[1]),
                pack_snorm(normal[2]),
                0,
            ],
            color: color.map(pack_unorm),
        }
    }
}

fn pack_snorm(value: f32) -> i8 {
    (value.clamp(-1.0, 1.0) * 127.0).round() as i8
}

fn pack_unorm(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[derive(Clone, Copy)]
pub(crate) struct Transform {
    rotation: Mat3,
    offset: Vec3,
}

impl Transform {
    pub(crate) fn identity() -> Self {
        Self {
            rotation: Mat3::IDENTITY,
            offset: Vec3::ZERO,
        }
    }

    pub(crate) fn facing(direction: Direction) -> Self {
        let target = direction_vector(direction);
        let rotation = Mat3::from_quat(Quat::from_rotation_arc(Vec3::NEG_Z, target));
        Self {
            rotation,
            offset: centered_offset(rotation),
        }
    }

    pub(crate) fn mounted_facing(normal: Direction, facing: Direction) -> Self {
        let up = direction_vector(normal);
        let forward = if up.y.abs() > 0.5 {
            direction_vector(facing)
        } else {
            Vec3::Y
        };
        let local_z = -forward;
        let local_x = up.cross(local_z).normalize_or_zero();
        let rotation = Mat3::from_cols(local_x, up, local_z);
        Self {
            rotation,
            offset: centered_offset(rotation),
        }
    }

    pub(crate) fn translated(mut self, translation: [f32; 3]) -> Self {
        self.offset += Vec3::from_array(translation);
        self
    }

    fn compose(self, inner: Self) -> Self {
        Self {
            rotation: self.rotation * inner.rotation,
            offset: self.rotation * inner.offset + self.offset,
        }
    }

    fn point(self, point: [f32; 3]) -> Vec3 {
        self.rotation * Vec3::from_array(point) + self.offset
    }

    fn normal(self, normal: [f32; 3]) -> Vec3 {
        (self.rotation * Vec3::from_array(normal)).normalize_or_zero()
    }
}

pub(crate) fn cuboid(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    min: [f32; 3],
    max: [f32; 3],
    color: Color,
    visible: Option<[bool; 6]>,
    transform: Transform,
) {
    let point = |x: f32, y: f32, z: f32| [x, y, z];
    let faces = [
        ([-1.0, 0.0, 0.0], [point(min[0], min[1], min[2]), point(min[0], min[1], max[2]), point(min[0], max[1], max[2]), point(min[0], max[1], min[2])]),
        ([1.0, 0.0, 0.0], [point(max[0], min[1], max[2]), point(max[0], min[1], min[2]), point(max[0], max[1], min[2]), point(max[0], max[1], max[2])]),
        ([0.0, -1.0, 0.0], [point(min[0], min[1], max[2]), point(min[0], min[1], min[2]), point(max[0], min[1], min[2]), point(max[0], min[1], max[2])]),
        ([0.0, 1.0, 0.0], [point(min[0], max[1], min[2]), point(min[0], max[1], max[2]), point(max[0], max[1], max[2]), point(max[0], max[1], min[2])]),
        ([0.0, 0.0, -1.0], [point(max[0], min[1], min[2]), point(min[0], min[1], min[2]), point(min[0], max[1], min[2]), point(max[0], max[1], min[2])]),
        ([0.0, 0.0, 1.0], [point(min[0], min[1], max[2]), point(max[0], min[1], max[2]), point(max[0], max[1], max[2]), point(min[0], max[1], max[2])]),
    ];
    for (index, (normal, corners)) in faces.into_iter().enumerate() {
        if visible.is_some_and(|visible| !visible[index]) {
            continue;
        }
        quad(output, pos, corners, normal, color, transform, false);
    }
}

pub(crate) fn quad(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    corners: [[f32; 3]; 4],
    normal: [f32; 3],
    color: Color,
    transform: Transform,
    double_sided: bool,
) {
    let base = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32);
    let normal = transform.normal(normal);
    for index in [0, 1, 2, 0, 2, 3] {
        output.push(Vertex::new(
            (base + transform.point(corners[index])).to_array(),
            normal.to_array(),
            color,
        ));
    }
    if double_sided {
        for index in [3, 2, 0, 2, 1, 0] {
            output.push(Vertex::new(
                (base + transform.point(corners[index])).to_array(),
                (-normal).to_array(),
                color,
            ));
        }
    }
}

pub(crate) fn rod(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    start: [f32; 3],
    end: [f32; 3],
    width: f32,
    color: Color,
) {
    rod_transformed(
        output,
        pos,
        start,
        end,
        width,
        color,
        Transform::identity(),
    );
}

pub(crate) fn rod_transformed(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    start: [f32; 3],
    end: [f32; 3],
    width: f32,
    color: Color,
    outer: Transform,
) {
    let start = Vec3::from_array(start);
    let end = Vec3::from_array(end);
    let delta = end - start;
    let length = delta.length();
    if length <= f32::EPSILON {
        return;
    }
    let midpoint = (start + end) * 0.5;
    let rotation = Mat3::from_quat(Quat::from_rotation_arc(Vec3::Y, delta / length));
    let inner = Transform {
        rotation,
        offset: midpoint - rotation * Vec3::splat(0.5),
    };
    cuboid(
        output,
        pos,
        [0.5 - width * 0.5, 0.5 - length * 0.5, 0.5 - width * 0.5],
        [0.5 + width * 0.5, 0.5 + length * 0.5, 0.5 + width * 0.5],
        color,
        None,
        outer.compose(inner),
    );
}

pub(crate) fn ribbon(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    start: [f32; 3],
    end: [f32; 3],
    width: f32,
    color: Color,
) {
    let start = Vec3::from_array(start);
    let end = Vec3::from_array(end);
    let delta = end - start;
    if delta.length_squared() <= f32::EPSILON {
        return;
    }
    let mut side = delta.cross(Vec3::Y).normalize_or_zero();
    if side.length_squared() <= f32::EPSILON {
        side = Vec3::X;
    }
    side *= width * 0.5;
    let corners = [start - side, end - side, end + side, start + side]
        .map(|point| point.to_array());
    let normal = (end - start).cross(side).normalize_or_zero().to_array();
    quad(
        output,
        pos,
        corners,
        normal,
        color,
        Transform::identity(),
        true,
    );
}

fn direction_vector(direction: Direction) -> Vec3 {
    let (x, y, z) = direction.step();
    Vec3::new(x as f32, y as f32, z as f32)
}

fn centered_offset(rotation: Mat3) -> Vec3 {
    let pivot = Vec3::splat(0.5);
    pivot - rotation * pivot
}
