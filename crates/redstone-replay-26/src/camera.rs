use redstone_core::BlockPos;

use crate::protocol::chunk::{MAX_Y, MIN_Y};
use crate::{ReplayError, ReplayRegion};

pub const DEFAULT_VIEW_DISTANCE: i32 = 8;

const MIN_VIEW_DISTANCE: i32 = 2;
const MAX_VIEW_DISTANCE: i32 = 32;
const MIN_CAMERA_DISTANCE: f64 = 8.0;
const MAX_CAMERA_DISTANCE: f64 = 32.0;
const FLATNESS_RATIO: f64 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReplayCameraOptions {
    pub view_distance: i32,
    pub position: Option<[f64; 3]>,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
}

impl Default for ReplayCameraOptions {
    fn default() -> Self {
        Self {
            view_distance: DEFAULT_VIEW_DISTANCE,
            position: None,
            yaw: None,
            pitch: None,
        }
    }
}

impl ReplayCameraOptions {
    pub(crate) fn validate(self) -> Result<(), ReplayError> {
        if !(MIN_VIEW_DISTANCE..=MAX_VIEW_DISTANCE).contains(&self.view_distance) {
            return Err(ReplayError::CameraViewDistance {
                value: self.view_distance,
                min: MIN_VIEW_DISTANCE,
                max: MAX_VIEW_DISTANCE,
            });
        }
        if let Some(position) = self.position
            && position.iter().any(|value| !value.is_finite())
        {
            return Err(ReplayError::CameraPositionNotFinite { position });
        }
        match (self.yaw, self.pitch) {
            (Some(yaw), Some(pitch)) => {
                if !yaw.is_finite() || !pitch.is_finite() {
                    return Err(ReplayError::CameraAnglesNotFinite { yaw, pitch });
                }
                if !(-90.0..=90.0).contains(&pitch) {
                    return Err(ReplayError::CameraPitchOutOfRange { pitch });
                }
            }
            (None, None) => {}
            _ => return Err(ReplayError::IncompleteCameraAngles),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReplayCameraHint {
    pub position: [f64; 3],
    pub weight: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Camera {
    pub(crate) position: [f64; 3],
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) target: BlockPos,
}

pub(crate) fn camera_for_region(
    region: ReplayRegion,
    options: ReplayCameraOptions,
    hints: &[ReplayCameraHint],
) -> Result<Camera, ReplayError> {
    options.validate()?;
    let target = region_center(region);
    let position = options
        .position
        .unwrap_or_else(|| automatic_position(region, target, options.view_distance, hints));
    let mut camera = camera_looking_at(position, target);
    if let (Some(yaw), Some(pitch)) = (options.yaw, options.pitch) {
        camera.yaw = yaw;
        camera.pitch = pitch;
    }
    Ok(camera)
}

fn automatic_position(
    region: ReplayRegion,
    target: [f64; 3],
    view_distance: i32,
    hints: &[ReplayCameraHint],
) -> [f64; 3] {
    let dimensions = region_dimensions(region);
    let direction = camera_direction(dimensions, target, hints);
    let longest = dimensions[0].max(dimensions[1]).max(dimensions[2]);
    let preferred_distance = (longest * 0.75 + 6.0)
        .clamp(MIN_CAMERA_DISTANCE, MAX_CAMERA_DISTANCE);
    let horizontal = direction[0].abs().max(direction[2].abs());
    let horizontal_budget = f64::from(view_distance.saturating_sub(1)) * 16.0;
    let distance = if horizontal > f64::EPSILON {
        preferred_distance.min(horizontal_budget / horizontal)
    } else {
        preferred_distance
    };
    [
        target[0] + direction[0] * distance,
        target[1] + direction[1] * distance,
        target[2] + direction[2] * distance,
    ]
}

fn camera_direction(
    dimensions: [f64; 3],
    target: [f64; 3],
    hints: &[ReplayCameraHint],
) -> [f64; 3] {
    let mut ordered = [
        (dimensions[0], 0usize),
        (dimensions[1], 1usize),
        (dimensions[2], 2usize),
    ];
    ordered.sort_by(|left, right| left.0.total_cmp(&right.0));
    let candidates = if ordered[0].0 * FLATNESS_RATIO <= ordered[1].0 {
        flat_candidates(ordered[0].1)
    } else {
        vec![
            [0.62, 0.72, 0.62],
            [0.62, 0.72, -0.62],
            [-0.62, 0.72, 0.62],
            [-0.62, 0.72, -0.62],
        ]
    };
    let mut candidates = candidates.into_iter().map(normalize);
    let mut best = candidates.next().expect("camera candidates must not be empty");
    let mut best_score = hint_score(best, target, hints);
    for candidate in candidates {
        let score = hint_score(candidate, target, hints);
        if score > best_score {
            best = candidate;
            best_score = score;
        }
    }
    best
}

fn flat_candidates(axis: usize) -> Vec<[f64; 3]> {
    match axis {
        0 => vec![[1.0, 0.18, 0.0], [-1.0, 0.18, 0.0]],
        1 => vec![[0.18, 1.0, 0.18], [0.18, -1.0, 0.18]],
        2 => vec![[0.0, 0.18, 1.0], [0.0, 0.18, -1.0]],
        _ => unreachable!(),
    }
}

fn hint_score(direction: [f64; 3], target: [f64; 3], hints: &[ReplayCameraHint]) -> f64 {
    hints
        .iter()
        .filter(|hint| hint.weight.is_finite() && hint.weight > 0.0)
        .map(|hint| {
            let offset = [
                hint.position[0] - target[0],
                hint.position[1] - target[1],
                hint.position[2] - target[2],
            ];
            (offset[0] * direction[0]
                + offset[1] * direction[1]
                + offset[2] * direction[2])
                * hint.weight
        })
        .sum()
}

fn normalize(direction: [f64; 3]) -> [f64; 3] {
    let length = direction[0]
        .hypot(direction[1])
        .hypot(direction[2]);
    [
        direction[0] / length,
        direction[1] / length,
        direction[2] / length,
    ]
}

fn region_center(region: ReplayRegion) -> [f64; 3] {
    [
        (f64::from(region.min.x) + f64::from(region.max.x) + 1.0) / 2.0,
        (f64::from(region.min.y) + f64::from(region.max.y) + 1.0) / 2.0,
        (f64::from(region.min.z) + f64::from(region.max.z) + 1.0) / 2.0,
    ]
}

fn region_dimensions(region: ReplayRegion) -> [f64; 3] {
    [
        f64::from(region.max.x) - f64::from(region.min.x) + 1.0,
        f64::from(region.max.y) - f64::from(region.min.y) + 1.0,
        f64::from(region.max.z) - f64::from(region.min.z) + 1.0,
    ]
}

fn camera_looking_at(position: [f64; 3], target: [f64; 3]) -> Camera {
    let dx = target[0] - position[0];
    let dy = target[1] - position[1];
    let dz = target[2] - position[2];
    let horizontal = dx.hypot(dz);
    Camera {
        position,
        yaw: (-dx).atan2(dz).to_degrees() as f32,
        pitch: (-dy.atan2(horizontal)).to_degrees() as f32,
        target: BlockPos::new(
            floor_to_i32(target[0]),
            floor_to_i32(target[1]).clamp(MIN_Y, MAX_Y),
            floor_to_i32(target[2]),
        ),
    }
}

pub(crate) fn floor_to_i32(value: f64) -> i32 {
    value.floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_plane_is_observed_from_above() {
        let camera = automatic_camera(BlockPos::ZERO, BlockPos::new(31, 0, 31), &[]);
        let target = region_center(ReplayRegion::new(
            BlockPos::ZERO,
            BlockPos::new(31, 0, 31),
        ));

        assert!(camera.position[1] > target[1]);
        assert!(camera.pitch > 70.0);
    }

    #[test]
    fn vertical_plane_is_observed_along_its_thin_axis() {
        let x_plane = automatic_camera(BlockPos::ZERO, BlockPos::new(0, 31, 31), &[]);
        let x_target = region_center(ReplayRegion::new(
            BlockPos::ZERO,
            BlockPos::new(0, 31, 31),
        ));
        assert!((x_plane.position[0] - x_target[0]).abs() > 20.0);
        assert!((x_plane.position[2] - x_target[2]).abs() < 0.001);

        let z_plane = automatic_camera(BlockPos::ZERO, BlockPos::new(31, 31, 0), &[]);
        let z_target = region_center(ReplayRegion::new(
            BlockPos::ZERO,
            BlockPos::new(31, 31, 0),
        ));
        assert!((z_plane.position[2] - z_target[2]).abs() > 20.0);
        assert!((z_plane.position[0] - z_target[0]).abs() < 0.001);
    }

    #[test]
    fn line_shape_uses_the_general_oblique_view() {
        let region = ReplayRegion::new(BlockPos::ZERO, BlockPos::new(0, 0, 63));
        let camera = camera_for_region(region, ReplayCameraOptions::default(), &[]).unwrap();
        let target = region_center(region);

        assert_ne!(camera.position[0], target[0]);
        assert!(camera.position[1] > target[1]);
        assert_ne!(camera.position[2], target[2]);
    }

    #[test]
    fn hints_select_the_observation_side() {
        let region = ReplayRegion::new(BlockPos::new(0, 0, 0), BlockPos::new(0, 31, 31));
        let positive = camera_for_region(
            region,
            ReplayCameraOptions::default(),
            &[ReplayCameraHint {
                position: [0.9, 16.0, 16.0],
                weight: 4.0,
            }],
        )
        .unwrap();
        let negative = camera_for_region(
            region,
            ReplayCameraOptions::default(),
            &[ReplayCameraHint {
                position: [0.1, 16.0, 16.0],
                weight: 4.0,
            }],
        )
        .unwrap();

        assert!(positive.position[0] > negative.position[0]);
    }

    #[test]
    fn automatic_horizontal_offset_respects_view_distance_budget() {
        let region = ReplayRegion::new(BlockPos::ZERO, BlockPos::new(0, 127, 127));
        let camera = camera_for_region(
            region,
            ReplayCameraOptions {
                view_distance: 2,
                ..ReplayCameraOptions::default()
            },
            &[],
        )
        .unwrap();
        let target = region_center(region);
        let camera_chunk = floor_to_i32(camera.position[0]).div_euclid(16);
        let target_chunk = floor_to_i32(target[0]).div_euclid(16);

        assert!((camera_chunk - target_chunk).abs() <= 2);
    }

    #[test]
    fn manual_pose_is_preserved() {
        let camera = camera_for_region(
            ReplayRegion::new(BlockPos::ZERO, BlockPos::new(10, 10, 10)),
            ReplayCameraOptions {
                position: Some([1.25, 2.5, 3.75]),
                yaw: Some(-45.0),
                pitch: Some(30.0),
                ..ReplayCameraOptions::default()
            },
            &[],
        )
        .unwrap();

        assert_eq!(camera.position, [1.25, 2.5, 3.75]);
        assert_eq!(camera.yaw, -45.0);
        assert_eq!(camera.pitch, 30.0);
    }

    #[test]
    fn yaw_and_pitch_must_be_set_together() {
        let error = camera_for_region(
            ReplayRegion::new(BlockPos::ZERO, BlockPos::ZERO),
            ReplayCameraOptions {
                yaw: Some(45.0),
                ..ReplayCameraOptions::default()
            },
            &[],
        )
        .unwrap_err();

        assert!(matches!(error, ReplayError::IncompleteCameraAngles));
    }

    fn automatic_camera(min: BlockPos, max: BlockPos, hints: &[ReplayCameraHint]) -> Camera {
        camera_for_region(
            ReplayRegion::new(min, max),
            ReplayCameraOptions::default(),
            hints,
        )
        .unwrap()
    }
}
