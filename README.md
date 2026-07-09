# redstone-rs

`redstone-rs` 是面向大型 Java 版红石机器的确定性 Rust 仿真器. 当前兼容目标固定为 Java `26.1.2`.

项目提供可嵌入的异步核心库和 `redstone` CLI. 正式运行不依赖 JVM. Java oracle 只用于开发和 CI 差分测试.

## Workspace

- `redstone-core`: 稀疏世界, 事件调度, 邻居更新, 探针, delta 和轨迹.
- `redstone-java-26`: Java `26.1.2` 状态注册表和红石规则.
- `redstone-io`: Litematic, 原版 structure NBT 和 TOML 场景.
- `redstone-cli`: `inspect`, `run`, `test` 和 `trace` 命令.
- `tools/vanilla-oracle`: 外部 Java 参考探针的调用协议.

## 已实现

- 16x16x16 稀疏调色板区段, 纯空气区段不分配.
- 计划方块刻优先级, 去重, 快照执行和 `sub_tick_order`.
- Java 式嵌套邻居更新和固定方向顺序.
- 默认及实验性红石线, 48 种 `Orientation` 和 Java 兼容随机源.
- 火把, 中继器, 比较器, 观察者, BUD/QC 基础行为.
- 活塞, 黏性活塞, 12 方块限制, 黏液/蜂蜜分支和移动占位.
- 灯, 铜灯, 常见受控方块和 TNT 触发轨迹.
- 漏斗, 投掷器, 发射器显式物品行为表和合成器最小模型.
- 压力板, 绊线, 探测铁轨, 讲台, 阳光探测器和陷阱箱最小模型.
- 物品实体寿命, 碰撞实体和矿车传感器模型.
- gzip/非 gzip NBT, Litematic 多区域, 负尺寸区域, 旋转和镜像.
- JSONL 微时序轨迹, VCD 波形和场景级并行测试.

## 构建和测试

```shell
just build
just clippy
just test
```

也可以直接运行 Cargo 命令:

```shell
cargo run -p redstone-cli -- inspect machine.litematic
cargo run -p redstone-cli -- run scenario.toml --trace trace.jsonl --vcd signals.vcd
cargo run -p redstone-cli -- test scenarios
cargo run --release -p redstone-cli -- bench
```

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

`redstone test --oracle` 通过 `tools/vanilla-oracle/run.sh` 或 `REDSTONE_ORACLE` 指定的程序调用 Java 参考探针. 探针接收场景路径和输出 JSONL 路径, CLI 会对 Rust 与 Java 轨迹做字节级比较.

Java `26.1.2` 的 GameTest/ASM 探针 JAR 需要单独构建并通过 `VANILLA_ORACLE_JAR` 提供. 发布的 Rust 库和 CLI 不需要该 JAR.

`redstone bench` 默认构建 100 万已放置方块和 1 万活跃元件, 运行 100 个空闲刻并输出 P50/P95/P99 与可获取的常驻内存.

## 当前限制

目前仍处于行为覆盖和 oracle 差分阶段. 活塞零刻细节, 完整方块形状更新, 发射器全部物品行为, 完整容器槽规则和部分主动方块尚未达到稳定标准. 爆炸, 火传播和流体只记录触发, 不修改世界.
