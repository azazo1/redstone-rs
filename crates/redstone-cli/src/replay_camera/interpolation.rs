use redstone_replay_26::ReplayCameraInterpolation;

pub(super) fn sample_vector(
    samples: &[(i64, [f64; 3])],
    time_ms: i64,
    interpolation: ReplayCameraInterpolation,
) -> [f64; 3] {
    if time_ms <= samples[0].0 {
        return samples[0].1;
    }
    if time_ms >= samples.last().unwrap().0 {
        return samples.last().unwrap().1;
    }
    let segment = samples
        .windows(2)
        .position(|pair| time_ms < pair[1].0)
        .unwrap();
    let start = samples[segment].0;
    let end = samples[segment + 1].0;
    let fraction = (time_ms - start) as f64 / (end - start) as f64;
    std::array::from_fn(|axis| {
        let values = samples
            .iter()
            .map(|(_, value)| value[axis])
            .collect::<Vec<_>>();
        sample_component(&values, samples, segment, fraction, interpolation)
    })
}

fn sample_component(
    values: &[f64],
    samples: &[(i64, [f64; 3])],
    segment: usize,
    fraction: f64,
    interpolation: ReplayCameraInterpolation,
) -> f64 {
    match interpolation {
        ReplayCameraInterpolation::Linear => {
            values[segment] + (values[segment + 1] - values[segment]) * fraction
        }
        ReplayCameraInterpolation::CatmullRom => {
            let p0 = values[segment.saturating_sub(1)];
            let p1 = values[segment];
            let p2 = values[segment + 1];
            let p3 = values[(segment + 2).min(values.len() - 1)];
            let t0 = 0.5 * (p2 - p0);
            let t1 = 0.5 * (p3 - p1);
            let a = 2.0 * p1 - 2.0 * p2 + t0 + t1;
            let b = -3.0 * p1 + 3.0 * p2 - 2.0 * t0 - t1;
            ((a * fraction + b) * fraction + t0) * fraction + p1
        }
        ReplayCameraInterpolation::Cubic => {
            sample_natural_cubic(values, samples, segment, fraction)
        }
    }
}

fn sample_natural_cubic(
    values: &[f64],
    samples: &[(i64, [f64; 3])],
    segment: usize,
    fraction: f64,
) -> f64 {
    if values.len() == 2 {
        return values[0] + (values[1] - values[0]) * fraction;
    }
    let n = values.len();
    let mut second = vec![0.0; n];
    let mut rhs = vec![0.0; n];
    let mut diagonal = vec![1.0; n];
    let mut upper = vec![0.0; n];
    for index in 1..n - 1 {
        let left = (samples[index].0 - samples[index - 1].0) as f64;
        let right = (samples[index + 1].0 - samples[index].0) as f64;
        diagonal[index] = 2.0 * (left + right);
        upper[index] = right;
        rhs[index] = 6.0
            * ((values[index + 1] - values[index]) / right
                - (values[index] - values[index - 1]) / left);
        let factor = left / diagonal[index - 1];
        diagonal[index] -= factor * upper[index - 1];
        rhs[index] -= factor * rhs[index - 1];
    }
    for index in (1..n - 1).rev() {
        second[index] = (rhs[index] - upper[index] * second[index + 1]) / diagonal[index];
    }
    let h = (samples[segment + 1].0 - samples[segment].0) as f64;
    let a = 1.0 - fraction;
    let b = fraction;
    a * values[segment]
        + b * values[segment + 1]
        + ((a * a * a - a) * second[segment] + (b * b * b - b) * second[segment + 1])
            * h
            * h
            / 6.0
}

pub(super) fn unwrap_rotations(samples: &mut [(i64, [f64; 3])]) {
    for index in 1..samples.len() {
        for axis in 0..3 {
            let previous = samples[index - 1].1[axis];
            while samples[index].1[axis] - previous > 180.0 {
                samples[index].1[axis] -= 360.0;
            }
            while samples[index].1[axis] - previous < -180.0 {
                samples[index].1[axis] += 360.0;
            }
        }
    }
}
