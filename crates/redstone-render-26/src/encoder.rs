use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::FrameFormat;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VideoEncoderKind {
    #[default]
    Auto,
    Libx264,
    H264VideoToolbox,
    H264Nvenc,
    H264Qsv,
    H264Amf,
}

impl VideoEncoderKind {
    fn ffmpeg_name(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Libx264 => Some("libx264"),
            Self::H264VideoToolbox => Some("h264_videotoolbox"),
            Self::H264Nvenc => Some("h264_nvenc"),
            Self::H264Qsv => Some("h264_qsv"),
            Self::H264Amf => Some("h264_amf"),
        }
    }
}

pub(crate) async fn select_encoder(
    ffmpeg: &Path,
    requested: VideoEncoderKind,
) -> Result<&'static str> {
    if let Some(name) = requested.ffmpeg_name() {
        if probe_encoder(ffmpeg, name).await? {
            return Ok(name);
        }
        bail!("FFmpeg 编码器不可用: {name}");
    }
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["h264_videotoolbox", "libx264"]
    } else if cfg!(target_os = "windows") {
        &["h264_nvenc", "h264_qsv", "h264_amf", "libx264"]
    } else {
        &["h264_nvenc", "h264_qsv", "libx264"]
    };
    for &candidate in candidates {
        if probe_encoder(ffmpeg, candidate).await.unwrap_or(false) {
            tracing::info!(encoder = candidate, "选择 FFmpeg H.264 编码器");
            return Ok(candidate);
        }
    }
    bail!("FFmpeg 没有可用的 H.264 编码器");
}

async fn probe_encoder(ffmpeg: &Path, encoder: &str) -> Result<bool> {
    let mut child = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "bgra",
            "-s",
            "64x64",
            "-r",
            "1",
            "-i",
            "-",
            "-frames:v",
            "1",
            "-an",
            "-c:v",
            encoder,
            "-f",
            "null",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("启动 FFmpeg 失败: {}", ffmpeg.display()))?;
    let mut stdin = child.stdin.take().context("FFmpeg probe 缺少 stdin")?;
    stdin.write_all(&vec![0; 64 * 64 * 4]).await?;
    drop(stdin);
    Ok(child.wait().await?.success())
}

pub(crate) struct VideoEncoder {
    sender: Option<mpsc::Sender<Arc<[u8]>>>,
    writer: Option<JoinHandle<Result<()>>>,
    temporary: PathBuf,
    output: PathBuf,
    backpressure: Duration,
}

pub(crate) struct EncoderSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_mbps: u32,
    pub frame_format: FrameFormat,
}

impl VideoEncoder {
    pub(crate) async fn start(
        ffmpeg: &Path,
        encoder: &str,
        output: &Path,
        settings: EncoderSettings,
    ) -> Result<Self> {
        let EncoderSettings {
            width,
            height,
            fps,
            bitrate_mbps,
            frame_format,
        } = settings;
        let temporary = temporary_output(output)?;
        let size = format!("{width}x{height}");
        let fps = fps.to_string();
        let bitrate = format!("{bitrate_mbps}M");
        let mut command = Command::new(ffmpeg);
        let input_pixel_format = match frame_format {
            FrameFormat::Bgra => "bgra",
            FrameFormat::Nv12 => "nv12",
        };
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostats",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            input_pixel_format,
            "-s",
            &size,
            "-r",
            &fps,
            "-i",
            "-",
            "-an",
            "-c:v",
            encoder,
            "-b:v",
            &bitrate,
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
            "-bsf:v",
            "h264_metadata=video_full_range_flag=0:colour_primaries=1:transfer_characteristics=1:matrix_coefficients=1",
        ]);
        if encoder == "libx264" {
            command.args(["-preset", "veryfast"]);
        } else if encoder == "h264_videotoolbox" {
            command.args(["-realtime", "1", "-prio_speed", "1"]);
        }
        command.arg(&temporary);
        command.kill_on_drop(true);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("启动 FFmpeg 失败: {}", ffmpeg.display()))?;
        let stdin = child.stdin.take().context("FFmpeg 缺少 stdin")?;
        let mut child_stderr = child.stderr.take().context("FFmpeg 缺少 stderr")?;
        let stderr = tokio::spawn(async move {
            let mut bytes = Vec::new();
            child_stderr.read_to_end(&mut bytes).await?;
            Ok(bytes)
        });
        let (sender, receiver) = mpsc::channel(4);
        let writer = tokio::spawn(run_writer(child, stdin, stderr, receiver));
        Ok(Self {
            sender: Some(sender),
            writer: Some(writer),
            temporary,
            output: output.to_path_buf(),
            backpressure: Duration::ZERO,
        })
    }

    pub(crate) async fn write_frame(&mut self, frame: Arc<[u8]>) -> Result<()> {
        let started = Instant::now();
        let result = self
            .sender
            .as_ref()
            .context("FFmpeg writer 已关闭")?
            .send(frame)
            .await
            .context("FFmpeg writer 提前结束");
        self.backpressure += started.elapsed();
        if result.is_err() {
            self.wait_writer().await?;
        }
        result
    }

    pub(crate) fn backpressure(&self) -> Duration {
        self.backpressure
    }

    pub(crate) async fn finish(mut self) -> Result<()> {
        self.sender.take();
        if let Err(error) = self.wait_writer().await {
            let _ = std::fs::remove_file(&self.temporary);
            return Err(error);
        }
        std::fs::rename(&self.temporary, &self.output).with_context(|| {
            format!(
                "原子替换视频失败: {} -> {}",
                self.temporary.display(),
                self.output.display()
            )
        })?;
        Ok(())
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take() {
            writer.abort();
        }
        let _ = std::fs::remove_file(&self.temporary);
    }
}

impl VideoEncoder {
    async fn wait_writer(&mut self) -> Result<()> {
        self.writer
            .take()
            .context("FFmpeg writer 任务已结束")?
            .await
            .context("FFmpeg writer 任务失败")?
    }
}

async fn run_writer(
    mut child: Child,
    mut stdin: ChildStdin,
    stderr: JoinHandle<Result<Vec<u8>, std::io::Error>>,
    mut receiver: mpsc::Receiver<Arc<[u8]>>,
) -> Result<()> {
    while let Some(frame) = receiver.recv().await {
        stdin
            .write_all(&frame)
            .await
            .context("向 FFmpeg 写入视频帧失败")?;
    }
    stdin.shutdown().await?;
    drop(stdin);
    let status = child.wait().await?;
    let stderr = stderr
        .await
        .context("读取 FFmpeg stderr 任务失败")??;
    if !status.success() {
        bail!(
            "FFmpeg 编码失败: {}",
            String::from_utf8_lossy(&stderr).trim()
        );
    }
    Ok(())
}

fn temporary_output(output: &Path) -> Result<PathBuf> {
    let file_name = output
        .file_name()
        .context("视频输出路径缺少文件名")?
        .to_string_lossy();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(output
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            ".{file_name}.redstone-{}-{nonce}.mp4",
            std::process::id()
        )))
}
