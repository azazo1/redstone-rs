use clap::{Parser, ValueEnum};
use mchprs_blocks::blocks::Block;
use mchprs_blocks::BlockPos;
use mchprs_core::mchprs_schematic::{load_schematic, paste_clipboard, WorldEditClipboard};
use mchprs_redpiler::{Compiler, CompilerOptions, TaskMonitor};
use mchprs_world::testing::TestWorld;
use mchprs_world::{for_each_block_mut_optimized, World};
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

const BASE_PLACEMENT: BlockPos = BlockPos::new(-9, -52, 1);
const BUTTON_POS: BlockPos = BlockPos::new(274 - 9, 88 - 52, 197 + 1);
const STABILITY_CONFIRMATION_TICKS: u64 = 20;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Backend {
    Redstone,
    Redpiler,
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    base: PathBuf,
    #[arg(long)]
    program: PathBuf,
    #[arg(long)]
    scenario: PathBuf,
    #[arg(long, value_enum)]
    backend: Backend,
    #[arg(long, default_value_t = 5_000)]
    progress_interval: u64,
}

#[derive(Debug)]
struct Probe {
    position: BlockPos,
}

#[derive(Debug)]
struct ExpectedProbe {
    name: String,
    position: BlockPos,
    lit: bool,
}

#[derive(Clone, Copy, Debug)]
struct StabilityPoint {
    redstone_tick: u64,
    elapsed: Duration,
}

#[derive(Debug)]
struct RunTiming {
    elapsed: Duration,
    stability: Option<StabilityPoint>,
}

#[derive(Default)]
struct StabilityTracker {
    activity_seen: bool,
    candidate: Option<StabilityPoint>,
    idle_ticks: u64,
    stable: Option<StabilityPoint>,
}

impl StabilityTracker {
    fn observe(&mut self, tick: u64, elapsed: Duration, idle: bool) {
        if !idle {
            self.activity_seen = true;
            self.candidate = None;
            self.idle_ticks = 0;
            self.stable = None;
            return;
        }
        if !self.activity_seen {
            return;
        }
        if self.candidate.is_none() {
            self.candidate = Some(StabilityPoint {
                redstone_tick: tick,
                elapsed,
            });
        }
        self.idle_ticks += 1;
        if self.idle_ticks >= STABILITY_CONFIRMATION_TICKS {
            self.stable = self.candidate;
        }
    }
}

#[derive(Default)]
struct ProbeBuilder {
    name: Option<String>,
    position: Option<BlockPos>,
    property: Option<String>,
}

#[derive(Default)]
struct ExpectationBuilder {
    tick: Option<u64>,
    probe: Option<String>,
    equals: Option<String>,
}

fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .init();
    let args = Args::parse();
    let total_started = Instant::now();
    let (max_game_ticks, expected) = load_expectations(&args.scenario)?;
    if max_game_ticks % 2 != 0 {
        return Err("MCHPRS 需要偶数 game tick 数".into());
    }
    let redstone_ticks = max_game_ticks / 2;

    info!(path = %args.base.display(), "加载 Frostbyte CPU");
    let load_started = Instant::now();
    let base = load_schematic(&args.base)?;
    let program = load_schematic(&args.program)?;
    let mut world = TestWorld::new(18, 7, 25);
    paste_clipboard(&mut world, &base, BASE_PLACEMENT, false);
    let program_origin = program_origin(&args.program)?;
    paste_clipboard(&mut world, &program, program_origin, true);
    update_clipboard_region(&mut world, &program, program_origin);
    let load_elapsed = load_started.elapsed();
    info!(?load_elapsed, "完成 CPU 和程序装载");

    let (timing, compile_elapsed) = match args.backend {
        Backend::Redstone => (
            run_redstone(&mut world, redstone_ticks, args.progress_interval),
            None,
        ),
        Backend::Redpiler => {
            let compile_started = Instant::now();
            let mut compiler = Compiler::default();
            let options = CompilerOptions {
                optimize: true,
                io_only: true,
                ..CompilerOptions::default()
            };
            let ticks = world.to_be_ticked.clone();
            compiler.compile(
                &world,
                (BlockPos::new(0, 0, 0), BlockPos::new(287, 105, 398)),
                options,
                ticks,
                Arc::new(TaskMonitor::default()),
            );
            let compile_elapsed = compile_started.elapsed();
            info!(?compile_elapsed, "完成 Redpiler 编译");
            (
                run_redpiler(
                    &mut compiler,
                    &mut world,
                    redstone_ticks,
                    args.progress_interval,
                ),
                Some(compile_elapsed),
            )
        }
    };

    verify_expectations(&world, &expected)?;
    let total_elapsed = total_started.elapsed();
    let game_ticks_per_second = max_game_ticks as f64 / timing.elapsed.as_secs_f64();
    println!("backend: {:?}", args.backend);
    println!("game_ticks: {max_game_ticks}");
    println!("redstone_ticks: {redstone_ticks}");
    println!("load_ms: {:.3}", load_elapsed.as_secs_f64() * 1_000.0);
    if let Some(elapsed) = compile_elapsed {
        println!("compile_ms: {:.3}", elapsed.as_secs_f64() * 1_000.0);
    }
    println!("tick_ms: {:.3}", timing.elapsed.as_secs_f64() * 1_000.0);
    println!("game_ticks_per_second: {game_ticks_per_second:.3}");
    if let Some(stability) = timing.stability {
        let stable_game_tick = stability.redstone_tick * 2;
        println!("stable_game_tick: {stable_game_tick}");
        println!(
            "active_tick_ms: {:.3}",
            stability.elapsed.as_secs_f64() * 1_000.0
        );
        println!(
            "active_game_ticks_per_second: {:.3}",
            stable_game_tick as f64 / stability.elapsed.as_secs_f64()
        );
    } else {
        println!("stable_game_tick: not_reached");
    }
    println!("total_ms: {:.3}", total_elapsed.as_secs_f64() * 1_000.0);
    println!("verified_probes: {}", expected.len());
    Ok(())
}

fn program_origin(path: &Path) -> Result<BlockPos, Box<dyn Error>> {
    let name = path.file_name().and_then(|value| value.to_str()).unwrap_or_default();
    let original = match name {
        "frostbyte-hello-world.schem" | "frostbyte-line-drawing.schem" => {
            BlockPos::new(64, 152, 389)
        }
        _ => return Err(format!("未知 Frostbyte 程序: {name}").into()),
    };
    Ok(original + BASE_PLACEMENT)
}

fn update_clipboard_region(world: &mut TestWorld, cb: &WorldEditClipboard, pos: BlockPos) {
    let first = BlockPos::new(pos.x - cb.offset_x, pos.y - cb.offset_y, pos.z - cb.offset_z);
    let second = BlockPos::new(
        first.x + cb.size_x as i32 - 1,
        first.y + cb.size_y as i32 - 1,
        first.z + cb.size_z as i32 - 1,
    );
    for_each_block_mut_optimized(world, first, second, |world, position| {
        mchprs_redstone::update(world.get_block(position), world, position);
    });
}

fn run_redstone(world: &mut TestWorld, ticks: u64, progress_interval: u64) -> RunTiming {
    let started = Instant::now();
    let mut stability = StabilityTracker::default();
    for tick in 1..=ticks {
        if tick == 50 {
            mchprs_redstone::on_use(world.get_block(BUTTON_POS), world, BUTTON_POS);
        }
        world
            .to_be_ticked
            .sort_by_key(|entry| (entry.ticks_left, entry.tick_priority));
        for pending in &mut world.to_be_ticked {
            pending.ticks_left = pending.ticks_left.saturating_sub(1);
        }
        while world.to_be_ticked.first().map_or(1, |entry| entry.ticks_left) == 0 {
            let entry = world.to_be_ticked.remove(0);
            mchprs_redstone::tick(world.get_block(entry.pos), world, entry.pos);
        }
        stability.observe(tick, started.elapsed(), world.to_be_ticked.is_empty());
        report_progress(tick, ticks, progress_interval, started.elapsed());
    }
    RunTiming {
        elapsed: started.elapsed(),
        stability: stability.stable,
    }
}

fn run_redpiler(
    compiler: &mut Compiler,
    world: &mut TestWorld,
    ticks: u64,
    progress_interval: u64,
) -> RunTiming {
    let started = Instant::now();
    let mut stability = StabilityTracker::default();
    for tick in 1..=ticks {
        if tick == 50 {
            compiler.on_use_block(BUTTON_POS);
        }
        compiler.tick();
        compiler.flush(world);
        let idle = !compiler.has_pending_ticks();
        stability.observe(tick, started.elapsed(), idle);
        report_progress(tick, ticks, progress_interval, started.elapsed());
    }
    RunTiming {
        elapsed: started.elapsed(),
        stability: stability.stable,
    }
}

fn report_progress(tick: u64, total: u64, interval: u64, elapsed: Duration) {
    if tick == total || tick % interval == 0 {
        info!(tick, total, ?elapsed, "Frostbyte 基准进度");
    }
}

fn verify_expectations(
    world: &TestWorld,
    expected: &[ExpectedProbe],
) -> Result<(), Box<dyn Error>> {
    let mut mismatches = Vec::new();
    for probe in expected {
        let shifted = probe.position + BASE_PLACEMENT;
        let actual = match world.get_block(shifted) {
            Block::RedstoneLamp { lit } => lit,
            block => {
                mismatches.push(format!("{} 不是红石灯: {:?}", probe.name, block));
                continue;
            }
        };
        if actual != probe.lit {
            mismatches.push(format!(
                "{} 期望 lit={}, 实际 lit={}",
                probe.name, probe.lit, actual
            ));
        }
    }
    if mismatches.is_empty() {
        info!(count = expected.len(), "最终屏幕 probe 全部匹配");
        return Ok(());
    }
    for mismatch in mismatches.iter().take(20) {
        eprintln!("probe_mismatch: {mismatch}");
    }
    Err(format!("{} 个最终 probe 不匹配", mismatches.len()).into())
}

fn load_expectations(path: &Path) -> Result<(u64, Vec<ExpectedProbe>), Box<dyn Error>> {
    let source = fs::read_to_string(path)?;
    let mut max_ticks = None;
    let mut section = "";
    let mut probe_builder = ProbeBuilder::default();
    let mut expectation_builder = ExpectationBuilder::default();
    let mut probes = HashMap::<String, Probe>::new();
    let mut expectations = Vec::<ExpectationBuilder>::new();

    for raw_line in source.lines() {
        let line = raw_line.trim();
        if line == "[[probes]]" {
            commit_probe(&mut probes, &mut probe_builder)?;
            commit_expectation(&mut expectations, &mut expectation_builder);
            section = "probe";
            continue;
        }
        if line == "[[expectations]]" {
            commit_probe(&mut probes, &mut probe_builder)?;
            commit_expectation(&mut expectations, &mut expectation_builder);
            section = "expectation";
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key == "max_ticks" {
            max_ticks = Some(value.parse()?);
        }
        match section {
            "probe" => match key {
                "name" => probe_builder.name = Some(parse_string(value)?),
                "pos" => probe_builder.position = Some(parse_position(value)?),
                "property" => probe_builder.property = Some(parse_string(value)?),
                _ => {}
            },
            "expectation" => match key {
                "tick" => expectation_builder.tick = Some(value.parse()?),
                "probe" => expectation_builder.probe = Some(parse_string(value)?),
                "equals" => expectation_builder.equals = Some(parse_string(value)?),
                _ => {}
            },
            _ => {}
        }
    }
    commit_probe(&mut probes, &mut probe_builder)?;
    commit_expectation(&mut expectations, &mut expectation_builder);

    let max_ticks = max_ticks.ok_or("场景缺少 max_ticks")?;
    let mut result = Vec::new();
    for expectation in expectations {
        if expectation.tick != Some(max_ticks) {
            continue;
        }
        let name = expectation.probe.ok_or("expectation 缺少 probe")?;
        let probe = probes.get(&name).ok_or_else(|| format!("未知 probe: {name}"))?;
        let equals = expectation.equals.ok_or("expectation 缺少 equals")?;
        result.push(ExpectedProbe {
            name,
            position: probe.position,
            lit: equals.parse()?,
        });
    }
    if result.is_empty() {
        return Err("场景没有最终 tick 断言".into());
    }
    Ok((max_ticks, result))
}

fn commit_probe(
    probes: &mut HashMap<String, Probe>,
    builder: &mut ProbeBuilder,
) -> Result<(), Box<dyn Error>> {
    let Some(name) = builder.name.take() else {
        return Ok(());
    };
    let position = builder.position.take().ok_or("probe 缺少 pos")?;
    let property = builder.property.take().ok_or("probe 缺少 property")?;
    if property == "lit" {
        probes.insert(name, Probe { position });
    }
    Ok(())
}

fn commit_expectation(
    expectations: &mut Vec<ExpectationBuilder>,
    builder: &mut ExpectationBuilder,
) {
    if builder.tick.is_some() || builder.probe.is_some() || builder.equals.is_some() {
        expectations.push(std::mem::take(builder));
    }
}

fn parse_string(value: &str) -> Result<String, Box<dyn Error>> {
    let value = value.trim();
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return Err(format!("无效字符串: {value}").into());
    }
    Ok(value[1..value.len() - 1].to_owned())
}

fn parse_position(value: &str) -> Result<BlockPos, Box<dyn Error>> {
    let value = value
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .ok_or("无效位置")?;
    let mut values = HashMap::new();
    for part in value.split(',') {
        let (key, value) = part.split_once('=').ok_or("无效位置字段")?;
        values.insert(key.trim(), value.trim().parse::<i32>()?);
    }
    Ok(BlockPos::new(
        *values.get("x").ok_or("位置缺少 x")?,
        *values.get("y").ok_or("位置缺少 y")?,
        *values.get("z").ok_or("位置缺少 z")?,
    ))
}
