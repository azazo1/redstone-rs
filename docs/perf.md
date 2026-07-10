# 性能优化记录

## 目标

面向大型红石器械提升单 tick 模拟吞吐, 同时缩短 `redstone run` 和 `redstone test` 的执行时间. 优化以 `samply` 采样和可重复的场景计时为依据, 不把缺少证据的微调当作结论.

## 初始证据

仓库根目录的 `profile.json.gz` 使用 1 ms 采样间隔. 主线程获得 17198 个样本, 对应约 17.2 秒的 CPU 密集执行. 该 profile 来自修改前的 `flying-roof` 场景, 当时运行 1200 tick, 产生 260749 条 trace, 耗时 17.17 秒.

当前 `HEAD` 已把该场景缩短到 170 tick. 重新构建 `samply` profile 后耗时 2.33 秒, 即约 13.7 ms/tick. 旧场景约为 14.3 ms/tick, 因此修正前后的单位 tick 成本一致, 旧 profile 仍能解释当前热路径.

当前场景基线如下.

| 场景 | tick | trace 事件 | 时间 |
| --- | ---: | ---: | ---: |
| `charging-tests` | 70 | 2027 | 0.40 s |
| `flying-roof` | 170 | 31574 | 2.33 s |
| `mux3-8-experimental` | 160 | 7849 | 0.07 s |
| `mux3-8` | 160 | 40989 | 0.08 s |
| `piston-gate-3x3` | 1200 | 39030 | 0.14 s |
| `piston-push-destroy-tests-2` | 40 | 1106 | 0.05 s |
| `piston-push-destroy-tests` | 40 | 815 | 0.05 s |
| `seg7` | 400 | 264479 | 0.34 s |
| `tripple-piston-extender` | 103 | 3729 | 0.10 s |

`charging-tests` 是本轮构建后的首次冷启动, 其 wall time 包含额外启动抖动. 后续比较使用多次运行和中位数.

全量 `cargo test --workspace` 的旧基线为 20.82 秒. 其中重新编译占 12.69 秒, 测试运行约 8 秒, 并在旧的 3x3 活塞门失败处提前停止. 当前修正已经让该场景通过, 因此优化后需要重新建立完整测试基线.

## Samply 结论

`flying-roof` 的主导路径是 `Simulation::with_context_and_changes` 对所有方块实体建立快照, 再由 `EventContext::record_untracked_block_entity_changes` 完整复制, 合并和比较两棵 `BTreeMap`.

关键包含样本如下.

| 热点 | 包含样本 | 主线程占比 |
| --- | ---: | ---: |
| 字符串克隆和方块实体 diff 上层 | 4458 | 25.9% |
| `record_untracked_block_entity_changes` | 4171 | 24.3% |
| 同一 diff 的相邻栈帧 | 3926 | 22.8% |
| `BTreeMap::from_iter` | 3681 | 21.4% |

这些栈帧互相包含, 不能相加. 叶节点样本同时集中在 `malloc`, `free`, `memcpy`, 字符串克隆, `BTreeMap` 子树克隆和比较. 这证明瓶颈来自每次规则回调的全量状态复制, 而不是红石规则计算本身.

## 原版 Java 和 GameTest 对照

原版 `CollectingNeighborUpdater` 使用长期复用的 `ArrayDeque` 和分层列表执行邻居更新. 调试监听器为 `null` 时不生成调试事件. Rust 版本此前无条件保存每一条 trace, 即使调用方没有请求输出.

原版 `LevelTicks` 按 chunk 保存计划刻. 它使用 primitive hash map 记录每个容器的下一次触发时间, 只把当前到期容器放进优先队列, 并在确实查询 `willTickThisTick` 时才惰性构建集合. Rust 版本当前仍使用全局 `BTreeSet`, 这是后续需要处理的扩展性问题.

`GameTestServer::waitUntilNextTick` 只调用 `runAllTasks`, 不执行普通服务端的等待和 `parkNanos`. 它还关闭 tick 时间日志, 使用一个 `GameTestTicker` 和同一个世界批量承载测试, 按 batch 生成和清理结构. 关键技巧是取消真实时间节流, 关闭非必要观测, 复用昂贵初始化, 以及批量执行.

## 第一轮修改

1. 删除每次规则回调的全量方块实体快照和 diff. 运行时修改统一通过 `EventContext::set_block_entity`, `update_block_entity` 或 `remove_block_entity` 记录增量事件.
2. trace 默认关闭. CLI 仅在请求 JSONL, VCD 或 oracle 对比时启用.
3. 没有 delta 订阅者时不再克隆整份 `WorldDelta`.

### 第一轮结果

`redstone-core` 的 15 个测试和 `redstone-java-26` 的 79 个测试通过. 其中只有 14 个专门检查内部事件顺序或 unsupported 诊断的测试显式启用 trace, 其余测试使用无 trace 快路径.

重新构建 `samply` profile 后连续运行 7 次 `flying-roof`. 首次冷启动为 0.43 秒, 后续 6 次均为 0.08 秒, user CPU 均约为 0.07 秒. 相对当前 `HEAD` 修改前的 2.33 秒基线, 热启动加速约为 29.1x. 单 tick wall time 从约 13.7 ms 降到约 0.47 ms.

| 指标 | 修改前 | 第一轮 | 加速比 |
| --- | ---: | ---: | ---: |
| `flying-roof` wall time | 2.33 s | 0.08 s | 29.1x |
| `flying-roof` user CPU | 2.29 s | 0.07 s | 32.7x |
| 单 tick wall time | 13.7 ms | 0.47 ms | 29.1x |

第一轮已经达到数十倍加速, 但仍需在 trace 开启时分离两项优化的贡献, 并重新采样剩余热路径.

## 第二轮修改

trace 开启后运行 `flying-roof` 的 user CPU 为 0.41 秒, 相对 2.29 秒初始值仍有约 5.6x 加速. 但写出 31574 行, 6.06 MiB JSONL 时出现约 3.9 秒 system time, 总时间为 4.36 秒.

`TraceLog::write_jsonl` 此前对每个事件直接调用底层 `Write`, 产生大量小文件写入. 原版 Java oracle 使用 `BufferedWriter`. Rust 版本改为 64 KiB `BufWriter`, JSONL 和 VCD 结束时显式 flush. `Simulation::trace()` 同时改为借用事件切片, 不再为了写文件复制整份 trace.

修改后 trace 模式热启动为 0.08 到 0.09 秒, system time 接近 0. 31574 行和 6.06 MiB 输出保持不变. 相对 4.36 秒的直接写入, 总时间改善约 54.5x.

## 优化后 Samply

优化后的 `flying-roof` 使用 4 kHz 重新采样, 获得 383 个主线程样本. `record_untracked_block_entity_changes` 和方块实体 `BTreeMap` diff 已完全退出热点.

207 个样本, 约 54%, 位于 `Java26Registry::new` 首次解析内嵌 `blocks.json` 的 `OnceLock` 初始化. 这属于短命令的冷启动成本. 真正模拟阶段的样本分散在形状修复, 活塞分支, `SparseWorld::get_block`, 状态克隆和 SipHash.

目录命令 `redstone test assets/scenarios` 当前运行 9 个场景, 全部通过, wall time 为 0.35 秒, 合计 user CPU 为 0.50 秒.

## 第三轮修改

为了获得足够长的活动 tick 样本, 使用 100 万方块, 1 万个活动压力板, 100 tick 的 bench 重新运行 `cargo samply`. Profile 获得 1406 个主线程样本.

`tick_entities` 约占 28%. 每个传感器每 tick 都克隆一个 `StateDefinition`, 其中包含 `String` 和 `BTreeMap<String,String>`. 状态定义在注册后不可变, 因此将名称改为 `Arc<str>`, 属性改为 `Arc<BTreeMap<String,String>>`, 并让 `BlockBehavior` 成为 `Copy`. 规则代码中为了释放 `self` 借用而保留的 `.clone()` 由深复制变为引用计数递增.

| 百万方块 bench | 修改前 | 第三轮中位数 | 改善 |
| --- | ---: | ---: | ---: |
| world build | 68.376 ms | 61.036 ms | 1.12x |
| tick p50 | 2.643 ms | 0.651 ms | 4.06x |
| tick p95 | 2.978 ms | 0.688 ms | 4.33x |
| tick p99 | 3.256 ms | 0.721 ms | 4.52x |

5 次第三轮运行的 tick p50 为 0.643, 0.643, 0.656, 0.673 和 0.651 ms. 结果稳定, 1 万活动传感器达到约 1536 tick/s.
