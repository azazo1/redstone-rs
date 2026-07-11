use std::f32::consts::{PI, TAU};

const DAY_TICKS: u64 = 24_000;
const SUN_ANGLE_OFFSET: u64 = 6_000;
const SKY_ANGLE_X1: f32 = 0.362;
const SKY_ANGLE_Y1: f32 = 0.241;
const NIGHT_SKY_LIGHT_FACTOR: f32 = 0.266_666_68;

pub(crate) fn daylight_detector_power(
    overworld_time: u64,
    raw_sky_light: u8,
    inverted: bool,
) -> u8 {
    let sky_darken = (15.0 - sky_light_level(overworld_time)) as i32;
    let effective_sky = i32::from(raw_sky_light) - sky_darken;
    let target = if inverted {
        15 - effective_sky
    } else if effective_sky > 0 {
        let mut sun_angle = sun_angle_degrees(overworld_time).to_radians();
        let offset = if sun_angle < PI { 0.0 } else { TAU };
        sun_angle += (offset - sun_angle) * 0.2;
        ((effective_sky as f32) * sun_angle.cos()).round() as i32
    } else {
        effective_sky
    };
    target.clamp(0, 15) as u8
}

pub(crate) fn sun_angle_degrees(overworld_time: u64) -> f32 {
    let ticks = (overworld_time % DAY_TICKS + DAY_TICKS - SUN_ANGLE_OFFSET) % DAY_TICKS;
    symmetric_cubic_bezier(ticks as f32 / DAY_TICKS as f32) * 360.0
}

pub(crate) fn sky_light_level(overworld_time: u64) -> f32 {
    let factor = sample_periodic_linear(
        overworld_time % DAY_TICKS,
        &[
            (133, 1.0),
            (11_867, 1.0),
            (13_670, NIGHT_SKY_LIGHT_FACTOR),
            (22_330, NIGHT_SKY_LIGHT_FACTOR),
        ],
    );
    15.0 * factor
}

fn symmetric_cubic_bezier(x: f32) -> f32 {
    let x2 = 1.0 - SKY_ANGLE_X1;
    let y2 = 1.0 - SKY_ANGLE_Y1;
    let x_curve = CubicCurve::from_controls(SKY_ANGLE_X1, x2);
    let y_curve = CubicCurve::from_controls(SKY_ANGLE_Y1, y2);
    let mut parameter = x;
    for _ in 0..4 {
        let gradient = x_curve.gradient(parameter);
        if gradient < 1.0e-5 {
            break;
        }
        parameter -= (x_curve.sample(parameter) - x) / gradient;
    }
    y_curve.sample(parameter)
}

fn sample_periodic_linear(ticks: u64, keyframes: &[(i32, f32)]) -> f32 {
    let ticks = ticks as i32;
    let last = keyframes[keyframes.len() - 1];
    let mut from = (last.0 - DAY_TICKS as i32, last.1);
    for to in keyframes
        .iter()
        .copied()
        .chain(std::iter::once((keyframes[0].0 + DAY_TICKS as i32, keyframes[0].1)))
    {
        if ticks < to.0 {
            let alpha = (ticks - from.0) as f32 / (to.0 - from.0) as f32;
            return from.1 + (to.1 - from.1) * alpha;
        }
        from = to;
    }
    unreachable!("periodic keyframes must cover the complete day")
}

#[derive(Clone, Copy)]
struct CubicCurve {
    a: f32,
    b: f32,
    c: f32,
}

impl CubicCurve {
    fn from_controls(first: f32, second: f32) -> Self {
        Self {
            a: 3.0 * first - 3.0 * second + 1.0,
            b: -6.0 * first + 3.0 * second,
            c: 3.0 * first,
        }
    }

    fn sample(self, value: f32) -> f32 {
        ((self.a * value + self.b) * value + self.c) * value
    }

    fn gradient(self, value: f32) -> f32 {
        (3.0 * self.a * value + 2.0 * self.b) * value + self.c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overworld_timeline_matches_vanilla_day_samples() {
        assert!((sun_angle_degrees(6_000) - 0.0).abs() < 0.001);
        assert!((sun_angle_degrees(0) - 282.37).abs() < 0.02);
        assert!((sky_light_level(200) - 15.0).abs() < 0.001);
        assert!((sky_light_level(18_000) - 4.0).abs() < 0.001);
    }

    #[test]
    fn daylight_detector_matches_clear_overworld_samples() {
        assert_eq!(daylight_detector_power(0, 15, false), 7);
        assert_eq!(daylight_detector_power(6_000, 15, false), 15);
        assert_eq!(daylight_detector_power(18_000, 15, false), 0);
        assert_eq!(daylight_detector_power(18_000, 15, true), 11);
    }
}
