# Vanilla oracle protocol

Java oracle 只用于测试和 CI, 不进入 Rust 发布产物.

入口接收两个位置参数:

```shell
oracle scenario.toml output.jsonl
```

探针必须加载 Java `26.1.2`, 按场景声明执行 GameTest 或 ASM 注入, 并将轨迹写入第二个参数. JSONL 字段和排序必须与 `redstone-core::TraceEvent` 一致.

默认包装器读取 `VANILLA_ORACLE_JAR`:

```shell
set -x VANILLA_ORACLE_JAR /path/to/vanilla-oracle.jar
cargo run -p redstone-cli -- test scenarios --oracle
```

也可以通过 `REDSTONE_ORACLE` 指向实现相同协议的可执行程序.
