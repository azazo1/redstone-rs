# redstone-rs

基于 Rust 的 Java Edition 26.1.2 默认红石仿真内核.

- 使用稀疏 section 世界, 确定性计划刻和 Java 默认邻居更新顺序.
- 支持 structure NBT, WorldEdit Sponge `.schem`, Litematica `.litematic` 和 Anvil `.mca` region 导入.
- 当前覆盖核心红石元件, 比较器容器信号和活塞准连接. 物流, 实体物理和 3D 编辑器尚未实现.

```shell
cargo run -- run --structure machine.nbt --ticks 200 --trace trace.json
cargo run -- run --structure machine.litematic --ticks 200
cargo run -- run --structure r.0.0.mca --ticks 200
cargo run -- vector --vector cases/repeater.json --output repeater-trace.json
cargo run -- benchmark --blocks 100000 --ticks 1000
```
