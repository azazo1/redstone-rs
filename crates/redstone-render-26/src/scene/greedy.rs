use super::mesh::{Color, Vertex};
use super::section::{SECTION_VOLUME, SectionPos, index};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ColorKey([u32; 4]);

impl ColorKey {
    pub(crate) fn new(color: Color) -> Self {
        Self(color.map(f32::to_bits))
    }

    fn color(self) -> Color {
        self.0.map(f32::from_bits)
    }
}

pub(crate) fn append_full_cubes(
    output: &mut Vec<Vertex>,
    section: SectionPos,
    colors: &[Option<ColorKey>; SECTION_VOLUME],
    occluding: &[bool; SECTION_VOLUME],
    outside_occluding: &[[bool; 256]; 6],
) {
    for (face, outside) in outside_occluding.iter().enumerate() {
        for slice in 0..16 {
            let mut mask = [None; 256];
            for v in 0..16 {
                for u in 0..16 {
                    let block_index = face_index(face, slice, u, v);
                    let Some(color) = colors[block_index] else {
                        continue;
                    };
                    let hidden = neighbor_index(face, slice, u, v)
                        .map_or(outside[u + v * 16], |neighbor| {
                            occluding[neighbor]
                        });
                    if !hidden {
                        mask[u + v * 16] = Some(color);
                    }
                }
            }
            merge_mask(output, section, face, slice, &mut mask);
        }
    }
}

fn merge_mask(
    output: &mut Vec<Vertex>,
    section: SectionPos,
    face: usize,
    slice: usize,
    mask: &mut [Option<ColorKey>; 256],
) {
    for v in 0..16 {
        let mut u = 0;
        while u < 16 {
            let Some(color) = mask[u + v * 16] else {
                u += 1;
                continue;
            };
            let mut width = 1;
            while u + width < 16 && mask[u + width + v * 16] == Some(color) {
                width += 1;
            }
            let mut height = 1;
            'height: while v + height < 16 {
                for x in u..u + width {
                    if mask[x + (v + height) * 16] != Some(color) {
                        break 'height;
                    }
                }
                height += 1;
            }
            for y in v..v + height {
                for x in u..u + width {
                    mask[x + y * 16] = None;
                }
            }
            append_face(output, section, face, FaceRect {
                slice: slice as f32,
                u: u as f32,
                v: v as f32,
                width: width as f32,
                height: height as f32,
                color: color.color(),
            });
            u += width;
        }
    }
}

struct FaceRect {
    slice: f32,
    u: f32,
    v: f32,
    width: f32,
    height: f32,
    color: Color,
}

fn append_face(
    output: &mut Vec<Vertex>,
    section: SectionPos,
    face: usize,
    rect: FaceRect,
) {
    let FaceRect {
        slice,
        u,
        v,
        width,
        height,
        color,
    } = rect;
    let origin = section.origin();
    let base = [origin.x as f32, origin.y as f32, origin.z as f32];
    let u1 = u + width;
    let v1 = v + height;
    let (normal, corners) = match face {
        0 => ([-1.0, 0.0, 0.0], [[slice, v, u], [slice, v, u1], [slice, v1, u1], [slice, v1, u]]),
        1 => ([1.0, 0.0, 0.0], [[slice + 1.0, v, u1], [slice + 1.0, v, u], [slice + 1.0, v1, u], [slice + 1.0, v1, u1]]),
        2 => ([0.0, -1.0, 0.0], [[u, slice, v1], [u, slice, v], [u1, slice, v], [u1, slice, v1]]),
        3 => ([0.0, 1.0, 0.0], [[u, slice + 1.0, v], [u, slice + 1.0, v1], [u1, slice + 1.0, v1], [u1, slice + 1.0, v]]),
        4 => ([0.0, 0.0, -1.0], [[u1, v, slice], [u, v, slice], [u, v1, slice], [u1, v1, slice]]),
        5 => ([0.0, 0.0, 1.0], [[u, v, slice + 1.0], [u1, v, slice + 1.0], [u1, v1, slice + 1.0], [u, v1, slice + 1.0]]),
        _ => unreachable!(),
    };
    for vertex in [0, 1, 2, 0, 2, 3] {
        let point = corners[vertex];
        output.push(Vertex::new(
            [base[0] + point[0], base[1] + point[1], base[2] + point[2]],
            normal,
            color,
        ));
    }
}

fn face_index(face: usize, slice: usize, u: usize, v: usize) -> usize {
    match face {
        0 | 1 => index(slice, v, u),
        2 | 3 => index(u, slice, v),
        4 | 5 => index(u, v, slice),
        _ => unreachable!(),
    }
}

fn neighbor_index(face: usize, slice: usize, u: usize, v: usize) -> Option<usize> {
    match face {
        0 => slice.checked_sub(1).map(|x| index(x, v, u)),
        1 => (slice < 15).then(|| index(slice + 1, v, u)),
        2 => slice.checked_sub(1).map(|y| index(u, y, v)),
        3 => (slice < 15).then(|| index(u, slice + 1, v)),
        4 => slice.checked_sub(1).map(|z| index(u, v, z)),
        5 => (slice < 15).then(|| index(u, v, slice + 1)),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_section_merges_to_six_quads() {
        let colors = [Some(ColorKey::new([0.5, 0.5, 0.5, 0.0])); SECTION_VOLUME];
        let occluding = [true; SECTION_VOLUME];
        let mut vertices = Vec::new();
        append_full_cubes(
            &mut vertices,
            SectionPos { x: 0, y: 0, z: 0 },
            &colors,
            &occluding,
            &[[false; 256]; 6],
        );
        assert_eq!(vertices.len(), 36);
    }
}
