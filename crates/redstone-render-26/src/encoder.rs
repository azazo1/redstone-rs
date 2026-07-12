use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::task::JoinHandle;

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
    child: Child,
    stdin: Option<ChildStdin>,
    stderr: Option<JoinHandle<Result<Vec<u8>, std::io::Error>>>,
    temporary: PathBuf,
    output: PathBuf,
}

impl VideoEncoder {
    pub(crate) async fn start(
        ffmpeg: &Path,
        encoder: &str,
        output: &Path,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_mbps: u32,
    ) -> Result<Self> {
        let temporary = temporary_output(output)?;
        let size = format!("{width}x{height}");
        let fps = fps.to_string();
        let bitrate = format!("{bitrate_mbps}M");
        let mut command = Command::new(ffmpeg);
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostats",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "bgra",
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
        ]);
        if encoder == "libx264" {
            command.args(["-preset", "veryfast"]);
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
        Ok(Self {
            child,
            stdin: Some(stdin),
            stderr: Some(stderr),
            temporary,
            output: output.to_path_buf(),
        })
    }

    pub(crate) async fn write_frame(&mut self, frame: &[u8]) -> Result<()> {
        self.stdin
            .as_mut()
            .context("FFmpeg stdin 已关闭")?
            .write_all(frame)
            .await
            .context("向 FFmpeg 写入视频帧失败")
    }

    pub(crate) async fn finish(mut self) -> Result<()> {
        if let Some(mut stdin) = self.stdin.take() {
            stdin.shutdown().await?;
        }
        let status = self.child.wait().await?;
        let stderr = self
            .stderr
            .take()
            .context("FFmpeg stderr 任务已结束")?
            .await
            .context("读取 FFmpeg stderr 任务失败")??;
        if !status.success() {
            let _ = std::fs::remove_file(&self.temporary);
            bail!(
                "FFmpeg 编码失败: {}",
                String::from_utf8_lossy(&stderr).trim()
            );
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
        let _ = std::fs::remove_file(&self.temporary);
    }
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
