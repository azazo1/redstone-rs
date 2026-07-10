# Vanilla oracle protocol

Java oracle 只用于测试和 CI, 不进入 Rust 发布产物.

入口接收两个位置参数:

```shell
oracle scenario.toml output.jsonl
```

探针必须加载 Java `26.1.2`, 按场景声明执行 GameTest 或 ASM 注入, 并将结果写入第二个参数.

当前支持三种输出:

- 完整轨迹 JSONL, 字段和排序与 `redstone-core::TraceEvent` 一致.
- 首行为 `{"format":"probe_samples_v1"}` 的探针样本 JSONL, 后续行为 `tick`, `probe`, `value`. CLI 会提取 Rust 的 `post_tick` 探针并比较.
- 首行为 `{"format":"oracle_samples_v2"}` 的组合样本 JSONL. `kind = "probe"` 记录逐 tick 探针, `kind = "neighbor_update"` 记录真实邻居执行顺序, `Orientation` 和 `moved_by_piston`, `kind = "scheduled_tick_queued"` 和 `kind = "scheduled_tick_executed"` 记录计划方块刻, `kind = "block_event_queued"` 和 `kind = "block_event_executed"` 记录方块事件, `kind = "block_changed"` 使用官方全局 state ID 记录状态写入.

先构建并校验本地 oracle 环境:

```shell
just oracle-build
just oracle-self-test
just oracle-server-self-test
just oracle-scenario-self-test
```

默认包装器使用 `tools/vanilla-oracle/build/vanilla-oracle.jar`. 也可以通过 `VANILLA_ORACLE_JAR` 覆盖:

```shell
set -x VANILLA_ORACLE_JAR /path/to/vanilla-oracle.jar
cargo run -p redstone-cli -- test scenarios --oracle
```

也可以通过 `REDSTONE_ORACLE` 指向实现相同协议的可执行程序.

当前 JAR 已实现 Java `26.1.2`, DataVersion `4790`, Bootstrap, 方块注册表, 内置 GameTestServer 生命周期和真实场景自检.

探针级场景执行首批支持:

- 原版 structure NBT.
- `raw` 和 `notify` 初始化, 非零 `origin`, 旋转, 镜像和任意场景 seed.
- 默认和实验性红石模式.
- `set_block`, `break_block`, `use_block`, `press_button`, `pull_lever`, `hit_target`.
- `spawn_entity`, `move_entity`, `remove_entity`, `set_entity_field`.
- `signal`, `block_state`, `property`, `container_count`.
- `entity_count`, `entity_field`, `entity_container_count`.
- 原版注册实体及 `generic_collision` 测试别名, 物品实体和容器矿车字段适配.
- 测试专用 `oracle_micro_trace = true`, 固定 GameTest 绝对原点并启用邻居更新, 计划方块刻, 方块事件及状态写入 ASM 采样.

不在上述范围内的场景会被明确拒绝. 形状更新的完整 ASM 微时序轨迹仍在实现中.
