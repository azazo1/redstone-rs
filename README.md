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

## 文档

- [已实现功能](docs/impl.md): 仿真内核, Java 规则, IO, CLI, Replay 和 oracle 的实际覆盖范围.
- [场景编写手册](docs/scenario-guide.md): TOML 场景, 动作, 探针, 断言, Replay 和运行方式.
- [实现状态](docs/todo.md): 已完成能力, 稳定前任务和暂不支持范围.
- [性能优化记录](docs/perf.md): 基准, profile 证据和优化结果.
- [Java oracle 协议](tools/vanilla-oracle/README.md): 参考探针的构建, 输出协议和支持范围.

## 构建和测试

```shell
just generate-reports
just build
just clippy
just test
```

> 注: 如果 build 失败, 请删除 `assets/libraries` 文件夹之后重新执行 `just generate-reports && just build`.

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
