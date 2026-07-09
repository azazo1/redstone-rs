use std::path::PathBuf;

use clap::{Parser, Subcommand};
use redstone_rs::{
    io::{StructureBlock, StructureInput, TestVector}, BlockKind, BlockState, Position, SimulationSession,
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
            info!(
                blocks = snapshot.blocks.len(),
                elapsed_ms = elapsed.as_millis(),
                scheduled = session.world().scheduled_count(),
                "benchmark completed"
            );
        }
        Command::Vector { vector, output } => {
            let trace = TestVector::run_from_path(&vector).await?;
            let encoded = serde_json::to_vec_pretty(&trace)?;
            tokio::fs::write(&output, encoded).await?;
            info!(frames = trace.frames.len(), events = trace.events.len(), path = %output.display(), "test vector completed");
        }
    }

    Ok(())
}
