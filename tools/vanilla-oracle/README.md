# Vanilla oracle protocol

Java oracle 只用于测试和 CI, 不进入 Rust 发布产物.

入口接收两个位置参数:

```shell
oracle scenario.toml output.jsonl
```

探针必须加载 Java `26.1.2`, 按场景声明执行 GameTest 或 ASM 注入, 并将结果写入第二个参数.

当前支持两种输出:

- 完整轨迹 JSONL, 字段和排序与 `redstone-core::TraceEvent` 一致.
- 首行为 `{"format":"probe_samples_v1"}` 的探针样本 JSONL, 后续行为 `tick`, `probe`, `value`. CLI 会提取 Rust 的 `post_tick` 探针并比较.

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
- `raw` 初始化, `origin = 0`, 无旋转和镜像, `seed = 0`, 默认红石模式.
- `set_block`, `break_block`, `use_block`, `press_button`, `pull_lever`.
- `signal`, `block_state`, `property`, `container_count`.

不在上述范围内的场景会被明确拒绝. ASM 微时序轨迹采集和实验性红石 feature flag 仍在实现中.
