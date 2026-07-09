use std::path::PathBuf;

use clap::{Parser, Subcommand};
use redstone_rs::{
    io::{diff_traces, write_smoke_datapack, write_structure_template, SimulationTrace, StructureBlock, StructureInput, TestVector}, BlockKind, BlockState,
    Position, SimulationSession,
};
use tracing::{info, Level};
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Debug, Parser)]
#[command(name = "redstone-rs", about = "Java redstone simulator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Run {
        #[arg(long)]
        structure: PathBuf,
        #[arg(long, default_value_t = 1)]
        ticks: u64,
        #[arg(long)]
        trace: Option<PathBuf>,
    },
    Inspect {
        #[arg(long)]
        structure: PathBuf,
    },
    Benchmark {
        #[arg(long, default_value_t = 100_000)]
        blocks: usize,
        #[arg(long, default_value_t = 1_000)]
        ticks: u64,
    },
    Vector {
        #[arg(long)]
        vector: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Diff {
        #[arg(long)]
        expected: PathBuf,
        #[arg(long)]
        actual: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    OracleInit {
        #[arg(long)]
        output: PathBuf,
    },
    OracleStructure {
        #[arg(long)]
        structure: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(Level::INFO.as_str())))
        .with_target(true)
        .init();

    match Cli::parse().command {
        Command::Run { structure, ticks, trace } => {
            let input = StructureInput::from_path(&structure).await?;
            let mut session = SimulationSession::load_structure(input).await?;
            session.run_until(ticks).await;
            let snapshot = session.snapshot().await;
            info!(tick = snapshot.game_tick, blocks = snapshot.blocks.len(), "simulation completed");
            if let Some(path) = trace {
                let encoded = serde_json::to_vec_pretty(&session.trace().await)?;
                tokio::fs::write(&path, encoded).await?;
                info!(path = %path.display(), "trace written");
            }
        }
        Command::Inspect { structure } => {
            let input = StructureInput::from_path(&structure).await?;
            info!(blocks = input.blocks.len(), "structure loaded");
        }
        Command::Benchmark { blocks, ticks } => {
            info!(blocks, ticks, "building benchmark world");
            let structure = StructureInput {
                blocks: (0..blocks)
                    .map(|index| StructureBlock {
                        position: Position::new(
                            (index % 320) as i32,
                            (index / 320 % 16) as i32,
                            (index / 5_120) as i32,
                        ),
                        state: BlockState::new(BlockKind::Solid),
                    })
                    .collect(),
            };
            let mut session = SimulationSession::load_structure(structure).await?;
            let start = std::time::Instant::now();
            session.run_until(ticks).await;
            let elapsed = start.elapsed();
            let snapshot = session.snapshot().await;
            let elapsed_seconds = elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
            info!(
                blocks = snapshot.blocks.len(),
                elapsed_ms = elapsed.as_millis(),
                ticks_per_second = ticks as f64 / elapsed_seconds,
                block_ticks_per_second = snapshot.blocks.len() as f64 * ticks as f64 / elapsed_seconds,
                scheduled = session.world().scheduled_count(),
                peak_scheduled = session.world().peak_scheduled_count(),
                estimated_storage_bytes = session.world().estimated_storage_bytes(),
                "benchmark completed"
            );
        }
        Command::Vector { vector, output } => {
            let trace = TestVector::run_from_path(&vector).await?;
            let encoded = serde_json::to_vec_pretty(&trace)?;
            tokio::fs::write(&output, encoded).await?;
            info!(frames = trace.frames.len(), events = trace.events.len(), path = %output.display(), "test vector completed");
        }
        Command::Diff {
            expected,
            actual,
            output,
        } => {
            let expected_trace: SimulationTrace = serde_json::from_slice(&tokio::fs::read(&expected).await?)?;
            let actual_trace: SimulationTrace = serde_json::from_slice(&tokio::fs::read(&actual).await?)?;
            let differences = diff_traces(&expected_trace, &actual_trace);
            tokio::fs::write(&output, serde_json::to_vec_pretty(&differences)?).await?;
            info!(
                snapshots = differences.snapshots.len(),
                events = differences.events.len(),
                matched = differences.is_empty(),
                path = %output.display(),
                "simulation traces compared"
            );
        }
        Command::OracleInit { output } => {
            write_smoke_datapack(&output).await?;
            info!(path = %output.display(), "GameTest oracle datapack written");
        }
        Command::OracleStructure { structure, output } => {
            let input = StructureInput::from_path(&structure).await?;
            write_structure_template(&output, &input).await?;
            info!(blocks = input.blocks.len(), path = %output.display(), "GameTest structure template written");
        }
    }

    Ok(())
}
