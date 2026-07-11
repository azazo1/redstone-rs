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

## 第四轮修改

模拟核心原先把 `load`, `initialize`, `apply`, `step`, `run_until` 和 `snapshot` 暴露为 async, 但这些操作没有等待点, 也不能与同一个可变模拟器并发执行. 这些接口改为同步函数, core 和 Java 规则测试改回普通 `#[test]`. CLI 只在 Java oracle 子进程, semaphore 和异步流读取处保留 async.

`redstone-java-26` 原先让 Cargo 为 10 个 integration test 文件分别创建测试进程. `OfficialStateCatalog` 的 `OnceLock` 只能在单进程内复用, 因此每个测试进程都重新解析一次 `blocks.json`. 按 GameTest 复用同一 server 和 registry 的思路, crate 关闭自动测试发现, 用一个 `tests/suite.rs` 包含全部 10 个测试模块.

| Java 规则测试 | 分散测试进程 | 单一 suite | 改善 |
| --- | ---: | ---: | ---: |
| warm wall time | 25.66 s | 2.16 s | 11.9x |
| user CPU | 22.06 s | 2.75 s | 8.0x |
| integration test 本体 | 多个 0.25-1.54 s 进程 | 1.53 s | 不适用 |

合并后的首次运行包含新 suite 编译, wall time 仍只有 5.55 s. 73 个 integration test 和 6 个库测试全部通过.

## 实体传感器按占用位置执行

状态共享后的百万方块 profile 获得 696 个主线程样本. 292 个样本位于活动 tick, 其中 205 个进入 `refresh_entity_sensor`, 152 个进入 `entity_ids_in_aabb`. 当时实现每 tick 遍历全部压力板, 探测铁轨和绊线, 即使世界中没有实体.

原版压力板和绊线由实体碰撞回调触发, 已按下元件才通过 scheduled tick 复查和延迟释放. Rust 实现改为先处理实体, 再只检查实体实际占据的去重方块位置. 已激活传感器继续使用原有 scheduled tick 释放, 因而空闲成本从 `O(全部传感器)` 降为 `O(实体数)`.

百万方块 bench 中的 `active` 表示可被激活的碰撞传感器数量, 该基准没有生成实体. 修改后 10000 tick 的分位数如下.

| 百万方块, 1 万传感器 | 第三轮 | 实体驱动 | 改善 |
| --- | ---: | ---: | ---: |
| tick p50 | 0.651 ms | 0.000083 ms | 7843x |
| tick p95 | 0.688 ms | 0.000084 ms | 8190x |
| tick p99 | 0.721 ms | 0.000084 ms | 8583x |

新的 tick 时间已经接近逐次读取计时器的开销, 不能外推为复杂红石计算的通用加速比. 该结果只证明空闲大型世界不再因为放置了大量碰撞传感器而产生线性 tick 成本.

## 状态转换缓存

单线程 release integration suite 的 `samply` profile 获得 1128 个测试线程样本. 694 个样本进入 `step_with_actions`, 500 个进入 wire 更新, 282 个进入 `Java26Registry::with_property`. 其中包括 88 个 `BTreeMap` 深克隆样本和 105 个 `state_key` 构造样本.

原版 `StateHolder` 保存属性值到相邻状态的转换. Rust registry 增加按 `state`, `property`, `value` 分层的惰性转换缓存. 相同 wire power 或 shape 转换命中时直接返回目标 `BlockStateId`, 只有首次组合复制属性表并查询官方目录. 查询参数同时改为 `AsRef<str>`, 缓存命中不再要求预先分配属性值字符串.

单线程 release suite 从 profile 运行的 0.29 s 降到暖运行 0.19 s, 改善约 1.53x. `flying-roof` 的 user CPU 从约 0.07 s 降到 0.05 s, 连续 wall time 为 0.09, 0.08 和 0.06 s. 短 CLI 命令仍明显受注册表冷启动和进程启动影响.

## 当前结论

大型红石器械的主要通用热路径已经从全量方块实体复制, 无条件 trace, 状态深克隆和全量传感器扫描中移除. `flying-roof` 相对修正后的同 tick 基线达到约 29.1x wall time 加速, Java 规则测试通过单进程目录复用达到 11.9x wall time 加速.

短 CLI 命令的主要剩余固定成本是首次解析内嵌 `blocks.json`. 大量未来计划刻的扩展性风险仍在全局 `BTreeSet`, 后续可参考 `LevelTicks` 做 chunk 分桶和到期桶合并. 这两项分别针对冷启动和超大计划刻队列, 不应与已经消除的逐 tick 全量工作混在同一轮微调中.

最终的 `profile.json.gz` 使用属性转换缓存后的代码生成, 采样对象是单线程 release Java integration suite. 73 个测试在采样下用时 0.25 s. 生成命令如下.

```shell
cargo samply --profile samply -p redstone-java-26 --test suite --samply-args="--rate 4000 --save-only --output profile.json.gz" -- --test-threads=1
```

## CPU+DVD 场景专项

`assets/scenarios/cpu-8bit-dvd.toml` 使用用户生成的 `assets/schematics/cpu-8bit.nbt`. 为缩短优化循环, 当前工作区把 `max_ticks` 从 staged 的 300 临时降为 60. 普通 release 连续运行 5 次的优化前结果为 137.8, 143.8, 143.7, 146.9 和 147.1 tick/s. 中位数为 143.8 tick/s.

等价 Java GameTest 在 300 tick 下用时 1153.942 ms, 即 259.978 tick/s. Rust 最初版本约为 30-33 tick/s, 因而当前版本相对最初值已经约有 4.5x 加速, 也达到原游戏 20 tick/s 的约 7.2x, 但尚未达到 200 tick/s 目标.

带调试符号的 samply profile 获得 9339 个样本. 主要包含热点如下. 表中路径互相包含, 不能直接相加.

| 热点 | 包含占比 |
| --- | ---: |
| `Simulation::process_neighbor_tasks_with_changes` | 64.15% |
| `Java26Rules::on_neighbor_update` | 49.08% |
| `Java26Rules::update_wire` | 32.17% |
| `Java26Rules::wire_target_power` | 25.57% |
| `SparseWorld::section` | 8.94% |
| 坐标缓存的 SipHash | 11.18% |

本轮先处理 4 个不改变规则顺序的固定成本.

1. 稠密 section 的空槽不再回退查询稀疏 `BTreeMap`.
2. trace 关闭时不再构造 `NeighborUpdate` trace 值.
3. wire power 变化检查相邻 observer 时, 只在目标确实是 observer 时克隆状态.
4. wire 输入缓存改用针对 `BlockPos` 的轻量哈希器.

普通 release 连续运行 7 次得到 150.9, 146.8, 148.0, 147.4, 148.5, 148.9 和 149.9 tick/s. 中位数为 148.5 tick/s, 相对本轮 143.8 tick/s 基线改善约 3.3%. 结果说明剩余成本主要来自实际 neighbor 链和 wire 功率计算, 仅清理外围固定开销不足以达到 200 tick/s.

### Neighbor 任务栈复用

`process_neighbor_tasks_with_changes` 原先为每次规则回调创建一个临时 `Vec<NeighborTask>`, 回调结束后再倒序搬到主处理栈. wire 功率变化会生成多组 neighbor update, 因而在热路径中持续分配, 释放和复制任务枚举.

`EventContext` 现在直接借用当前任务栈. 每次回调记录追加前的长度, 回调结束后只反转新增切片, 从而保持 nested task 先于 multi continuation 的原有顺序. Core 中验证 nested neighbor, deferred tick 和 deferred block change 顺序的 17 个测试全部通过.

普通 release 连续运行 7 次得到 142.5, 162.7, 162.6, 163.9, 164.0, 159.6 和 163.6 tick/s. 首次运行存在冷启动抖动, 全部结果中位数为 162.7 tick/s. 相对上一轮 148.5 tick/s 中位数改善约 9.6%.

### 状态预分类和方块读取内联

每条 neighbor update 原先都按方块名称判断 rail 和 piston head, 再用一个较长的 `BlockBehavior` match 判断是否需要处理. 这些结果对注册后的 state 永远不变, 因此改为在 `StateDefinition` 构建时预计算. Redstone wire 的 `BlockKindId` 也在 `Java26Rules` 构建时缓存, 避免每条更新查询名称.

`SectionPos` 计算, 稠密 section 索引, palette 读取和 `SparseWorld::get_block` 是 wire 功率计算的最底层路径. 这些短函数增加明确内联, 让优化器把边界检查和 `Option` 分支合并到调用点.

Java 规则的 80 个测试全部通过. 普通 release 连续运行 7 次得到 146.3, 197.7, 193.1, 197.4, 182.7, 198.2 和 199.9 tick/s. 全部结果中位数为 197.4 tick/s, 相对上一轮 162.7 tick/s 中位数改善约 21.3%. 除冷启动和一次系统抖动外, 热运行已经接近 200 tick/s, 但仍需要提高稳定余量.

### Wire 最大值早停

一次规则级 neighbor 预筛选实验把静态目标挡在 `EventContext` 之外, 但 CPU 场景的大多数通知确实落在活跃红石元件上. 额外 trait 调用和重复方块读取使结果降到 94.7-185.4 tick/s, 因此该实验已撤回.

保留的下一轮优化只利用信号范围的数学上界. `signal_without_wire_feedback` 得到 15 后不再扫描剩余方向, wire 邻接扫描得到 15 后也不再继续求最大值. Wire power 的 `0-15` 属性值改用静态字符串表, 避免每次状态转换创建临时 `String`. 这些变化不改变最大值和 neighbor 顺序.

Java 规则的 80 个测试再次全部通过. 普通 release 连续运行 10 次得到 199.0, 207.6, 206.3, 208.5, 208.2, 199.4, 211.0, 206.0, 203.7 和 205.3 tick/s. 中位数约为 206.2 tick/s, 相对最初 30-33 tick/s 约有 6.3-6.9x 加速, 也达到原游戏 20 tick/s 的约 10.3x.

CPU+DVD 的完整 Java oracle 对比使用临时的 60 tick 场景运行, 共比较 7937630 条微轨迹事件, 结果通过. 这覆盖了当前 CPU 活动阶段的 wire, repeater, comparator 和 neighbor 顺序, 证明最大值早停和任务栈复用没有改变 oracle 可见行为.

## 世界转换并行化

Anvil region 的文件定位, ZIP entry 读取和世界状态合并必须保持顺序. Region 读取器现在每批最多缓冲 32 个 chunk, 顺序取得压缩数据后使用 Rayon 并行解压, 再按原 chunk 顺序解析和合并. 该边界限制了额外内存, 同时保持 palette 注册, 方块实体和实体顺序确定.

Litematic, Sponge 和包含空气的 vanilla writer 会预分配完整状态数组, 再按 16384 个方块一组并行读取 `SparseWorld`. 状态描述增加 `BlockStateId -> palette index` 缓存, 同一状态不再为每个方块重复解析. Palette index 和方块遍历顺序保持确定, NBT 编码, 压缩和原子文件替换仍保持顺序. 现有 HashMap NBT 序列化不承诺跨进程字节完全相同, 因此验证以重新加载后的 region, 方块状态, 方块实体和实体语义为准. 省略空气的 vanilla writer 继续按稀疏方块顺序编码, 避免为大范围空气建立稠密数组.

release 模式使用 8 个可用处理器进行对比. CPU 世界全 region inspect 在 `RAYON_NUM_THREADS=1` 下为 5.57 s, 默认线程池为 5.19 s. 小范围 26 chunk 转换约为 0.19-0.20 s, 并行启动成本与收益接近. 15 MB `cpu-8bit.nbt` 转换约为 6.57-6.85 s, 该案例主要受输入 NBT 解析, 顺序世界构建和压缩限制, 不能从并行状态扫描获得明显收益.

### 最终验证

当前 release 同时通过 tracing 的 `release_max_level_info` 移除 debug 和 trace 级别 callsite, 因此热路径中的 `debug!` 不进入 release 二进制执行路径.

最终验证结果如下.

| 验证 | 结果 |
| --- | --- |
| `cargo test --workspace` | 通过 |
| `cargo clippy --workspace --all-targets` | 通过 |
| `just oracle-scenario-self-test` | 通过, Java GameTest 约 497.6 tick/s |
| 三种结构格式 oracle converter 测试 | 通过 |
| initial source paste Java 微轨迹 | 通过 |
| default wire chain Java 微轨迹 | 通过 |
| CPU+DVD 60 tick 完整 oracle | 通过, 7937630 条事件 |
