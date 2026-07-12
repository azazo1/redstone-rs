use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use redstone_java_26::Java26Registry;
use redstone_replay_26::RenderTrace;

mod encoder;
mod gpu;
mod scene;
mod timeline;

pub use encoder::VideoEncoderKind;

use encoder::{VideoEncoder, select_encoder};
use gpu::GpuRenderer;
use scene::Scene;
use timeline::ReplayTimeline;

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_mbps: u32,
    pub fov_degrees: f32,
    pub antialiasing: u32,
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
            antialiasing: 4,
            encoder: VideoEncoderKind::Auto,
            ffmpeg: PathBuf::from("ffmpeg"),
            original_speed: false,
            view_distance_chunks: 32,
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
}

pub async fn render_replay(
    input: &Path,
    output: &Path,
    options: &RenderOptions,
    mut progress: impl FnMut(u64, u64),
) -> Result<RenderStats> {
    validate_options(output, options)?;
    let started = Instant::now();
    tracing::info!(path = %input.display(), "读取 replay 渲染轨迹");
    let trace = RenderTrace::read_mcpr(input)?;
    let timeline = ReplayTimeline::load(input)?;
    let render_duration_ms = if options.original_speed {
        if trace.recorded_ticks == 0 || trace.simulation_walltime.is_zero() {
            bail!("Replay 缺少有效模拟 walltime, 请用当前版本重新生成 replay");
        }
        let source_ticks = f64::from(timeline.source_end_ms() - timeline.source_start_ms()) / 50.0;
        walltime_clip_duration_ms(
            trace.simulation_walltime,
            trace.recorded_ticks,
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
    let encoder_name = select_encoder(&options.ffmpeg, options.encoder).await?;
    let mut registry = Java26Registry::new();
    let mut scene = Scene::from_trace(&trace, registry.air_state());
    let initial_pose = timeline.camera(0.0);
    let initial_meshes = scene.rebuild_visible(
        initial_pose.position,
        options.view_distance_chunks,
        &mut registry,
    )?;
    let initial_vertices = initial_meshes
        .iter()
        .map(|mesh| mesh.vertices.len())
        .sum::<usize>();
    tracing::info!(
        blocks = trace.initial_blocks.len(),
        vertices = initial_vertices,
        chunks = initial_meshes.len(),
        updates = trace.frames.len(),
        "构建 replay 初始渲染网格"
    );
    let mut gpu = GpuRenderer::new(
        options.width,
        options.height,
        options.antialiasing,
        options.fov_degrees,
    )
    .await?;
    gpu.update_meshes(initial_meshes)?;
    let mut video = VideoEncoder::start(
        &options.ffmpeg,
        encoder_name,
        output,
        options.width,
        options.height,
        options.fps,
        options.bitrate_mbps,
    )
    .await?;
    let mut trace_index = 0;
    for frame_index in 0..total_frames {
        let timeline_ms = frame_index as f64 * 1000.0 / f64::from(options.fps);
        let replay_ms = if options.original_speed {
            let source_span = f64::from(timeline.source_end_ms() - timeline.source_start_ms());
            (f64::from(timeline.source_start_ms())
                + timeline_ms / render_duration_ms * source_span)
                .round() as i32
        } else {
            timeline.replay_time(timeline_ms).round() as i32
        };
        while let Some(frame) = trace.frames.get(trace_index) {
            if frame.timestamp_ms > replay_ms {
                break;
            }
            scene.apply(&frame.changes);
            trace_index += 1;
        }
        let camera_ms = if options.original_speed {
            timeline_ms * timeline.duration_ms() as f64 / render_duration_ms
        } else {
            timeline_ms
        };
        let pose = timeline.camera(camera_ms);
        let mesh_updates = scene.rebuild_visible(
            pose.position,
            options.view_distance_chunks,
            &mut registry,
        )?;
        gpu.update_meshes(mesh_updates)?;
        let frame = gpu.render(pose)?;
        video.write_frame(&frame).await?;
        progress(frame_index + 1, total_frames);
    }
    video.finish().await?;
    let output_size = std::fs::metadata(output)?.len();
    Ok(RenderStats {
        frames: total_frames,
        duration_ms: render_duration_ms.round() as i64,
        elapsed: started.elapsed(),
        encoder: encoder_name.to_owned(),
        output_size,
    })
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
    if !matches!(options.antialiasing, 1 | 2 | 4 | 8) {
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
}
