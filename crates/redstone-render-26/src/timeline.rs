use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use redstone_replay_26::{ReplayCameraInterpolation, ReplayRenderOptions};
use serde::Deserialize;
use zip::ZipArchive;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    pub position: [f64; 3],
    pub rotation: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
enum Interpolation {
    Linear,
    Cubic,
    CatmullRom { alpha: f64 },
}

#[derive(Clone, Debug)]
pub struct FovTimeline {
    fallback: f32,
    times: Vec<i64>,
    values: Vec<f64>,
    interpolation: Interpolation,
}

impl FovTimeline {
    pub fn new(options: ReplayRenderOptions, duration_ms: i64) -> Result<Self> {
        let keyframes = options
            .fov_keyframes
            .iter()
            .map(|keyframe| (keyframe.time_ms, keyframe.fov_degrees))
            .collect::<Vec<_>>();
        if !keyframes.is_empty()
            && (keyframes.len() < 2
                || keyframes[0].0 != 0
                || keyframes.last().unwrap().0 != duration_ms)
        {
            bail!("FOV path 必须至少有两帧并完整覆盖视频时长");
        }
        for pair in keyframes.windows(2) {
            if pair[1].0 <= pair[0].0 {
                bail!("FOV path 编辑时间必须严格递增");
            }
        }
        Ok(Self {
            fallback: options.fov_degrees.unwrap_or(70.0),
            times: keyframes.iter().map(|(time_ms, _)| *time_ms).collect(),
            values: keyframes
                .iter()
                .map(|(_, value)| f64::from(*value))
                .collect(),
            interpolation: options.fov_interpolation.into(),
        })
    }

    pub fn sample(&self, time_ms: f64) -> f32 {
        if self.times.is_empty() {
            return self.fallback;
        }
        let last = self.times.len() - 1;
        if time_ms <= self.times[0] as f64 {
            return self.values[0] as f32;
        }
        if time_ms >= self.times[last] as f64 {
            return self.values[last] as f32;
        }
        let segment = self
            .times
            .windows(2)
            .position(|pair| time_ms < pair[1] as f64)
            .unwrap_or(last - 1);
        let start = self.times[segment] as f64;
        let end = self.times[segment + 1] as f64;
        let fraction = (time_ms - start) / (end - start);
        (sample_component(
            &self.values,
            &self.times,
            segment,
            fraction,
            self.interpolation,
        ) as f32)
            .clamp(10.0, 140.0)
    }
}

impl From<ReplayCameraInterpolation> for Interpolation {
    fn from(value: ReplayCameraInterpolation) -> Self {
        match value {
            ReplayCameraInterpolation::Linear => Self::Linear,
            ReplayCameraInterpolation::Cubic => Self::Cubic,
            ReplayCameraInterpolation::CatmullRom => Self::CatmullRom { alpha: 0.5 },
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReplayTimeline {
    time_keyframes: Vec<(i64, i32)>,
    camera_keyframes: Vec<(i64, CameraPose)>,
    camera_segments: Vec<Interpolation>,
    unwrapped_rotation: Vec<[f64; 3]>,
}

impl ReplayTimeline {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path.as_ref())
            .with_context(|| format!("打开 MCPR 失败: {}", path.as_ref().display()))?;
        let mut archive = ZipArchive::new(file).context("MCPR ZIP 无效")?;
        let mut entry = archive
            .by_name("timelines.json")
            .context("MCPR 缺少 timelines.json")?;
        let mut json = String::new();
        entry.read_to_string(&mut json)?;
        let timelines = serde_json::from_str::<BTreeMap<String, Vec<RawPath>>>(&json)
            .context("timelines.json 格式无效")?;
        let paths = timelines
            .get("")
            .context("timelines.json 缺少默认时间轴")?;
        let time_path = paths
            .iter()
            .find(|path| {
                path.keyframes
                    .iter()
                    .any(|keyframe| keyframe.properties.timestamp.is_some())
            })
            .context("默认时间轴缺少 TIME path")?;
        let camera_path = paths
            .iter()
            .find(|path| {
                path.keyframes.iter().any(|keyframe| {
                    keyframe.properties.position.is_some()
                        && keyframe.properties.rotation.is_some()
                })
            })
            .context("默认时间轴缺少 POSITION path")?;

        let time_keyframes = time_path
            .keyframes
            .iter()
            .map(|keyframe| {
                Ok((
                    keyframe.time,
                    keyframe
                        .properties
                        .timestamp
                        .context("TIME keyframe 缺少 timestamp")?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        validate_time_keyframes(&time_keyframes)?;

        let camera_keyframes = camera_path
            .keyframes
            .iter()
            .map(|keyframe| {
                Ok((
                    keyframe.time,
                    CameraPose {
                        position: keyframe
                            .properties
                            .position
                            .context("POSITION keyframe 缺少 camera:position")?,
                        rotation: keyframe
                            .properties
                            .rotation
                            .context("POSITION keyframe 缺少 camera:rotation")?,
                    },
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        validate_camera_keyframes(&camera_keyframes, duration_of(&time_keyframes))?;
        let camera_segments = parse_segments(camera_path, camera_keyframes.len())?;
        let unwrapped_rotation = unwrap_rotations(&camera_keyframes);
        Ok(Self {
            time_keyframes,
            camera_keyframes,
            camera_segments,
            unwrapped_rotation,
        })
    }

    pub fn duration_ms(&self) -> i64 {
        duration_of(&self.time_keyframes)
    }

    pub fn source_start_ms(&self) -> i32 {
        self.time_keyframes[0].1
    }

    pub fn source_end_ms(&self) -> i32 {
        self.time_keyframes
            .last()
            .expect("time path is not empty")
            .1
    }

    pub fn replay_time(&self, time_ms: f64) -> f64 {
        sample_linear_pairs(&self.time_keyframes, time_ms)
    }

    pub fn camera(&self, time_ms: f64) -> CameraPose {
        let last = self.camera_keyframes.len() - 1;
        if time_ms <= self.camera_keyframes[0].0 as f64 {
            return self.camera_keyframes[0].1;
        }
        if time_ms >= self.camera_keyframes[last].0 as f64 {
            return self.camera_keyframes[last].1;
        }
        let segment = self
            .camera_keyframes
            .windows(2)
            .position(|pair| time_ms < pair[1].0 as f64)
            .unwrap_or(last - 1);
        let start = self.camera_keyframes[segment].0 as f64;
        let end = self.camera_keyframes[segment + 1].0 as f64;
        let fraction = (time_ms - start) / (end - start);
        let mut position = [0.0; 3];
        let mut rotation = [0.0; 3];
        let times = self
            .camera_keyframes
            .iter()
            .map(|(time_ms, _)| *time_ms)
            .collect::<Vec<_>>();
        for axis in 0..3 {
            let position_values = self
                .camera_keyframes
                .iter()
                .map(|(_, pose)| pose.position[axis])
                .collect::<Vec<_>>();
            position[axis] = sample_component(
                &position_values,
                &times,
                segment,
                fraction,
                self.camera_segments[segment],
            );
            let rotation_values = self
                .unwrapped_rotation
                .iter()
                .map(|value| value[axis])
                .collect::<Vec<_>>();
            rotation[axis] = sample_component(
                &rotation_values,
                &times,
                segment,
                fraction,
                self.camera_segments[segment],
            ) as f32;
        }
        CameraPose { position, rotation }
    }
}

fn duration_of(keyframes: &[(i64, i32)]) -> i64 {
    keyframes.last().expect("time path is not empty").0
}

fn validate_time_keyframes(keyframes: &[(i64, i32)]) -> Result<()> {
    if keyframes.is_empty() || keyframes[0].0 != 0 {
        bail!("TIME path 必须从 0 ms 开始");
    }
    for pair in keyframes.windows(2) {
        if pair[1].0 <= pair[0].0 {
            bail!("TIME path 编辑时间必须严格递增");
        }
        if pair[1].1 < pair[0].1 {
            bail!("TIME path 不支持 replay 时间倒放");
        }
    }
    Ok(())
}

fn validate_camera_keyframes(keyframes: &[(i64, CameraPose)], duration_ms: i64) -> Result<()> {
    if keyframes.len() < 2 || keyframes[0].0 != 0 || keyframes.last().unwrap().0 != duration_ms {
        bail!("POSITION path 必须至少有两帧并完整覆盖视频时长");
    }
    for pair in keyframes.windows(2) {
        if pair[1].0 <= pair[0].0 {
            bail!("POSITION path 编辑时间必须严格递增");
        }
    }
    for (_, pose) in keyframes {
        if pose.position.iter().any(|value| !value.is_finite())
            || pose.rotation.iter().any(|value| !value.is_finite())
        {
            bail!("POSITION path 包含非有限坐标或角度");
        }
    }
    Ok(())
}

fn parse_segments(path: &RawPath, keyframe_count: usize) -> Result<Vec<Interpolation>> {
    let segment_count = keyframe_count.saturating_sub(1);
    if path.segments.len() != segment_count {
        bail!("POSITION path 的 segments 数量无效");
    }
    path.segments
        .iter()
        .map(|index| {
            let index = index.context("POSITION path 存在空 interpolator")?;
            parse_interpolation(
                path.interpolators
                    .get(index)
                    .context("POSITION path interpolator 索引越界")?,
            )
        })
        .collect()
}

fn parse_interpolation(interpolator: &RawInterpolator) -> Result<Interpolation> {
    match &interpolator.kind {
        serde_json::Value::String(kind) if kind == "linear" => Ok(Interpolation::Linear),
        serde_json::Value::String(kind) if kind == "cubic-spline" => Ok(Interpolation::Cubic),
        serde_json::Value::Object(object)
            if object.get("type").and_then(|value| value.as_str())
                == Some("catmull-rom-spline") =>
        {
            let alpha = object
                .get("alpha")
                .and_then(|value| value.as_f64())
                .context("Catmull-Rom interpolator 缺少 alpha")?;
            Ok(Interpolation::CatmullRom { alpha })
        }
        _ => bail!("不支持的 POSITION interpolator: {}", interpolator.kind),
    }
}

fn sample_linear_pairs(keyframes: &[(i64, i32)], time_ms: f64) -> f64 {
    if time_ms <= 0.0 {
        return f64::from(keyframes[0].1);
    }
    let last = keyframes.len() - 1;
    if time_ms >= keyframes[last].0 as f64 {
        return f64::from(keyframes[last].1);
    }
    let segment = keyframes
        .windows(2)
        .position(|pair| time_ms < pair[1].0 as f64)
        .unwrap_or(last - 1);
    let start = keyframes[segment];
    let end = keyframes[segment + 1];
    let fraction = (time_ms - start.0 as f64) / (end.0 - start.0) as f64;
    f64::from(start.1) + f64::from(end.1 - start.1) * fraction
}

fn sample_component(
    values: &[f64],
    times: &[i64],
    segment: usize,
    fraction: f64,
    interpolation: Interpolation,
) -> f64 {
    match interpolation {
        Interpolation::Linear => values[segment] + (values[segment + 1] - values[segment]) * fraction,
        Interpolation::CatmullRom { alpha } => {
            let p0 = values[segment.saturating_sub(1)];
            let p1 = values[segment];
            let p2 = values[segment + 1];
            let p3 = values[(segment + 2).min(values.len() - 1)];
            let t0 = alpha * (p2 - p0);
            let t1 = alpha * (p3 - p1);
            let a = 2.0 * p1 - 2.0 * p2 + t0 + t1;
            let b = -3.0 * p1 + 3.0 * p2 - 2.0 * t0 - t1;
            ((a * fraction + b) * fraction + t0) * fraction + p1
        }
        Interpolation::Cubic => sample_natural_cubic(values, times, segment, fraction),
    }
}

fn sample_natural_cubic(
    values: &[f64],
    times: &[i64],
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
        let left = (times[index] - times[index - 1]) as f64;
        let right = (times[index + 1] - times[index]) as f64;
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
    let h = (times[segment + 1] - times[segment]) as f64;
    let a = 1.0 - fraction;
    let b = fraction;
    a * values[segment]
        + b * values[segment + 1]
        + ((a * a * a - a) * second[segment] + (b * b * b - b) * second[segment + 1])
            * h
            * h
            / 6.0
}

fn unwrap_rotations(keyframes: &[(i64, CameraPose)]) -> Vec<[f64; 3]> {
    let mut output: Vec<[f64; 3]> = Vec::with_capacity(keyframes.len());
    for (index, (_, pose)) in keyframes.iter().enumerate() {
        let mut current = pose.rotation.map(f64::from);
        if index > 0 {
            for axis in 0..3 {
                let previous = output[index - 1][axis];
                while current[axis] - previous > 180.0 {
                    current[axis] -= 360.0;
                }
                while current[axis] - previous < -180.0 {
                    current[axis] += 360.0;
                }
            }
        }
        output.push(current);
    }
    output
}

#[derive(Deserialize)]
struct RawPath {
    keyframes: Vec<RawKeyframe>,
    segments: Vec<Option<usize>>,
    interpolators: Vec<RawInterpolator>,
}

#[derive(Deserialize)]
struct RawKeyframe {
    time: i64,
    properties: RawProperties,
}

#[derive(Deserialize)]
struct RawProperties {
    #[serde(default)]
    timestamp: Option<i32>,
    #[serde(default, rename = "camera:position")]
    position: Option<[f64; 3]>,
    #[serde(default, rename = "camera:rotation")]
    rotation: Option<[f32; 3]>,
}

#[derive(Deserialize)]
struct RawInterpolator {
    #[serde(rename = "type")]
    kind: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use redstone_replay_26::ReplayFovKeyframe;

    use super::*;

    #[test]
    fn linear_time_path_can_pause() {
        let timeline = ReplayTimeline {
            time_keyframes: vec![(0, 100), (1000, 200), (2000, 200), (3000, 400)],
            camera_keyframes: vec![
                (0, CameraPose { position: [0.0; 3], rotation: [0.0; 3] }),
                (3000, CameraPose { position: [3.0, 0.0, 0.0], rotation: [0.0; 3] }),
            ],
            camera_segments: vec![Interpolation::Linear],
            unwrapped_rotation: vec![[0.0; 3], [0.0; 3]],
        };
        assert_eq!(timeline.replay_time(1500.0), 200.0);
        assert_eq!(timeline.replay_time(2500.0), 300.0);
    }

    #[test]
    fn camera_rotation_uses_shortest_angle_path() {
        let keyframes = vec![
            (0, CameraPose { position: [0.0; 3], rotation: [170.0, 0.0, 0.0] }),
            (1000, CameraPose { position: [0.0; 3], rotation: [-170.0, 0.0, 0.0] }),
        ];
        let timeline = ReplayTimeline {
            time_keyframes: vec![(0, 0), (1000, 1000)],
            camera_keyframes: keyframes.clone(),
            camera_segments: vec![Interpolation::Linear],
            unwrapped_rotation: unwrap_rotations(&keyframes),
        };
        assert!((timeline.camera(500.0).rotation[0] - 180.0).abs() < 0.001);
    }

    #[test]
    fn fov_timeline_interpolates_between_sparse_samples() {
        let timeline = FovTimeline::new(
            ReplayRenderOptions {
                fov_degrees: Some(70.0),
                fov_interpolation: ReplayCameraInterpolation::Linear,
                fov_keyframes: vec![
                    ReplayFovKeyframe {
                        time_ms: 0,
                        fov_degrees: 80.0,
                    },
                    ReplayFovKeyframe {
                        time_ms: 500,
                        fov_degrees: 70.0,
                    },
                    ReplayFovKeyframe {
                        time_ms: 1000,
                        fov_degrees: 60.0,
                    },
                ],
            },
            1000,
        )
        .unwrap();

        assert_eq!(timeline.sample(250.0), 75.0);
        assert_eq!(timeline.sample(750.0), 65.0);
    }
}
