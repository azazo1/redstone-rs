# redstone-rs

`redstone-rs` 是面向大型 Java 版红石机器的确定性 Rust 仿真器. 当前兼容目标固定为 Java `26.1.2`.

项目提供可嵌入的异步核心库和 `redstone` CLI. 正式运行不依赖 JVM. Java oracle 只用于开发和 CI 差分测试.

## Workspace

- `redstone-core`: 稀疏世界, 事件调度, 邻居更新, 探针, delta 和轨迹.
- `redstone-java-26`: Java `26.1.2` 状态注册表和红石规则.
- `redstone-io`: Litematic, 原版 structure NBT 和 TOML 场景.
- `redstone-replay-26`: Replay Mod MCPR 容器和 Java `26.1.2` 网络协议编码.
- `redstone-cli`: `inspect`, `run`, `test` 和 `trace` 命令.
- `tools/vanilla-oracle`: 外部 Java 参考探针的调用协议.

## 已实现

- 16x16x16 稀疏调色板区段, 纯空气区段不分配.
- 计划方块刻优先级, 去重, 快照执行和 `sub_tick_order`.
- Java 式嵌套邻居更新和固定方向顺序.
- 默认及实验性红石线, 48 种 `Orientation` 和 Java 兼容随机源.
- 火把, 中继器, 比较器, 观察者, BUD/QC 基础行为.
- 活塞, 黏性活塞, 12 方块限制, 黏液/蜂蜜分支, 取消, 零刻和移动方块实体.
- 灯, 铜灯, 门, 动力铁轨, 音符盒, 钟, 目标方块和 TNT 触发轨迹.
- 漏斗, 投掷器, 发射器投射物/矿车/TNT 显式行为和合成器最小模型.
- 熔炉和酿造台侧面槽规则, 合成器禁用槽和潜影盒嵌套限制.
- 压力板, 绊线, 探测铁轨, 讲台, 阳光探测器和陷阱箱最小模型.
- 物品实体寿命, 通用碰撞实体, 物品展示框和容器/漏斗矿车最小模型.
- 红石线, 铁轨, 绊线, 栅栏, 玻璃板, 栏杆和墙的结构形状修复.
- gzip/非 gzip NBT, Litematic 多区域, 负尺寸区域, 旋转和镜像.
- JSONL 微时序轨迹, VCD 波形和场景级并行测试.
- Rust 仿真初始世界和逐 tick 方块变化的 Replay Mod `.mcpr` 导出.
- TOML 实体生成, 移动, 删除, 字段修改和目标方块命中动作.

## 构建和测试

```shell
just generate-reports
just build
just clippy
just test
```

> 注: 如果 build 失败, 请删除 `assets/libraries` 文件夹之后重新执行 `just generate-reports && just build`.

也可以直接运行 Cargo 命令:

```shell
cargo run -p redstone-cli -- inspect machine.litematic
cargo run -p redstone-cli -- inspect machine.litematic --block 10,20,30
cargo run -p redstone-cli -- inspect machine.litematic --block 0..=10,5,0.. --type minecraft:hopper
cargo run -p redstone-cli -- inspect machine.litematic --type minecraft:hopper --format json
cargo run -p redstone-cli -- inspect machine.litematic --all --json
cargo run -p redstone-cli -- run scenario.toml --trace trace.jsonl --vcd signals.vcd
cargo run -p redstone-cli -- run scenario.toml --replay scenario.mcpr
cargo run -p redstone-cli -- run scenario.toml --replay scenario.mcpr --replay-anim
cargo run -p redstone-cli -- test scenario.toml --replay scenario.mcpr
cargo run -p redstone-cli -- test scenarios
cargo run --release -p redstone-cli -- bench
```

`inspect` 默认输出结构汇总. `--block X,Y,Z` 可重复查询指定坐标, 每个坐标轴也支持 `a..b`, `a..=b`, `..b`, `..=b`, `a..` 和 `..` 范围. 范围只返回非空气方块, 并可叠加 `--type BLOCK_ID` 按方块类型筛选. `--all` 输出全部非空气方块. 明细包含状态 ID, properties, 支持状态和完整方块实体 NBT, 包括嵌套的 `components`. `--format json` 与 `--json` 均可输出 JSON.

`run --replay` 和单场景 `test --replay` 会直接从 Rust 仿真生成 Replay Mod 格式 14 录像. 录像目标版本为 Minecraft `26.1.2`, 每个游戏 tick 对应 50 ms. 初始原理图声明区域覆盖的全部 chunk 会在同一批次加载, 外围保留一圈渲染邻居. 后续方块变化进入新 chunk 时只加载目标 chunk 的局部邻接圈, 可随飞行器移动持续扩展, 不会补齐与原理图之间的无关 chunk. 目录批量测试暂不支持共享录像输出路径. 断言失败时录像仍会完成并保留, 仿真或编码失败时不会替换目标文件.

场景可以设置 Replay 初始摄像头:

```toml
[replay.camera]
view_distance = 8
position = [12.5, 20.0, -6.5]
yaw = 135.0
pitch = 35.0
```

所有字段均可省略. 自动取景使用录像开始时的非空气方块边界, 优先靠近机器, 并在结构明显扁平时沿最薄轴观察. 场景探针, 红石灯和铜灯会用于选择更接近观测结果的一面. `view_distance` 约束自动机位的 Replay 播放视距预算, 不裁剪录像中的远端 chunk 数据. 完整字段和角度约定见 [场景编写手册](docs/scenario-guide.md#replay-初始摄像头).

`--replay-anim` 会在录像中额外保留原版活塞 block event, 由客户端生成伸缩动画和声音. 该开关必须与 `--replay` 同时使用, 默认关闭.

> 注: Replay Mod 版本: replaymod-26.1-2.6.26 fabric

## 场景格式

场景固定指定版本, 红石模式, 随机种子, 结构来源, 初始化方式, 动作, 探针和断言. 动作在目标游戏刻的 `pre_tick` 阶段按声明顺序执行, 断言读取 `post_tick` 探针值.

```toml
version = "26.1.2"
mode = "default"
seed = 42
max_ticks = 20
strict = true

[source]
path = "machine.litematic"
initialization = "notify"
rotation = "none"
mirror = "none"

[[actions]]
tick = 1
type = "pull_lever"
pos = { x = 0, y = 0, z = 0 }

[[probes]]
name = "output"
type = "signal"
pos = { x = 10, y = 0, z = 0 }

[[expectations]]
tick = 2
probe = "output"
equals = 15
```

严格模式会拒绝已识别但未实现的主动方块. `--allow-static-fallback` 可以保留其静态状态并继续执行.

## Java oracle

`redstone test --oracle` 通过 `tools/vanilla-oracle/run.sh` 或 `REDSTONE_ORACLE` 指定的程序调用 Java 参考探针. 探针接收场景路径和输出 JSONL 路径. 完整轨迹使用字节级比较, `probe_samples_v1` 输出使用逐 tick 探针比较.

`just oracle-build` 会下载测试专用 TOML 解析依赖并构建 Java `26.1.2` 探针 JAR. `just oracle-scenario-self-test` 会运行真实 structure, 动作和探针场景. `VANILLA_ORACLE_JAR` 可以覆盖默认 JAR. 发布的 Rust 库和 CLI 不需要 Java.

当前 GameTest oracle 支持原版 structure NBT, `raw`/`notify` 初始化, 非零原点, 旋转/镜像, 任意场景 seed, 默认/实验性红石模式, 基础方块与实体动作, 方块与实体探针. 测试场景可启用 ASM 邻居更新, 计划方块刻, 方块事件及状态写入采样. CLI 会差分探针和全局微轨迹, 包括同步嵌套顺序, `Orientation`, `moved_by_piston`, 计划刻优先级, `sub_tick_order`, 方块事件参数和官方全局 state ID. 当前 18 个真实 Java 场景达到零状态差异和零事件顺序差异. 形状更新的完整 ASM 微轨迹仍待完成.

`redstone bench` 默认构建 100 万已放置方块和 1 万活跃元件, 运行 100 个空闲刻并输出 P50/P95/P99 与可获取的常驻内存.

## 当前限制

目前仍处于行为覆盖和 oracle 差分阶段. 多方块活塞分支及破坏反应顺序, 铁轨支撑破坏和矿车物理, 墙的精确碰撞判定, 发射器剩余物品行为, 全量物品标签和完整合成配方尚未达到稳定标准. TNT 引信和爆炸触发会进入轨迹, 但爆炸, 火传播和流体不修改世界.

## 部分原理图来源

- [cpu-8bit.litematic](assets/schematics/cpu-8bit.litematic): <https://www.planetminecraft.com/project/new-redstone-computer/>
- [dvdprogram.schem](assets/schematics/dvdprogram.schem): <https://github.com/mattbatwings/BatPU-2>
- [digital-clock.litematic](assets/schematics/digital-clock.litematic): <https://www.planetminecraft.com/project/digital-clock-v2/>
- [head.litematic](assets/schematics/head.litematic): <https://www.planetminecraft.com/project/redstone-automaton-5932751/>
