# redstone-rs

`redstone-rs` 是面向大型 Java 版红石机器的确定性 Rust 仿真器. 当前兼容目标固定为 Java `26.1.2`.

项目提供可嵌入的异步核心库和 `redstone` CLI. 正式运行不依赖 JVM. Java oracle 只用于开发和 CI 差分测试.

## Workspace

- `redstone-core`: 稀疏世界, 事件调度, 邻居更新, 探针, delta 和轨迹.
- `redstone-java-26`: Java `26.1.2` 状态注册表, 红石规则和混合编译执行器.
- `redstone-io`: Litematic, 原版 structure NBT 和 TOML 场景.
- `redstone-replay-26`: Replay Mod MCPR 容器和 Java `26.1.2` 网络协议编码.
- `redstone-cli`: `inspect`, `run`, `test`, `trace`, `bench` 和 `convert` 命令.
- `tools/vanilla-oracle`: 外部 Java 参考探针的调用协议.

## 文档

- [已实现功能](docs/impl.md): 仿真内核, Java 规则, IO, CLI, Replay 和 oracle 的实际覆盖范围.
- [场景编写手册](docs/scenario-guide.md): TOML 场景, 动作, 探针, 断言, Replay 和运行方式.
- [实现状态](docs/todo.md): 已完成能力, 稳定前任务和暂不支持范围.
- [性能优化记录](docs/perf.md): 基准, profile 证据和优化结果.
- [MCHPRS 基准适配器](docs/mchprs-benchmark.md): headless 适配口径, 构建方式和独立 Rust 源码.
- [Java oracle 协议](tools/vanilla-oracle/README.md): 参考探针的构建, 输出协议和支持范围.

## Frostbyte 16-bit CPU 性能

下表使用 Apple M1 8 核, 16 GB 内存在 2026-07-12 测量. 每项独立运行 3 次且保留全部结果, 表中为中位数. `总 tick/s` 包含机器稳定后的空闲尾部, `稳定前 tick/s` 只统计首次确认稳定之前的活动区间. MCHPRS 已换算为等价 game tick/s. 详细口径和原始数据见 [性能优化记录](docs/perf.md#跨实现-frostbyte-基准).

### Hello World, 8800 game tick

| 实现 | 总 tick/s | 稳定前 tick/s | 稳定 tick | tick 阶段 | wall | 峰值 RSS | 正确性 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `redstone-rs` compiled | 10922.122 | 10785.317 | 8688 | 0.806 s | 5.10 s | 1469.9 MiB | 206 个灯通过 |
| `redstone-rs` interpreted | 2828.238 | 2792.337 | 8688 | 3.111 s | 3.77 s | 103.2 MiB | 206 个灯通过 |
| [`redstone-rs` interpreted `7f8344e7`, 历史](https://github.com/azazo1/redstone-rs/commit/7f8344e70ed78c38e535db7eeaf9851d73ed45fc) | 2459.498 | 2428.251 | 8688 | 3.578 s | 4.19 s | 93.3 MiB | 206 个灯通过 |
| [MCHPRS 普通引擎 `8734f72b`](https://github.com/MCHPR/MCHPRS/commit/8734f72bcf48be492c39e657d549e054255bed31) | 1917.223 | 1892.394 | 8686 | 4.590 s | 5.37 s | 128.6 MiB | 206 个灯通过 |
| [MCHPRS Redpiler `8734f72b`](https://github.com/MCHPR/MCHPRS/commit/8734f72bcf48be492c39e657d549e054255bed31) | 10220.672 | 10223.299 | 8686 | 0.861 s | 2.26 s | 205.3 MiB | 206 个灯通过 |
| Minecraft Java 26.1.2 GameTest | 257.618 | - | - | 34.159 s | 88.42 s | 1698.9 MiB | 206 个灯与 Rust 一致 |

### Line Drawing, 225000 game tick

| 实现 | 总 tick/s | 稳定前 tick/s | 稳定 tick | tick 阶段 | wall | 峰值 RSS | 正确性 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `redstone-rs` compiled | 11386.807 | 9704.431 | 191672 | 19.760 s | 24.14 s | 1404.0 MiB | 36 个灯通过 |
| `redstone-rs` interpreted, 历史 | 2501.382 | 2130.913 | 191672 | 89.950 s | 90.61 s | 97.0 MiB | 36 个灯通过 |
| MCHPRS 普通引擎 | 1748.683 | 1489.656 | 191670 | 128.668 s | 129.40 s | 129.6 MiB | 36 个灯通过 |
| MCHPRS Redpiler | 11230.790 | 11108.074 | 191670 | 20.034 s | 21.46 s | 195.6 MiB | 36 个灯通过 |
| Minecraft Java 26.1.2 GameTest | - | - | - | - | - | - | 未执行完整 225000 tick |

当前 compiled 构图耗时中位数为 hello-world 2909.625 ms, line-drawing 3032.389 ms, 不计入 tick/s. wall 和峰值 RSS 与 tick 数据来自同一批独立进程.

这不是单一速度排名. `redstone-rs` 和 Java GameTest 按 game tick 执行完整规则, MCHPRS 普通引擎以 1 redstone tick 对应 2 game tick, Redpiler 则预计算连接并使用 `optimize` 和 `io_only` 缩小运行时工作. Redpiler 编译时间单列在详细报告中, 不计入 tick/s. Java line-drawing 不用短截断结果代替完整程序结果.

## 模拟器功能对比

| 能力 | `redstone-rs` | Minecraft Java 26.1.2 | MCHPRS 普通引擎 | MCHPRS Redpiler | [3D Redstone Simulator `d52c5ca0`](https://github.com/GuilhermeRossato/3D-Redstone-Simulator/commit/d52c5ca09ad62f18abdcccc9b6eb18cae12b5478) |
| --- | --- | --- | --- | --- | --- |
| Java 规则目标 | 26.1.2, default/experimental 更新顺序 | 26.1.2 原版 | 1.20.4 计算红石子集 | 1.20.4 编译图子集 | 无版本化红石执行规则 |
| 执行策略 | `auto`, `interpreted`, `compiled`, 编译器只加速电气传播 | 原版解释执行 | 解释执行 | 预编译连接图 | 浏览器逻辑 |
| 真实三维线路 | 支持 | 支持 | 支持 | 编译三维世界中的连接 | 支持三维世界和方块外观 |
| wire, torch, repeater, comparator | 支持 | 支持 | 支持 | 支持编译后的节点子集 | 未实现传播 |
| 普通/黏性活塞, QC, zero-tick, 黏连分支 | 支持, 编译模式仍由 Java 规则处理, 方块实体移动仍有限制 | 支持 | 不支持活塞行为 | 不支持 | 未实现, 位于计划中 |
| 观察者 | 支持 | 支持 | 不支持主动行为 | 不支持 | 未实现 |
| 容器, hopper 和实体传感器 | 最小容器转移和实体模型 | 完整游戏规则 | 比较器容器和玩家交互子集, 无通用实体模型 | 容器常量和输入节点子集 | 无红石执行模型 |
| 世界及 schematic 输入 | 世界目录/ZIP, Litematic, Sponge, vanilla structure | 世界和 oracle 生成的 structure | plot 和 Sponge schematic | 从 plot/选区编译 | 自有浏览器世界持久化 |
| 场景动作和断言 | TOML action, probe, expectation | GameTest 适配全部场景动作和 probe | 基准适配器支持按钮和最终灯断言 | 同左 | 无可复现红石断言协议 |
| 运行中修改结构 | action, 定时 paste 和编译拓扑同步, 失败时按执行模式回退或报错 | 支持 | WorldEdit 和玩家修改 | 修改会 reset 并停用 Redpiler | 支持浏览器放置/破坏方块 |
| 回放和可视化 | Replay Mod MCPR 可保留 compiled, JSONL/VCD 使用 interpreted | 原版客户端, oracle JSONL | Minecraft 客户端 | Minecraft 客户端 | 浏览器三维可视化和世界历史 |

`redstone-rs` 的 compiled 后端保留 `SparseWorld`, 计划刻, 方块事件和 Java 26.1.2 规则作为权威状态, 编译图只加速电气传播. 活塞及其动态拓扑变化仍通过现有 Java 规则执行并同步回编译拓扑. MCHPRS 普通引擎确实在三维世界中计算红石, 但当前红石执行入口没有活塞和观察者行为. Redpiler 通过预搜索 wire 路径和保存连接换取高吞吐, 运行时改建会使编译结果失效. 3D Redstone Simulator 当前主要是浏览器三维世界项目, 其 README 把 redstone simulation 和 piston simulation 列为后续目标, 因而不进入性能表.

## 构建和测试

```shell
just generate-reports
just build
just clippy
just test
```

> 注: 如果 build 失败, 请删除 `assets/libraries` 文件夹之后重新执行 `just generate-reports && just build`.

### 执行器选择

`run`, `test`, `trace` 和 `bench` 接受 `--engine auto|interpreted|compiled`, 默认使用 `auto`. 执行器是 CLI 和库运行策略, 不写入场景 TOML.

- `auto` 尝试编译电气传播图. 诊断模式不兼容时记录 warning 并使用解释器, 构图或动态拓扑同步失败时记录 warning 并永久回退到解释器.
- `interpreted` 始终使用 Java 26.1.2 规则解释执行.
- `compiled` 强制使用编译后端, 用于差分和性能测试. 遇到无法安全编译或同步的状态时直接返回错误.
- trace, VCD, Java oracle 和 experimental redstone 需要完整诊断或特定更新顺序. `auto` 会改用 interpreted, `compiled` 会明确报错.
- Replay MCPR 读取有序 `WorldDelta`, 不要求微轨迹, 因而录制 Replay 本身不会停用 compiled 后端.

```shell
cargo run --release -p redstone-cli -- run assets/scenarios/flying-machine.toml --engine auto
cargo run --release -p redstone-cli -- test assets/scenarios/flying-machine.toml --engine compiled
cargo run --release -p redstone-cli -- trace assets/scenarios/flying-machine.toml --engine interpreted --output /tmp/flying-machine.jsonl
cargo run --release -p redstone-cli -- bench --engine auto
```

运行摘要会同时输出请求模式和实际后端, 回退原因, `compile_ms`, 节点和边数量, 编译/解释更新数, compiled 命中率, 局部/全量重编译次数, 重编译节点数和 `recompile_ms`.

常用结构检查和转换命令:

```shell
cargo run -p redstone-cli -- inspect machine.litematic
cargo run -p redstone-cli -- inspect machine.litematic --block 10,20,30
cargo run -p redstone-cli -- inspect machine.litematic --region 0,5,0 10,5,20 --type minecraft:hopper
cargo run -p redstone-cli -- inspect machine.litematic --type minecraft:hopper --format json
cargo run -p redstone-cli -- inspect machine.litematic --all --json
cargo run -p redstone-cli -- inspect path/to/world --region 0,-64,0 255,319,255 --json
cargo run -p redstone-cli -- inspect path/to/world --region 0,-64,0 255,319,255 --skip-old-regions
cargo run -p redstone-cli -- convert machine.litematic machine.schem
cargo run -p redstone-cli -- convert path/to/world machine.litematic --region 0,-64,0 255,319,255
cargo run -p redstone-cli -- convert scenario.toml prepared/scenario.toml
```

`inspect` 默认输出结构汇总. `--block X,Y,Z` 只查询单个坐标并可重复使用. `--region FROM TO` 使用两个方块坐标查询闭合区域内的非空气方块, 两个端点不要求按大小排序, 并可叠加 `--type BLOCK_ID` 按方块类型筛选. 对世界目录或 ZIP, `--region` 同时限制需要读取的 chunk. `--all` 输出全部非空气方块. 明细包含状态 ID, properties, 支持状态和完整方块实体 NBT, 包括嵌套的 `components`. `--format json` 与 `--json` 均可输出 JSON.

`convert` 根据输入自动识别场景 TOML, Minecraft Java 26.1.2 世界目录, 世界 ZIP 或结构文件, 并根据输出扩展名写出 Litematic v7, Sponge v3 或 vanilla structure NBT. 世界 ZIP 直接从 archive entry 读取, 不创建中间解压目录, 可将存档文件散放在 ZIP 根目录或放在一个顶层文件夹中. 世界输入仅读取 `minecraft:overworld`. 普通世界必须通过 `--region FROM TO` 指定两个方块坐标形成的有限区域. 可证明使用标准虚空生成设置的世界可以省略范围, 此时自动读取全部已保存内容. 场景转换会在输出旁创建独立 assets 目录, 供 Java oracle 或其他场景副本直接加载.

`inspect` 和 `convert` 默认拒绝 DataVersion 不是 4790 的 chunk. `--skip-old-regions` 会跳过包含旧版 chunk 的整个 `.mca` 文件并输出 `WARN`, 其他 region 继续加载. 该选项可能产生缺少方块或实体的不完整结果, 只适合临时检查或导出已升级部分.

### 在游戏内升级旧版 region

1. 先复制并备份原世界. 如果输入是 ZIP, 将世界目录解压到 Minecraft 的 `saves` 目录, 并确认 `level.dat` 位于该世界目录顶层.
2. 使用 Minecraft Java 26.1.2 启动游戏, 进入单人游戏世界列表.
3. 选中目标世界, 依次点击 `编辑` 和 `优化世界`. 建议保留游戏提供的备份选项.
4. 点击 `开始优化`, 等待进度达到 100%. 不要在处理中关闭游戏.
5. 优化完成后退出游戏, 再将该世界目录交给 `redstone inspect` 或 `redstone convert`. 需要 ZIP 时可以重新压缩世界目录.

仅打开并保存世界不会升级全部已保存 chunk. `优化世界` 会遍历已有 region 并通过游戏的数据修复器写入当前 DataVersion, 因此是完整升级存档的推荐方式.

## 部分原理图来源

- [cpu-8bit.litematic](assets/schematics/cpu-8bit.litematic): <https://www.planetminecraft.com/project/new-redstone-computer/>
- [dvdprogram.schem](assets/schematics/dvdprogram.schem): <https://github.com/mattbatwings/BatPU-2>
- [digital-clock.litematic](assets/schematics/digital-clock.litematic): <https://www.planetminecraft.com/project/digital-clock-v2/>
- [head.litematic](assets/schematics/head.litematic): <https://www.planetminecraft.com/project/redstone-automaton-5932751/>
- [CPU_22_08_2025.zip](assets/worlds/CPU_22_08_2025.zip): <https://www.planetminecraft.com/project/frostbyte-a-16-bit-minecraft-cpu-with-just-redstone/> (<https://github.com/IceWizard7/frostbyte-cpu>)
