use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use redstone_java_26::Java26Registry;
use redstone_replay_26::RenderTrace;

mod encoder;
mod gpu;
mod scene;
mod timeline;

pub use encoder::VideoEncoderKind;

use encoder::{EncoderSettings, VideoEncoder, select_encoder};
use gpu::GpuRenderer;
use scene::{Scene, SceneView};
use timeline::ReplayTimeline;
use timeline::CameraPose;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FrameFormat {
    Bgra,
    Nv12,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RenderQuality {
    #[default]
    High,
    Balanced,
    Fast,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShadowQuality {
    Off,
    Low,
    Medium,
    High,
}

impl ShadowQuality {
    fn size(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Low => 512,
            Self::Medium => 1024,
            Self::High => 2048,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ResolvedQuality {
    antialiasing: u32,
    shadow_size: u32,
}

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_mbps: u32,
    pub fov_degrees: f32,
    pub quality: RenderQuality,
    pub antialiasing: Option<u32>,
    pub shadows: Option<ShadowQuality>,
    pub encoder: VideoEncoderKind,
    pub ffmpeg: PathBuf,
    pub original_speed: bool,
    pub view_distance_chunks: i32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_mbps: 20,
            fov_degrees: 70.0,
            quality: RenderQuality::High,
            antialiasing: None,
            shadows: None,
            encoder: VideoEncoderKind::Auto,
            ffmpeg: PathBuf::from("ffmpeg"),
            original_speed: false,
            view_distance_chunks: 32,
        }
    }
}

impl RenderOptions {
    fn resolved_quality(&self) -> ResolvedQuality {
        let defaults = match self.quality {
            RenderQuality::High => ResolvedQuality {
                antialiasing: 4,
                shadow_size: 2048,
            },
            RenderQuality::Balanced => ResolvedQuality {
                antialiasing: 2,
                shadow_size: 1024,
            },
            RenderQuality::Fast => ResolvedQuality {
                antialiasing: 1,
                shadow_size: 0,
            },
        };
        ResolvedQuality {
            antialiasing: self.antialiasing.unwrap_or(defaults.antialiasing),
            shadow_size: self.shadows.map_or(defaults.shadow_size, ShadowQuality::size),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RenderStats {
    pub frames: u64,
    pub duration_ms: i64,
    pub elapsed: Duration,
    pub encoder: String,
    pub output_size: u64,
    pub average_output_fps: f64,
    pub mesh_elapsed: Duration,
    pub gpu_submit_elapsed: Duration,
    pub readback_wait_elapsed: Duration,
    pub cpu_copy_elapsed: Duration,
    pub encoder_backpressure_elapsed: Duration,
}

pub async fn render_replay(
    input: &Path,
    output: &Path,
    options: &RenderOptions,
    mut progress: impl FnMut(u64, u64),
) -> Result<RenderStats> {
    validate_options(output, options)?;
    let quality = options.resolved_quality();
    let started = Instant::now();
    let timeline = ReplayTimeline::load(input)?;
    let header = RenderTrace::read_mcpr_header(input)?;
    let render_duration_ms = if options.original_speed {
        if header.recorded_ticks == 0 || header.simulation_walltime.is_zero() {
            bail!("Replay 缺少有效模拟 walltime, 请用当前版本重新生成 replay");
        }
        let source_ticks = f64::from(timeline.source_end_ms() - timeline.source_start_ms()) / 50.0;
        walltime_clip_duration_ms(
            header.simulation_walltime,
            header.recorded_ticks,
            source_ticks,
        )
    } else {
        timeline.duration_ms() as f64
    };
    if !render_duration_ms.is_finite() || render_duration_ms <= 0.0 {
        bail!("Replay 渲染时长必须大于 0");
    }
    let total_frames = (render_duration_ms * f64::from(options.fps) / 1000.0)
        .floor()
        .max(1.0) as u64;
    progress(0, total_frames);
    let frame_plans = build_frame_plans(
        &timeline,
        total_frames,
        options.fps,
        options.original_speed,
        render_duration_ms,
    );
    let required_chunks = required_chunks(&frame_plans, options.view_distance_chunks);
    tracing::info!(
        path = %input.display(),
        required_chunks = required_chunks.len(),
        "读取 replay 可见渲染轨迹"
    );
    let mut registry = Java26Registry::new();
    let mut scene = Scene::new(registry.air_state());
    let mut initial_blocks = 0u64;
    let trace = RenderTrace::read_mcpr_filtered_into(
        input,
        |pos| required_chunks.contains(&(pos.x.div_euclid(16), pos.z.div_euclid(16))),
        |block| {
            scene.insert_initial(block);
            initial_blocks += 1;
        },
    )?;
    scene.set_initial_pistons(&trace.initial_pistons);
    let encoder_name = select_encoder(&options.ffmpeg, options.encoder).await?;
    let initial_pose = frame_plans[0].pose;
    let mesh_started = Instant::now();
    let scene_view = |pose| SceneView {
        pose,
        view_distance: options.view_distance_chunks,
        fov_degrees: options.fov_degrees,
        aspect_ratio: options.width as f32 / options.height as f32,
        shadows: quality.shadow_size > 0,
    };
    let initial_meshes = scene.rebuild_visible(scene_view(initial_pose), &mut registry)?;
    let initial_vertices = initial_meshes
        .iter()
        .map(|mesh| mesh.vertices.len())
        .sum::<usize>();
    let initial_instances = initial_meshes
        .iter()
        .flat_map(|mesh| &mesh.models)
        .map(|model| model.positions.len())
        .sum::<usize>();
    let mut mesh_elapsed = mesh_started.elapsed();
    tracing::info!(
        blocks = initial_blocks,
        pistons = trace.initial_pistons.len(),
        vertices = initial_vertices,
        instances = initial_instances,
        chunks = initial_meshes.len(),
        updates = trace.frames.len(),
        "构建 replay 初始渲染网格"
    );
    let mut gpu = GpuRenderer::new(
        options.width,
        options.height,
        quality.antialiasing,
        quality.shadow_size,
        options.fov_degrees,
    )
    .await?;
    gpu.update_meshes(initial_meshes)?;
    let initial_dynamic = scene.dynamic_vertices(
        timeline.source_start_ms(),
        initial_pose.position,
        options.view_distance_chunks,
        &mut registry,
    )?;
    gpu.update_dynamic_mesh(&initial_dynamic)?;
    let mut video = VideoEncoder::start(
        &options.ffmpeg,
        encoder_name,
        output,
        EncoderSettings {
            width: options.width,
            height: options.height,
            fps: options.fps,
            bitrate_mbps: options.bitrate_mbps,
            frame_format: gpu.frame_format(),
        },
    )
    .await?;
    let mut trace_index = 0;
    let mut visual_groups = VecDeque::new();
    let mut last_visual_key = None;
    for (frame_index, plan) in frame_plans.into_iter().enumerate() {
        let replay_ms = plan.replay_ms;
        while let Some(frame) = trace.frames.get(trace_index) {
            if frame.timestamp_ms > replay_ms {
                break;
            }
            scene.apply(&frame.changes, &frame.pistons);
            trace_index += 1;
        }
        let pose = plan.pose;
        let mesh_started = Instant::now();
        let mesh_updates = scene.rebuild_visible(scene_view(pose), &mut registry)?;
        gpu.update_meshes(mesh_updates)?;
        let dynamic_vertices = scene.dynamic_vertices(
            replay_ms,
            pose.position,
            options.view_distance_chunks,
            &mut registry,
        )?;
        mesh_elapsed += mesh_started.elapsed();
        gpu.update_dynamic_mesh(&dynamic_vertices)?;
        let visual_key = VisualKey {
            position: pose.position.map(f64::to_bits),
            rotation: pose.rotation.map(f32::to_bits),
            scene: scene.visual_sample(replay_ms),
        };
        if last_visual_key == Some(visual_key) {
            *visual_groups
                .back_mut()
                .expect("重复帧之前必须已经提交首帧") += 1;
        } else {
            if let Some(frame) = gpu.render(pose)? {
                let copies = visual_groups
                    .pop_front()
                    .context("GPU 完成帧缺少重复计数")?;
                write_frame_copies(&mut video, Arc::from(frame), copies).await?;
            }
            visual_groups.push_back(1u64);
            last_visual_key = Some(visual_key);
        }
        progress(frame_index as u64 + 1, total_frames);
    }
    for frame in gpu.finish_frames()? {
        let copies = visual_groups
            .pop_front()
            .context("GPU drain 帧缺少重复计数")?;
        write_frame_copies(&mut video, Arc::from(frame), copies).await?;
    }
    if !visual_groups.is_empty() {
        bail!("GPU drain 后仍有未写入的视频帧");
    }
    let encoder_backpressure_elapsed = video.backpressure();
    video.finish().await?;
    let output_size = std::fs::metadata(output)?.len();
    let elapsed = started.elapsed();
    let gpu_timings = gpu.timings();
    Ok(RenderStats {
        frames: total_frames,
        duration_ms: render_duration_ms.round() as i64,
        elapsed,
        encoder: encoder_name.to_owned(),
        output_size,
        average_output_fps: total_frames as f64 / elapsed.as_secs_f64(),
        mesh_elapsed,
        gpu_submit_elapsed: gpu_timings.submit,
        readback_wait_elapsed: gpu_timings.readback_wait,
        cpu_copy_elapsed: gpu_timings.cpu_copy,
        encoder_backpressure_elapsed,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VisualKey {
    position: [u64; 3],
    rotation: [u32; 3],
    scene: (u64, Option<i32>),
}

async fn write_frame_copies(
    video: &mut VideoEncoder,
    frame: Arc<[u8]>,
    copies: u64,
) -> Result<()> {
    for _ in 0..copies {
        video.write_frame(Arc::clone(&frame)).await?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct FramePlan {
    replay_ms: i32,
    pose: CameraPose,
}

fn build_frame_plans(
    timeline: &ReplayTimeline,
    total_frames: u64,
    fps: u32,
    original_speed: bool,
    render_duration_ms: f64,
) -> Vec<FramePlan> {
    let source_span = f64::from(timeline.source_end_ms() - timeline.source_start_ms());
    (0..total_frames)
        .map(|frame_index| {
            let timeline_ms = frame_index as f64 * 1000.0 / f64::from(fps);
            let replay_ms = if original_speed {
                (f64::from(timeline.source_start_ms())
                    + timeline_ms / render_duration_ms * source_span)
                    .round() as i32
            } else {
                timeline.replay_time(timeline_ms).round() as i32
            };
            let camera_ms = if original_speed {
                timeline_ms * timeline.duration_ms() as f64 / render_duration_ms
            } else {
                timeline_ms
            };
            FramePlan {
                replay_ms,
                pose: timeline.camera(camera_ms),
            }
        })
        .collect()
}

fn required_chunks(plans: &[FramePlan], view_distance: i32) -> HashSet<(i32, i32)> {
    let camera_chunks = plans
        .iter()
        .map(|plan| {
            (
                (plan.pose.position[0].floor() as i32).div_euclid(16),
                (plan.pose.position[2].floor() as i32).div_euclid(16),
            )
        })
        .collect::<HashSet<_>>();
    let mut chunks = HashSet::new();
    for (center_x, center_z) in camera_chunks {
        for x in center_x - view_distance..=center_x + view_distance {
            for z in center_z - view_distance..=center_z + view_distance {
                chunks.insert((x, z));
            }
        }
    }
    chunks
}

fn walltime_clip_duration_ms(
    simulation_walltime: Duration,
    recorded_ticks: u64,
    source_ticks: f64,
) -> f64 {
    simulation_walltime.as_secs_f64() * 1000.0 * source_ticks / recorded_ticks as f64
}

fn validate_options(output: &Path, options: &RenderOptions) -> Result<()> {
    if options.width < 16
        || options.height < 16
        || options.width > 8192
        || options.height > 8192
        || !options.width.is_multiple_of(2)
        || !options.height.is_multiple_of(2)
    {
        bail!("视频宽高必须是 16..=8192 范围内的偶数");
    }
    if !(1..=240).contains(&options.fps) {
        bail!("视频 FPS 必须位于 1..=240");
    }
    if !(1..=500).contains(&options.bitrate_mbps) {
        bail!("视频码率必须位于 1..=500 Mbps");
    }
    if !(10.0..=140.0).contains(&options.fov_degrees) || !options.fov_degrees.is_finite() {
        bail!("视频 FOV 必须位于 10..=140");
    }
    if !matches!(options.resolved_quality().antialiasing, 1 | 2 | 4 | 8) {
        bail!("视频 AA 只支持 1, 2, 4 或 8");
    }
    if !(2..=32).contains(&options.view_distance_chunks) {
        bail!("视频 view distance 必须位于 2..=32 chunk");
    }
    if output.extension().and_then(|extension| extension.to_str()) != Some("mp4") {
        bail!("原生 replay 渲染首版只支持 .mp4 输出");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walltime_mode_uses_measured_simulation_tps() {
        let duration_ms = walltime_clip_duration_ms(Duration::from_secs(1), 3000, 3000.0);
        let game_duration_ms = 3000.0 * 50.0;

        assert_eq!(duration_ms, 1000.0);
        assert_eq!(game_duration_ms / duration_ms, 150.0);
    }

    #[test]
    fn quality_presets_and_explicit_overrides_resolve_in_order() {
        let balanced = RenderOptions {
            quality: RenderQuality::Balanced,
            ..RenderOptions::default()
        };
        assert_eq!(balanced.resolved_quality().antialiasing, 2);
        assert_eq!(balanced.resolved_quality().shadow_size, 1024);

        let overridden = RenderOptions {
            quality: RenderQuality::Fast,
            antialiasing: Some(8),
            shadows: Some(ShadowQuality::High),
            ..RenderOptions::default()
        };
        assert_eq!(overridden.resolved_quality().antialiasing, 8);
        assert_eq!(overridden.resolved_quality().shadow_size, 2048);
    }
}
