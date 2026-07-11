use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use redstone_core::{
    Action, BlockPos, BlockStateId, GameTick, ProbeValue, RedstoneMode, Simulation,
    SimulationConfig, SparseWorld, TraceEvent, TraceKind, WorldPaste,
};
use redstone_io::{
    InitializationMode, Scenario, ScenarioActionKind, StructureLoader, StructureRegion,
    StructureStateResolver,
};
use redstone_java_26::{
    JAVA_DATA_VERSION, JAVA_VERSION, Java26Registry, Java26Rules, StateResolveError, StateResolver,
};
use redstone_replay_26::{ReplayOptions, ReplayRegion, ReplayStats, ReplayWriter};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command as ProcessCommand;
use tokio::sync::Semaphore;
use tracing::{Instrument, debug, info};
use tracing_indicatif::{IndicatifLayer, span_ext::IndicatifSpanExt, style::ProgressStyle};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod convert;
mod inspect;
mod replay_camera;

#[derive(Debug, Parser)]
#[command(name = "redstone", version, about = "Java 26.1.2 红石时序仿真器")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Inspect {
        #[arg(help = "要检查的 litematic, schem 或 structure NBT 文件")]
        structure: PathBuf,
        #[arg(
            long,
            value_name = "X_RANGE,Y_RANGE,Z_RANGE",
            value_parser = inspect::parse_finite_region,
            help = "读取 Minecraft 世界目录时限制有限区域"
        )]
        region: Option<StructureRegion>,
        #[arg(
            long = "block",
            value_name = "X_OR_RANGE,Y_OR_RANGE,Z_OR_RANGE",
            value_parser = inspect::parse_block_selector,
            conflicts_with = "all",
            help = "按坐标或 Rust 风格范围查询方块, 可重复使用"
        )]
        blocks: Vec<inspect::BlockSelector>,
        #[arg(
            long,
            conflicts_with_all = ["blocks", "block_types"],
            help = "输出全部非空气方块"
        )]
        all: bool,
        #[arg(
            long = "type",
            visible_aliases = ["block-type", "name"],
            value_name = "BLOCK_ID",
            conflicts_with = "all",
            help = "按方块 ID 筛选, 可重复使用"
        )]
        block_types: Vec<String>,
        #[arg(
            long,
            value_enum,
            default_value = "text",
            help = "选择文本或 JSON 输出"
        )]
        format: inspect::OutputFormat,
        #[arg(long, conflicts_with = "format", help = "使用 JSON 输出")]
        json: bool,
    },
    Run {
        scenario: PathBuf,
        #[arg(long)]
        replay: Option<PathBuf>,
        #[arg(
            long,
            requires = "replay",
            help = "在 Replay Mod 录像中保留活塞动画和声音"
        )]
        replay_anim: bool,
        #[arg(long)]
        trace: Option<PathBuf>,
        #[arg(long)]
        vcd: Option<PathBuf>,
        #[arg(long)]
        allow_static_fallback: bool,
    },
    Test {
        path: PathBuf,
        #[arg(long)]
        replay: Option<PathBuf>,
        #[arg(
            long,
            requires = "replay",
            help = "在 Replay Mod 录像中保留活塞动画和声音"
        )]
        replay_anim: bool,
        #[arg(long)]
        oracle: bool,
        #[arg(long)]
        allow_static_fallback: bool,
    },
    Trace {
        scenario: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        vcd: Option<PathBuf>,
    },
    Bench {
        #[arg(long, default_value_t = 1_000_000)]
        blocks: usize,
        #[arg(long, default_value_t = 10_000)]
        active: usize,
        #[arg(long, default_value_t = 100)]
        ticks: usize,
    },
    Convert {
        input: PathBuf,
        output: PathBuf,
        #[arg(
            long,
            value_name = "X_RANGE,Y_RANGE,Z_RANGE",
            value_parser = inspect::parse_finite_region,
            help = "读取 Minecraft 世界目录时限制有限区域"
        )]
        region: Option<StructureRegion>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let indicatif_layer = IndicatifLayer::new();
    let writer = indicatif_layer.get_stderr_writer();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_target(true)
                .with_timer(tracing_subscriber::fmt::time::uptime()),
        )
        .with(indicatif_layer)
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Inspect {
            structure,
            region,
            blocks,
            all,
            block_types,
            format,
            json,
        } => inspect::run(
            &structure,
            region,
            &blocks,
            all,
            &block_types,
            if json {
                inspect::OutputFormat::Json
            } else {
                format
            },
        ),
        Command::Run {
            scenario,
            replay,
            replay_anim,
            trace,
            vcd,
            allow_static_fallback,
        } => run(
                &scenario,
                replay.as_deref(),
                replay_anim,
                trace.as_deref(),
                vcd.as_deref(),
                allow_static_fallback,
            ),
        Command::Test {
            path,
            replay,
            replay_anim,
            oracle,
            allow_static_fallback,
        } => {
            test_path(
                &path,
                replay.as_deref(),
                replay_anim,
                oracle,
                allow_static_fallback,
            )
            .await
        }
        Command::Trace {
            scenario,
            output,
            vcd,
        } => run(&scenario, None, false, Some(&output), vcd.as_deref(), false),
        Command::Bench {
            blocks,
            active,
            ticks,
        } => bench(blocks, active, ticks),
        Command::Convert {
            input,
            output,
            region,
        } => convert::run(&input, &output, region),
    }
}

fn bench(blocks: usize, active: usize, ticks: usize) -> Result<()> {
    if blocks == 0 || ticks == 0 {
        bail!("blocks 和 ticks 必须大于 0");
    }
    if active > blocks {
        bail!("active 不能大于 blocks");
    }
    let started = Instant::now();
    let mut registry = Java26Registry::new();
    let stone = registry.resolve_state("minecraft:stone", &BTreeMap::new())?;
    let pressure_plate = registry.resolve_state(
        "minecraft:oak_pressure_plate",
        &BTreeMap::from([("powered".to_owned(), "false".to_owned())]),
    )?;
    let mut world = SparseWorld::new(registry.air_state());
    let side = cube_side(blocks);
    let progress = tracing::info_span!("benchmark_world");
    progress.pb_set_style(
        &ProgressStyle::with_template("{span_child_prefix}{msg} {wide_bar} {pos}/{len}")?
            .progress_chars("=>-"),
    );
    progress.pb_set_length(blocks as u64);
    progress.pb_set_message("构建基准世界");
    progress.pb_start();
    for index in 0..blocks {
        let pos = benchmark_position(index, side)?;
        world.set_block(
            pos,
            if index < active {
                pressure_plate
            } else {
                stone
            },
        )?;
        if index % 16_384 == 0 {
            progress.pb_set_position(index as u64);
        }
    }
    progress.pb_set_position(blocks as u64);
    let build_elapsed = started.elapsed();
    let sections = world.section_count();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())?;
    let mut samples = Vec::with_capacity(ticks);
    info!(blocks, active, sections, ?build_elapsed, "完成基准世界构建");
    for _ in 0..ticks {
        let started = Instant::now();
        simulation.step()?;
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    println!("blocks: {blocks}");
    println!("active_components: {active}");
    println!("sections: {sections}");
    println!("build_ms: {:.3}", build_elapsed.as_secs_f64() * 1_000.0);
    println!(
        "tick_p50_ms: {:.6}",
        percentile(&samples, 50).as_secs_f64() * 1_000.0
    );
    println!(
        "tick_p95_ms: {:.6}",
        percentile(&samples, 95).as_secs_f64() * 1_000.0
    );
    println!(
        "tick_p99_ms: {:.6}",
        percentile(&samples, 99).as_secs_f64() * 1_000.0
    );
    if let Some(rss) = resident_memory_bytes() {
        println!("resident_memory_mib: {:.2}", rss as f64 / 1024.0 / 1024.0);
    }
    Ok(())
}

fn cube_side(blocks: usize) -> usize {
    let mut side = 1usize;
    while side.saturating_mul(side).saturating_mul(side) < blocks {
        side += 1;
    }
    side
}

fn benchmark_position(index: usize, side: usize) -> Result<BlockPos> {
    let layer = side.saturating_mul(side);
    Ok(BlockPos::new(
        i32::try_from(index % side)?,
        i32::try_from(index / layer)?,
        i32::try_from((index / side) % side)?,
    ))
}

fn percentile(samples: &[Duration], percentile: usize) -> Duration {
    let index = (samples.len() - 1) * percentile / 100;
    samples[index]
}

fn resident_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let resident_pages = statm.split_whitespace().nth(1)?.parse::<u64>().ok()?;
        return Some(resident_pages.saturating_mul(4_096));
    }
    #[cfg(target_os = "macos")]
    {
        let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_taskinfo>();
        // SAFETY: proc_pidinfo writes at most size bytes into the correctly sized output buffer.
        let written = unsafe {
            libc::proc_pidinfo(
                std::process::id() as i32,
                libc::PROC_PIDTASKINFO,
                0,
                info.as_mut_ptr().cast(),
                size as i32,
            )
        };
        if written != size as i32 {
            return None;
        }
        // SAFETY: a full-size proc_pidinfo result initialized the entire proc_taskinfo value.
        let info = unsafe { info.assume_init() };
        return Some(info.pti_resident_size);
    }
    #[allow(unreachable_code)]
    None
}

#[derive(Debug)]
struct RunSummary {
    ticks: u64,
    blocks: usize,
    trace_events: usize,
    tick_elapsed: Duration,
}

const TICK_PROGRESS_UPDATE_INTERVAL: u64 = 10;

fn simulation_progress_style() -> Result<ProgressStyle> {
    Ok(ProgressStyle::with_template(
        "{span_child_prefix}{spinner:.green} {msg} [{bar:28.green}] {pos}/{len} {per_sec:2} ETA:{eta}",
    )?
    .progress_chars("=> "))
}

fn oracle_progress_style() -> Result<ProgressStyle> {
    Ok(ProgressStyle::with_template(
        "{span_child_prefix}{spinner} {msg:72!}",
    )?)
}

fn run(
    scenario_path: &Path,
    replay_path: Option<&Path>,
    replay_anim: bool,
    trace_path: Option<&Path>,
    vcd_path: Option<&Path>,
    allow_static_fallback: bool,
) -> Result<()> {
    let summary = execute_scenario(
        scenario_path,
        replay_path,
        replay_anim,
        trace_path,
        vcd_path,
        allow_static_fallback,
    )?;
    println!("ticks: {}", summary.ticks);
    println!("blocks: {}", summary.blocks);
    println!("trace_events: {}", summary.trace_events);
    println!("tick_ms: {:.3}", summary.tick_elapsed.as_secs_f64() * 1_000.0);
    println!(
        "ticks_per_second: {:.3}",
        summary.ticks as f64 / summary.tick_elapsed.as_secs_f64()
    );
    println!("expectations: passed");
    Ok(())
}

fn execute_scenario(
    scenario_path: &Path,
    replay_path: Option<&Path>,
    replay_anim: bool,
    trace_path: Option<&Path>,
    vcd_path: Option<&Path>,
    allow_static_fallback: bool,
) -> Result<RunSummary> {
    let scenario = Scenario::load(scenario_path)?;
    if scenario.version != JAVA_VERSION {
        bail!("场景版本必须是 {JAVA_VERSION}, 收到 {}", scenario.version);
    }
    let mut resolver = RegistryResolver(Java26Registry::new());
    let loaded = StructureLoader::load_with_options(
        &scenario.source.path,
        scenario.source.load_options(),
        &mut resolver,
    )?;
    reject_newer_data_version(loaded.data_version)?;
    let mut replay_region = ReplayRegion::new(loaded.region_min, loaded.region_max);
    let mut pastes = Vec::with_capacity(scenario.source.pastes.len());
    for paste in &scenario.source.pastes {
        if paste
            .tick
            .is_some_and(|tick| tick.0 == 0 || tick.0 > scenario.max_ticks)
        {
            bail!(
                "附加结构 {} 的 tick 必须在 1..={} 范围内",
                paste.path.display(),
                scenario.max_ticks
            );
        }
        let structure = StructureLoader::load_with_options(
            &paste.path,
            paste.load_options(),
            &mut resolver,
        )?;
        reject_newer_data_version(structure.data_version)?;
        if paste.tick.is_none() {
            replay_region = union_replay_regions(
                replay_region,
                ReplayRegion::new(structure.region_min, structure.region_max),
            );
        }
        pastes.push((paste, structure));
    }
    let mut actions = scenario
        .actions
        .iter()
        .map(|action| {
            Ok((
                action.tick,
                resolve_scenario_action(&mut resolver.0, &action.action)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    actions.sort_by_key(|(tick, _)| *tick);

    let camera_hints = replay_camera::hints(
        &scenario,
        std::iter::once(&loaded.world).chain(
            pastes
                .iter()
                .filter(|(paste, _)| paste.tick.is_none())
                .map(|(_, structure)| &structure.world),
        ),
        &resolver.0,
    );

    let rules = Java26Rules::new(resolver.0);
    let mut simulation = Simulation::load(
        rules,
        loaded.world,
        SimulationConfig {
            mode: scenario.mode,
            seed: scenario.seed,
            environment: scenario.environment.into(),
            strict: scenario.strict && !allow_static_fallback,
            trace: trace_path.is_some() || vcd_path.is_some(),
            record_events: replay_path.is_some(),
            ..SimulationConfig::default()
        },
    )?;
    for probe in &scenario.probes {
        simulation.add_probe(&probe.name, probe.probe.clone());
    }
    if scenario.source.initialization == InitializationMode::Notify {
        simulation.initialize()?;
    }
    for (paste, structure) in pastes.iter().filter(|(paste, _)| paste.tick.is_none()) {
        simulation.paste_world(
            &structure.world,
            structure.region_min,
            structure.region_max,
            paste.ignore_air,
            paste.paste_entities,
        )?;
        if paste.update {
            simulation.update_region(structure.region_min, structure.region_max)?;
        }
        info!(
            path = %paste.path.display(),
            origin = ?paste.origin,
            ignore_air = paste.ignore_air,
            paste_entities = paste.paste_entities,
            update = paste.update,
            "粘贴场景附加结构"
        );
    }

    let mut replay = if let Some(path) = replay_path {
        let name = scenario_path.file_name().map_or_else(
            || scenario_path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let camera = replay_camera::options(&scenario);
        Some(
            ReplayWriter::new(
                path,
                ReplayOptions::new(
                    name,
                    scenario.seed,
                    scenario.mode == RedstoneMode::Experimental,
                    replay_region,
                )
                .with_environment(scenario.environment.into())
                .with_piston_animation(replay_anim)
                .with_camera(camera)
                .with_camera_hints(camera_hints),
                simulation.world(),
            )
            .with_context(|| format!("初始化 Replay Mod 录像失败: {}", path.display()))?,
        )
    } else {
        None
    };

    let mut action_index = 0;
    let mut samples = BTreeMap::<(GameTick, String), ProbeValue>::new();
    let progress = tracing::info_span!("scenario_ticks");
    progress.pb_set_style(&simulation_progress_style()?);
    progress.pb_set_length(scenario.max_ticks);
    progress.pb_set_message(&format!(
        "计算 ticks {}",
        scenario_path
            .file_name()
            .unwrap_or(scenario_path.as_os_str())
            .to_string_lossy()
    ));
    progress.pb_start();
    let tick_started = Instant::now();
    while simulation.current_tick().0 < scenario.max_ticks {
        let next_tick = GameTick(simulation.current_tick().0 + 1);
        let current_pastes = pastes
            .iter()
            .filter(|(paste, _)| paste.tick == Some(next_tick))
            .map(|(paste, structure)| {
                info!(
                    path = %paste.path.display(),
                    tick = next_tick.0,
                    origin = ?paste.origin,
                    ignore_air = paste.ignore_air,
                    paste_entities = paste.paste_entities,
                    update = paste.update,
                    "执行场景附加结构粘贴"
                );
                WorldPaste {
                    source: &structure.world,
                    region_min: structure.region_min,
                    region_max: structure.region_max,
                    ignore_air: paste.ignore_air,
                    paste_entities: paste.paste_entities,
                    update: paste.update,
                }
            })
            .collect::<Vec<_>>();
        let mut current_actions = Vec::new();
        while actions
            .get(action_index)
            .is_some_and(|(tick, _)| *tick == next_tick)
        {
            current_actions.push(actions[action_index].1.clone());
            action_index += 1;
        }
        let delta = simulation.step_with_pastes_and_actions(&current_pastes, &current_actions)?;
        let completed_tick = delta.tick.0;
        if let Some(replay) = replay.as_mut() {
            replay.record_delta(&delta).with_context(|| {
                format!(
                    "编码 Replay Mod tick {} 失败: {}",
                    delta.tick.0,
                    replay_path.expect("replay path must exist").display()
                )
            })?;
        }
        for sample in delta.probes {
            samples.insert((delta.tick, sample.name), sample.value);
        }
        if completed_tick % TICK_PROGRESS_UPDATE_INTERVAL == 0
            || completed_tick == scenario.max_ticks
        {
            progress.pb_set_position(completed_tick);
        }
    }
    let tick_elapsed = tick_started.elapsed();
    drop(progress);

    let mut failures = Vec::new();
    for expectation in scenario.expectations() {
        let actual = samples.get(&(expectation.tick, expectation.probe.clone()));
        if actual != Some(&expectation.equals) {
            failures.push(format!(
                "tick {} probe {}: expected {:?}, actual {:?}",
                expectation.tick.0, expectation.probe, expectation.equals, actual
            ));
        }
    }
    if let Some(replay) = replay {
        let path = replay_path.expect("replay path must exist");
        let stats = replay
            .finish()
            .with_context(|| format!("完成 Replay Mod 录像失败: {}", path.display()))?;
        log_replay_stats(path, &stats);
    }
    if let Some(path) = trace_path {
        let output =
            File::create(path).with_context(|| format!("创建轨迹文件失败: {}", path.display()))?;
        simulation.trace().write_jsonl(output)?;
        debug!(path = %path.display(), "写入 JSONL 轨迹");
    }
    if let Some(path) = vcd_path {
        let output =
            File::create(path).with_context(|| format!("创建 VCD 文件失败: {}", path.display()))?;
        simulation.trace().write_vcd(output)?;
        info!(path = %path.display(), "写入 VCD 波形");
    }
    if !failures.is_empty() {
        bail!("{} 个断言失败\n{}", failures.len(), failures.join("\n"));
    }
    Ok(RunSummary {
        ticks: simulation.current_tick().0,
        blocks: simulation.world().non_air_blocks(),
        trace_events: simulation.trace().events().len(),
        tick_elapsed,
    })
}

fn union_replay_regions(left: ReplayRegion, right: ReplayRegion) -> ReplayRegion {
    ReplayRegion::new(
        BlockPos::new(
            left.min.x.min(right.min.x),
            left.min.y.min(right.min.y),
            left.min.z.min(right.min.z),
        ),
        BlockPos::new(
            left.max.x.max(right.max.x),
            left.max.y.max(right.max.y),
            left.max.z.max(right.max.z),
        ),
    )
}

fn log_replay_stats(path: &Path, stats: &ReplayStats) {
    info!(
        path = %path.display(),
        ticks = stats.ticks,
        packets = stats.packets,
        block_updates = stats.block_updates,
        file_size = stats.file_size,
        elapsed_ms = stats.elapsed.as_secs_f64() * 1_000.0,
        "导出 Replay Mod 录像"
    );
}

async fn test_path(
    path: &Path,
    replay: Option<&Path>,
    replay_anim: bool,
    oracle: bool,
    allow_static_fallback: bool,
) -> Result<()> {
    if path.is_dir() && replay.is_some() {
        bail!("目录测试暂不支持 --replay, 请指定单个场景文件");
    }
    let mut scenarios = if path.is_dir() {
        std::fs::read_dir(path)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "toml")
            })
            .collect::<Vec<_>>()
    } else {
        vec![path.to_path_buf()]
    };
    scenarios.sort();
    if scenarios.is_empty() {
        bail!("没有找到 TOML 场景");
    }
    let concurrency = std::thread::available_parallelism().map_or(1, usize::from);
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let progress = tracing::info_span!("scenario_tests");
    progress.pb_set_style(&simulation_progress_style()?);
    progress.pb_set_length(scenarios.len() as u64);
    progress.pb_set_message("批量场景");
    progress.pb_start();
    let mut tasks = Vec::new();
    for (index, scenario) in scenarios.into_iter().enumerate() {
        let permit = semaphore.clone().acquire_owned().await?;
        let progress = progress.clone();
        let progress_parent = progress.clone();
        let replay = replay.map(Path::to_path_buf);
        tasks.push(tokio::spawn(
            async move {
                let _permit = permit;
                let result = execute_test_scenario(
                    &scenario,
                    replay.as_deref(),
                    replay_anim,
                    oracle,
                    allow_static_fallback,
                )
                .await;
                progress.pb_inc(1);
                (index, scenario, result)
            }
            .instrument(progress_parent),
        ));
    }
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await?);
    }
    drop(progress);
    results.sort_by_key(|(index, _, _)| *index);
    let mut failures = 0;
    for (_, scenario, result) in results {
        match result {
            Ok(summary) => println!(
                "PASS {} ticks={} trace_events={}",
                scenario.display(),
                summary.ticks,
                summary.trace_events
            ),
            Err(error) => {
                failures += 1;
                eprintln!("FAIL {}\n{error:#}", scenario.display());
            }
        }
    }
    if failures == 0 {
        Ok(())
    } else {
        Err(anyhow!("{failures} 个场景失败"))
    }
}

async fn execute_test_scenario(
    scenario: &Path,
    replay: Option<&Path>,
    replay_anim: bool,
    oracle: bool,
    allow_static_fallback: bool,
) -> Result<RunSummary> {
    let run_oracle = if oracle {
        let parsed = Scenario::load(scenario)?;
        if parsed.skip_oracle {
            info!(parent: None,
                scenario = %scenario.display(),
                "跳过 Java oracle 对照: 场景设置了 skip-oracle"
            );
            false
        } else {
            true
        }
    } else {
        false
    };
    let trace_path = run_oracle.then(|| {
        std::env::temp_dir().join(format!(
            "redstone-rust-trace-{}-{}.jsonl",
            std::process::id(),
            stable_path_hash(scenario)
        ))
    });
    let result = execute_scenario(
        scenario,
        replay,
        replay_anim,
        trace_path.as_deref(),
        None,
        allow_static_fallback,
    );
    let comparison = if let (Ok(_), Some(trace_path)) = (&result, trace_path.as_deref()) {
        compare_with_oracle(scenario, trace_path).await
    } else {
        Ok(())
    };
    if let Some(trace_path) = trace_path {
        let _ = std::fs::remove_file(trace_path);
    }
    comparison?;
    result
}

async fn compare_with_oracle(scenario: &Path, rust_trace: &Path) -> Result<()> {
    let oracle = std::env::var_os("REDSTONE_ORACLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tools/vanilla-oracle/run.sh"));
    if !oracle.is_file() {
        bail!(
            "--oracle 需要可执行探针 {}, 或设置 REDSTONE_ORACLE",
            oracle.display()
        );
    }
    let oracle_trace = std::env::temp_dir().join(format!(
        "redstone-java-trace-{}-{}.jsonl",
        std::process::id(),
        stable_path_hash(scenario)
    ));
    let mut command = if oracle
        .extension()
        .is_some_and(|extension| extension == "sh")
    {
        let mut command = ProcessCommand::new("sh");
        command.arg(&oracle);
        command
    } else {
        ProcessCommand::new(&oracle)
    };
    let scenario_name = scenario
        .file_name()
        .unwrap_or(scenario.as_os_str())
        .to_string_lossy()
        .into_owned();
    let progress = tracing::info_span!("java_oracle");
    progress.pb_set_style(&oracle_progress_style()?);
    progress.pb_set_message(&format!("Java oracle {scenario_name}"));
    progress.pb_start();
    let comparison = async {
        command
            .arg(scenario)
            .arg(&oracle_trace)
            .env(
                "REDSTONE_ORACLE_CONVERTER",
                convert::converter_executable()?,
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("启动 Java oracle 失败: {}", oracle.display()))?;
        let stdout = child
            .stdout
            .take()
            .context("捕获 Java oracle stdout 失败")?;
        let stderr = child
            .stderr
            .take()
            .context("捕获 Java oracle stderr 失败")?;
        let (status, stdout, stderr) = tokio::join!(
            child.wait(),
            capture_oracle_stream(stdout, "stdout", &scenario_name, &progress),
            capture_oracle_stream(stderr, "stderr", &scenario_name, &progress),
        );
        let status = status.context("等待 Java oracle 失败")?;
        let stdout = stdout.context("读取 Java oracle stdout 失败")?;
        let stderr = stderr.context("读取 Java oracle stderr 失败")?;
        if !status.success() {
            bail!(
                "Java oracle 返回失败状态: {status}\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
        }
        progress.pb_set_message(&format!("Java oracle {scenario_name}: 比较轨迹"));
        let rust = std::fs::read(rust_trace)?;
        let java = std::fs::read(&oracle_trace)?;
        if is_oracle_samples_v2(&java)? {
            compare_oracle_samples_v2(&rust, &java)
        } else if is_probe_sample_oracle(&java)? {
            compare_probe_samples(&rust, &java)
        } else if rust != java {
            let line = first_different_line(&rust, &java);
            bail!("Rust 与 Java oracle 轨迹不一致, 首个差异位于第 {line} 行");
        } else {
            Ok(())
        }
    }
    .await;
    drop(progress);
    let _ = std::fs::remove_file(&oracle_trace);
    comparison
}

async fn capture_oracle_stream(
    stream: impl AsyncRead + Unpin,
    stream_name: &'static str,
    scenario_name: &str,
    progress: &tracing::Span,
) -> std::io::Result<Vec<u8>> {
    let mut reader = BufReader::new(stream);
    let mut captured = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).await? == 0 {
            break;
        }
        captured.extend_from_slice(&line);
        let message = String::from_utf8_lossy(&line);
        let message = message.trim();
        if !message.is_empty() {
            progress.pb_set_message(&format!("Java oracle {scenario_name}: {message}"));
            debug!(
                parent: progress,
                stream = stream_name,
                output = message,
                "Java oracle 输出"
            );
        }
    }
    Ok(captured)
}

#[derive(Debug, serde::Deserialize, Eq, PartialEq)]
struct OracleProbeSample {
    tick: u64,
    probe: String,
    value: serde_json::Value,
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq)]
struct OracleNeighborSample {
    tick: u64,
    pos: BlockPos,
    moved_by_piston: bool,
    orientation: Option<u8>,
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq)]
struct OracleScheduledTickQueuedSample {
    tick: u64,
    pos: BlockPos,
    trigger_tick: u64,
    priority: i8,
    sub_tick_order: i64,
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq)]
struct OracleScheduledTickExecutedSample {
    tick: u64,
    pos: BlockPos,
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq)]
struct OracleBlockEventSample {
    tick: u64,
    pos: BlockPos,
    param_a: i32,
    param_b: i32,
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq)]
struct OracleBlockChangeSample {
    tick: u64,
    pos: BlockPos,
    old_state: u32,
    new_state: u32,
}

#[derive(Debug, Eq, PartialEq)]
enum OracleMicroSample {
    Neighbor(OracleNeighborSample),
    ScheduledTickQueued(OracleScheduledTickQueuedSample),
    ScheduledTickExecuted(OracleScheduledTickExecutedSample),
    BlockEventQueued(OracleBlockEventSample),
    BlockEventExecuted(OracleBlockEventSample),
    BlockChanged(OracleBlockChangeSample),
}

fn is_probe_sample_oracle(output: &[u8]) -> Result<bool> {
    oracle_format(output).map(|format| format.as_deref() == Some("probe_samples_v1"))
}

fn is_oracle_samples_v2(output: &[u8]) -> Result<bool> {
    oracle_format(output).map(|format| format.as_deref() == Some("oracle_samples_v2"))
}

fn oracle_format(output: &[u8]) -> Result<Option<String>> {
    let Some(first_line) = output.split(|byte| *byte == b'\n').next() else {
        return Ok(None);
    };
    if first_line.is_empty() {
        return Ok(None);
    }
    let marker = serde_json::from_slice::<serde_json::Value>(first_line)
        .context("解析 Java oracle 格式标记失败")?;
    Ok(marker
        .get("format")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned))
}

fn compare_probe_samples(rust_trace: &[u8], java_output: &[u8]) -> Result<()> {
    let rust_events = rust_trace
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<TraceEvent>(line).context("解析 Rust 轨迹失败"))
        .collect::<Result<Vec<_>>>()?;
    let rust_samples = rust_events
        .into_iter()
        .filter_map(|event| match event.kind {
            TraceKind::ProbeSample { sample } => Some(OracleProbeSample {
                tick: event.tick.0,
                probe: sample.name,
                value: probe_value_json(sample.value),
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let java_samples = java_output
        .split(|byte| *byte == b'\n')
        .skip(1)
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_slice::<OracleProbeSample>(line)
                .context("解析 Java oracle 探针样本失败")
        })
        .collect::<Result<Vec<_>>>()?;
    if rust_samples == java_samples {
        return Ok(());
    }
    let difference = rust_samples
        .iter()
        .zip(&java_samples)
        .position(|(rust, java)| rust != java)
        .unwrap_or(rust_samples.len().min(java_samples.len()));
    bail!(
        "Rust 与 Java oracle 探针不一致, 样本 {}, Rust={:?}, Java={:?}",
        difference + 1,
        rust_samples.get(difference),
        java_samples.get(difference)
    )
}

fn compare_oracle_samples_v2(rust_trace: &[u8], java_output: &[u8]) -> Result<()> {
    let rust_events = rust_trace
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<TraceEvent>(line).context("解析 Rust 轨迹失败"))
        .collect::<Result<Vec<_>>>()?;
    let rust_micro = rust_events
        .iter()
        .filter_map(|event| match event.kind {
            TraceKind::NeighborUpdate {
                pos,
                moved_by_piston,
                orientation,
                ..
            } => Some(OracleMicroSample::Neighbor(OracleNeighborSample {
                tick: event.tick.0,
                pos,
                moved_by_piston,
                orientation,
            })),
            TraceKind::ScheduledTickQueued {
                pos,
                trigger_tick,
                priority,
                sub_tick_order,
                ..
            } => Some(OracleMicroSample::ScheduledTickQueued(
                OracleScheduledTickQueuedSample {
                    tick: event.tick.0,
                    pos,
                    trigger_tick: trigger_tick.0,
                    priority,
                    sub_tick_order,
                },
            )),
            TraceKind::ScheduledTickExecuted { pos, .. } => Some(
                OracleMicroSample::ScheduledTickExecuted(OracleScheduledTickExecutedSample {
                    tick: event.tick.0,
                    pos,
                }),
            ),
            TraceKind::BlockEventQueued {
                pos,
                param_a,
                param_b,
                ..
            } => Some(OracleMicroSample::BlockEventQueued(
                OracleBlockEventSample {
                    tick: event.tick.0,
                    pos,
                    param_a,
                    param_b,
                },
            )),
            TraceKind::BlockEventExecuted {
                pos,
                param_a,
                param_b,
                ..
            } => Some(OracleMicroSample::BlockEventExecuted(
                OracleBlockEventSample {
                    tick: event.tick.0,
                    pos,
                    param_a,
                    param_b,
                },
            )),
            TraceKind::BlockChanged {
                pos,
                old_state,
                new_state,
                ..
            } => Some(OracleMicroSample::BlockChanged(OracleBlockChangeSample {
                tick: event.tick.0,
                pos,
                old_state: old_state.0,
                new_state: new_state.0,
            })),
            _ => None,
        })
        .collect::<Vec<_>>();
    let rust_probes = rust_events
        .iter()
        .filter_map(|event| match &event.kind {
            TraceKind::ProbeSample { sample } => Some(OracleProbeSample {
                tick: event.tick.0,
                probe: sample.name.clone(),
                value: probe_value_json(sample.value.clone()),
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let rust_neighbors = rust_events
        .iter()
        .filter_map(|event| match event.kind {
            TraceKind::NeighborUpdate {
                pos,
                moved_by_piston,
                orientation,
                ..
            } => Some(OracleNeighborSample {
                tick: event.tick.0,
                pos,
                moved_by_piston,
                orientation,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let rust_scheduled_queued = rust_events
        .iter()
        .filter_map(|event| match event.kind {
            TraceKind::ScheduledTickQueued {
                pos,
                trigger_tick,
                priority,
                sub_tick_order,
                ..
            } => Some(OracleScheduledTickQueuedSample {
                tick: event.tick.0,
                pos,
                trigger_tick: trigger_tick.0,
                priority,
                sub_tick_order,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let rust_scheduled_executed = rust_events
        .iter()
        .filter_map(|event| match event.kind {
            TraceKind::ScheduledTickExecuted { pos, .. } => {
                Some(OracleScheduledTickExecutedSample {
                    tick: event.tick.0,
                    pos,
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let rust_block_events_queued = rust_events
        .iter()
        .filter_map(|event| match event.kind {
            TraceKind::BlockEventQueued {
                pos,
                param_a,
                param_b,
                ..
            } => Some(OracleBlockEventSample {
                tick: event.tick.0,
                pos,
                param_a,
                param_b,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let rust_block_events_executed = rust_events
        .iter()
        .filter_map(|event| match event.kind {
            TraceKind::BlockEventExecuted {
                pos,
                param_a,
                param_b,
                ..
            } => Some(OracleBlockEventSample {
                tick: event.tick.0,
                pos,
                param_a,
                param_b,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let rust_block_changes = rust_events
        .iter()
        .filter_map(|event| match event.kind {
            TraceKind::BlockChanged {
                pos,
                old_state,
                new_state,
                ..
            } => Some(OracleBlockChangeSample {
                tick: event.tick.0,
                pos,
                old_state: old_state.0,
                new_state: new_state.0,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut java_probes = Vec::new();
    let mut java_neighbors = Vec::new();
    let mut java_scheduled_queued = Vec::new();
    let mut java_scheduled_executed = Vec::new();
    let mut java_block_events_queued = Vec::new();
    let mut java_block_events_executed = Vec::new();
    let mut java_block_changes = Vec::new();
    let mut java_micro = Vec::new();
    for line in java_output
        .split(|byte| *byte == b'\n')
        .skip(1)
        .filter(|line| !line.is_empty())
    {
        let value = serde_json::from_slice::<serde_json::Value>(line)
            .context("解析 Java oracle 样本失败")?;
        match value.get("kind").and_then(serde_json::Value::as_str) {
            Some("probe") => java_probes.push(
                serde_json::from_value::<OracleProbeSample>(value)
                    .context("解析 Java oracle 探针样本失败")?,
            ),
            Some("neighbor_update") => {
                let sample = serde_json::from_value::<OracleNeighborSample>(value)
                    .context("解析 Java oracle 邻居更新样本失败")?;
                java_micro.push(OracleMicroSample::Neighbor(sample.clone()));
                java_neighbors.push(sample);
            }
            Some("scheduled_tick_queued") => {
                let sample = serde_json::from_value::<OracleScheduledTickQueuedSample>(value)
                    .context("解析 Java oracle 计划刻入队样本失败")?;
                java_micro.push(OracleMicroSample::ScheduledTickQueued(sample.clone()));
                java_scheduled_queued.push(sample);
            }
            Some("scheduled_tick_executed") => {
                let sample = serde_json::from_value::<OracleScheduledTickExecutedSample>(value)
                    .context("解析 Java oracle 计划刻执行样本失败")?;
                java_micro.push(OracleMicroSample::ScheduledTickExecuted(sample.clone()));
                java_scheduled_executed.push(sample);
            }
            Some("block_event_queued") => {
                let sample = serde_json::from_value::<OracleBlockEventSample>(value)
                    .context("解析 Java oracle 方块事件入队样本失败")?;
                java_micro.push(OracleMicroSample::BlockEventQueued(sample.clone()));
                java_block_events_queued.push(sample);
            }
            Some("block_event_executed") => {
                let sample = serde_json::from_value::<OracleBlockEventSample>(value)
                    .context("解析 Java oracle 方块事件执行样本失败")?;
                java_micro.push(OracleMicroSample::BlockEventExecuted(sample.clone()));
                java_block_events_executed.push(sample);
            }
            Some("block_changed") => {
                let sample = serde_json::from_value::<OracleBlockChangeSample>(value)
                    .context("解析 Java oracle 方块状态写入样本失败")?;
                java_micro.push(OracleMicroSample::BlockChanged(sample.clone()));
                java_block_changes.push(sample);
            }
            other => bail!("Java oracle 返回未知样本类型: {other:?}"),
        }
    }
    compare_sample_vectors("探针", &rust_probes, &java_probes)?;
    compare_sample_vectors("全局微轨迹", &rust_micro, &java_micro)?;
    compare_sample_vectors("邻居更新", &rust_neighbors, &java_neighbors)?;
    compare_sample_vectors("计划刻入队", &rust_scheduled_queued, &java_scheduled_queued)?;
    compare_sample_vectors(
        "计划刻执行",
        &rust_scheduled_executed,
        &java_scheduled_executed,
    )?;
    compare_sample_vectors(
        "方块事件入队",
        &rust_block_events_queued,
        &java_block_events_queued,
    )?;
    compare_sample_vectors(
        "方块事件执行",
        &rust_block_events_executed,
        &java_block_events_executed,
    )?;
    compare_sample_vectors("方块状态写入", &rust_block_changes, &java_block_changes)
}

fn compare_sample_vectors<T>(kind: &str, rust: &[T], java: &[T]) -> Result<()>
where
    T: std::fmt::Debug + PartialEq,
{
    if rust == java {
        return Ok(());
    }
    let difference = rust
        .iter()
        .zip(java)
        .position(|(rust, java)| rust != java)
        .unwrap_or(rust.len().min(java.len()));
    let rust_start = difference.saturating_sub(4);
    let java_start = difference.saturating_sub(4);
    let rust_end = difference.saturating_add(8).min(rust.len());
    let java_end = difference.saturating_add(8).min(java.len());
    bail!(
        "Rust 与 Java oracle {kind}不一致, 样本 {}, Rust={:?}, Java={:?}, Rust 上下文={:?}, Java 上下文={:?}",
        difference + 1,
        rust.get(difference),
        java.get(difference),
        &rust[rust_start..rust_end],
        &java[java_start..java_end]
    )
}

fn probe_value_json(value: ProbeValue) -> serde_json::Value {
    match value {
        ProbeValue::Bool(value) => serde_json::Value::Bool(value),
        ProbeValue::Integer(value) => serde_json::Value::from(value),
        ProbeValue::String(value) => serde_json::Value::String(value),
        ProbeValue::State(value) => serde_json::Value::from(value.0),
        ProbeValue::None => serde_json::Value::Null,
    }
}

fn first_different_line(left: &[u8], right: &[u8]) -> usize {
    let common = left
        .iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count();
    left[..common].iter().filter(|byte| **byte == b'\n').count() + 1
}

fn stable_path_hash(path: &Path) -> u64 {
    path.to_string_lossy()
        .bytes()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
}

fn reject_newer_data_version(data_version: Option<i32>) -> Result<()> {
    if data_version.is_some_and(|version| version > JAVA_DATA_VERSION) {
        bail!(
            "结构 DataVersion {:?} 高于 Java {JAVA_VERSION} 的 {JAVA_DATA_VERSION}",
            data_version
        );
    }
    Ok(())
}

struct RegistryResolver(Java26Registry);

fn resolve_scenario_action(
    registry: &mut Java26Registry,
    action: &ScenarioActionKind,
) -> Result<Action> {
    Ok(match action {
        ScenarioActionKind::SetBlock {
            pos,
            name,
            properties,
        } => Action::SetBlock {
            pos: *pos,
            state: registry.resolve_state(name, properties)?,
        },
        ScenarioActionKind::BreakBlock { pos } => Action::BreakBlock { pos: *pos },
        ScenarioActionKind::UseBlock { pos } => Action::UseBlock { pos: *pos },
        ScenarioActionKind::PressButton { pos } => Action::PressButton { pos: *pos },
        ScenarioActionKind::PullLever { pos } => Action::PullLever { pos: *pos },
        ScenarioActionKind::SetBlockEntity { pos, data } => Action::SetBlockEntity {
            pos: *pos,
            data: data.clone(),
        },
        ScenarioActionKind::SpawnEntity {
            id,
            kind,
            position,
            fields,
        } => Action::SpawnEntity {
            id: *id,
            data: ScenarioActionKind::entity_data(kind.clone(), *position, fields.clone()),
        },
        ScenarioActionKind::MoveEntity { id, position } => Action::MoveEntity {
            id: *id,
            position: *position,
        },
        ScenarioActionKind::RemoveEntity { id } => Action::RemoveEntity { id: *id },
        ScenarioActionKind::SetEntityField { id, field, value } => Action::SetEntityField {
            id: *id,
            field: field.clone(),
            value: value.clone(),
        },
        ScenarioActionKind::HitTarget {
            pos,
            face,
            location,
            arrow,
        } => Action::HitTarget {
            pos: *pos,
            face: *face,
            location: *location,
            arrow: *arrow,
        },
    })
}

impl StructureStateResolver for RegistryResolver {
    type Error = StateResolveError;

    fn resolve_state(
        &mut self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<BlockStateId, Self::Error> {
        self.0.resolve_state(name, properties)
    }

    fn complete_state_properties(
        &mut self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, String>, Self::Error> {
        self.0.complete_state_properties(name, properties)
    }

    fn air_state(&self) -> BlockStateId {
        self.0.air_state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_diff_reports_the_first_changed_line() {
        assert_eq!(first_different_line(b"a\nb\n", b"a\nc\n"), 2);
        assert_eq!(first_different_line(b"a\n", b"a\nb\n"), 2);
    }

    #[test]
    fn scenario_hash_is_stable() {
        let path = Path::new("examples/basic.toml");
        assert_eq!(stable_path_hash(path), stable_path_hash(path));
    }

    #[test]
    fn probe_sample_format_compares_only_post_tick_samples() {
        let rust = br#"{"tick":1,"micro_step":1,"phase":"pre_tick","event":"message","level":"info","message":"ignored"}
{"tick":1,"micro_step":2,"phase":"post_tick","event":"probe_sample","sample":{"name":"power","probe":{"type":"property","pos":{"x":0,"y":0,"z":0},"property":"power"},"value":"15"}}
"#;
        let java = br#"{"format":"probe_samples_v1"}
{"tick":1,"probe":"power","value":"15"}
"#;
        assert!(is_probe_sample_oracle(java).unwrap());
        compare_probe_samples(rust, java).unwrap();
    }

    #[test]
    fn oracle_samples_v2_compares_probes_and_neighbor_order() {
        let rust = br#"{"tick":1,"micro_step":1,"phase":"pre_tick","event":"neighbor_update","pos":{"x":-1,"y":0,"z":0},"source_pos":{"x":0,"y":0,"z":0},"source_block":1,"moved_by_piston":false,"orientation":null}
{"tick":1,"micro_step":2,"phase":"pre_tick","event":"scheduled_tick_queued","pos":{"x":0,"y":0,"z":0},"block":1,"trigger_tick":9,"priority":0,"sub_tick_order":0}
{"tick":1,"micro_step":3,"phase":"pre_tick","event":"block_event_queued","pos":{"x":1,"y":0,"z":0},"block":2,"param_a":0,"param_b":5}
{"tick":1,"micro_step":4,"phase":"block_events","event":"block_event_executed","pos":{"x":1,"y":0,"z":0},"block":2,"param_a":0,"param_b":5}
{"tick":1,"micro_step":5,"phase":"block_events","event":"block_changed","pos":{"x":1,"y":0,"z":0},"old_state":4,"new_state":5,"cause":"test"}
{"tick":9,"micro_step":1,"phase":"scheduled_ticks","event":"scheduled_tick_executed","pos":{"x":0,"y":0,"z":0},"block":1}
{"tick":9,"micro_step":2,"phase":"post_tick","event":"probe_sample","sample":{"name":"power","probe":{"type":"property","pos":{"x":0,"y":0,"z":0},"property":"power"},"value":"0"}}
"#;
        let java = br#"{"format":"oracle_samples_v2"}
{"tick":9,"probe":"power","value":"0","kind":"probe"}
{"kind":"neighbor_update","tick":1,"pos":{"x":-1,"y":0,"z":0},"moved_by_piston":false,"orientation":null}
{"kind":"scheduled_tick_queued","tick":1,"pos":{"x":0,"y":0,"z":0},"trigger_tick":9,"priority":0,"sub_tick_order":0}
{"kind":"block_event_queued","tick":1,"pos":{"x":1,"y":0,"z":0},"block":"minecraft:piston","param_a":0,"param_b":5}
{"kind":"block_event_executed","tick":1,"pos":{"x":1,"y":0,"z":0},"block":"minecraft:piston","param_a":0,"param_b":5}
{"kind":"block_changed","tick":1,"pos":{"x":1,"y":0,"z":0},"old_state":4,"new_state":5}
{"kind":"scheduled_tick_executed","tick":9,"pos":{"x":0,"y":0,"z":0}}
"#;

        assert!(is_oracle_samples_v2(java).unwrap());
        compare_oracle_samples_v2(rust, java).unwrap();
    }
}
