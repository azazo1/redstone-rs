# redstone-rs

基于 Rust 的 Java Edition 26.1.2 默认红石仿真内核.

- 使用稀疏 section 世界, 确定性计划刻和 Java 默认邻居更新顺序.
- 支持 structure NBT, WorldEdit Sponge `.schem`, Litematica `.litematic` 和 Anvil `.mca` region 导入.
- 当前覆盖核心红石元件, 比较器容器信号, 漏斗和投掷器基础物流, 活塞准连接与运动状态. 物品实体物理和 3D 编辑器尚未实现.
- `vector` 输出的快照和 trace events 都会裁剪到观察区域. `diff` 会逐 tick 比较快照和事件顺序, 可直接作为 Java GameTest oracle 的交换格式.

```shell
cargo run -- run --structure machine.nbt --ticks 200 --trace trace.json
cargo run -- run --structure machine.litematic --ticks 200
cargo run -- run --structure r.0.0.mca --ticks 200
cargo run -- vector --vector cases/repeater.json --output repeater-trace.json
cargo run -- diff --expected vanilla-trace.json --actual repeater-trace.json --output differences.json
cargo run -- benchmark --blocks 100000 --ticks 1000
```

## Java GameTest oracle

`assets/client-26.1.2.jar` 包含 GameTest 类, 但不能独立运行. `just gametest-oracle` 使用官方 26.1.2 server bundler 在 Java 25 下解包运行库并启动 GameTest 主类. 运行数据保存在 `.gametest/<version>/`.

实际 oracle 需要用户接受 Minecraft EULA. `just oracle-run` 会在项目内构建 Java bridge, 让官方 GameTest 加载同一 structure 和输入脚本, 然后逐 tick 输出 `SimulationTrace` 快照 JSON. Rust 侧使用 `redstone-rs diff --snapshots-only` 比较观察区域. 原版没有公开的方块事件队列, bridge 当前不伪造事件顺序, 因此 Java trace 的 `events` 为空.

```shell
just download-server 26.1.2
just oracle-init
just gametest-oracle 26.1.2 --packs .gametest --tests redstone_oracle:smoke --report .gametest/26.1.2/smoke.xml
just oracle-structure machine.nbt .gametest/oracle-pack/data/redstone_oracle/structure/machine.nbt
just oracle-case case.json
just oracle-run case.json
just gametest-oracle 26.1.2 --tests minecraft:always_pass --report .gametest/26.1.2/always-pass.xml
redstone-rs vector --vector case.json --output rust-trace.json
redstone-rs diff --expected vanilla-trace.json --actual rust-trace.json --output differences.json
```
