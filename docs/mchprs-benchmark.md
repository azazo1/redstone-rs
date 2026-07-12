# MCHPRS Frostbyte 基准适配器

本文说明 MCHPRS Frostbyte 16-bit CPU 基准的构建和执行方式. 完整适配器源码位于 [mchprs-benchmark.rs](mchprs-benchmark.rs), README 不重复嵌入源码.

## 固定版本

- MCHPRS: commit [`8734f72bcf48be492c39e657d549e054255bed31`](https://github.com/MCHPR/MCHPRS/commit/8734f72bcf48be492c39e657d549e054255bed31).
- Redpiler 参数: `optimize=true`, `io_only=true`.
- 测试使用一个 headless `TestWorld`, 不启动客户端连接, plot 调度或网络发送.

## 构建适配器

先从项目根目录生成只包含 Frostbyte 测试区域的 Sponge schematic:

```shell
target/release/redstone convert \
  assets/worlds/CPU_22_08_2025.zip \
  frostbyte-base.schem \
  --region 9,52,-1 287,157,397
```

将 [mchprs-benchmark.rs](mchprs-benchmark.rs) 放到 MCHPRS checkout 的 `examples/frostbyte_bench.rs`. MCHPRS 当前没有从 `mchprs_core` 公开 schematic crate, 因此测试 checkout 还需要在 `crates/core/src/lib.rs` 增加下面一行:

```rust
pub use mchprs_schematic;
```

随后在 MCHPRS checkout 中构建:

```shell
cargo build --release --example frostbyte_bench
```

这个适配只用于 headless 基准, 不属于 MCHPRS 的公共接口修改.

## 坐标与 tick 换算

- 基础世界平移为 `(-9,-52,+1)`.
- 程序原点从 `(64,152,389)` 平移到 `(55,100,390)`.
- 运行按钮从 `(274,88,197)` 平移到 `(265,36,198)`.
- MCHPRS 的 1 redstone tick 对应 2 game tick. 场景在 game tick 100 按按钮, 适配器在 redstone tick 50 执行动作.
- `game_ticks_per_second` 使用完整 game tick 数除以 MCHPRS redstone tick 阶段时间, 因此已经完成 2 倍换算.

## 输入归一化

MCHPRS schematic parser 无法表示测试区域中的 1 个 chest, 1 个 lectern 和 9 个孤立 moving piston. 解析时这 11 个方块被映射为空气. 它们分别属于展示性容器或未连接到活动数据通路的遗留移动方块, 最终屏幕验证也确认两个程序未依赖它们.

除此以外不替换任何活动元件. 如果后续输入出现新的不支持元件, 该场景应标记为不可比较, 而不是继续改写结构语义.

## 正确性和稳定点

适配器只接受场景最终 tick 的 `lit` 断言. `hello-world` 校验 206 个屏幕灯, `line-drawing` 校验 36 个灯. 任一探针不匹配都会返回失败, 该轮结果不能进入速度表.

普通引擎以计划刻队列为空作为空闲条件, Redpiler 使用 `has_pending_ticks()`. 动作发生并观察到活动后, 连续 20 个 redstone tick 空闲才确认稳定. 输出的 `stable_game_tick` 是候选稳定 redstone tick 的 2 倍, `active_game_ticks_per_second` 只统计该点之前的 tick 阶段时间.

MCHPRS 的 redstone tick 模型和本项目的逐 game tick 模型并不完全相同. 本次结果中 MCHPRS 的稳定点比 `redstone-rs` 早 2 game tick, 因此稳定前速度可以用于观察活动区间成本, 但稳定点本身不能解释为逐事件完全等价.

## 执行命令

普通引擎:

```shell
path/to/MCHPRS/target/release/examples/frostbyte_bench \
  --base frostbyte-base.schem \
  --program assets/schematics/frostbyte-hello-world.schem \
  --scenario assets/scenarios/frostbyte-cpu-16bit-hello-world.toml \
  --backend redstone
```

Redpiler:

```shell
path/to/MCHPRS/target/release/examples/frostbyte_bench \
  --base frostbyte-base.schem \
  --program assets/schematics/frostbyte-line-drawing.schem \
  --scenario assets/scenarios/frostbyte-cpu-16bit-line-drawing.toml \
  --backend redpiler
```

将 `--program` 和 `--scenario` 成对替换即可运行另一个程序. 每个后端和场景都应启动独立进程运行 3 次. Redpiler 的 `compile_ms` 单列, 不计入 `tick_ms` 或 tick/s.
