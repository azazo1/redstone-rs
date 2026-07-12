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

### Replay 稳定尾部时间锚点

Replay exporter 原先只在世界事件发生时写入 packet. 当机器提前进入稳定状态时, `metaData.json` 和 Time Path 仍结束于场景配置时间, 但 `recording.tmcpr` 会提前在最后一次方块更新处结束. 例如 20 秒样本的 metadata 为 20000 ms, 实际最后 packet 却在 18250 ms. Replay Mod 的 `FullReplaySender` 在同步渲染时遇到 EOF 会清空输入流, 下一帧再从录像开头打开并扫描到目标时间. 稳定尾部因此可能逐帧重复扫描整段录像, 表现为越接近末尾越慢或卡住.

Exporter 现在分别跟踪逻辑结束时间和实际 packet 结束时间. 如果两者不同, finish 阶段会写入一个不改变世界内容的 chunk radius packet 作为时间锚点. 这不会按 tick 增加录像体积, 也不需要重复完整 chunk snapshot. 回归测试要求最后 packet 时间戳与 metadata duration 完全一致.

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

## 观察者面对面压力场景

`assets/scenarios/observer-clock-100.toml` 最终使用 100x100x100 个观察者组成 50 万组面对面时钟. 原始原理图只在 X 轴偶数位置放置朝东的观察者, 每个观察者之间保留 1 格空气. 场景把同一原理图设置 `mirror = "front_back"`, 并向 X 轴平移 99 格后忽略空气粘贴. 镜像副本中的观察者朝西并落入原有空位, 最终每两个相邻方块组成一组面对面时钟.

原理图由标准库 Python 脚本直接写出 Sponge schematic v3. 默认生成命令如下.

```shell
just generate-observer-clock
```

场景先以 `raw` 模式加载单侧结构, 再在第一个游戏刻之前粘贴镜像副本并执行选区更新. 镜像粘贴和选区更新会为全部观察者对建立初始计划刻, 但发生在场景 tick 计时开始之前. 因此 `ticking_elapsed_ms` 和 `ticks_per_second` 只衡量 100 tick 持续振荡阶段, 不包含结构解析, 镜像粘贴和启动更新耗时.

普通 release 和带调试符号的采样命令如下.

```shell
just run assets/scenarios/observer-clock-100.toml
cargo samply run assets/scenarios/observer-clock-100.toml
```

该场景设置 `skip_oracle = true`. 100 万观察者的 Java GameTest 微轨迹规模过大, 不适合作为日常性能基准的前置步骤. TOML 对第一组观察者在 tick 1 和 2 的状态进行断言, 用于确认镜像粘贴已经启动压力结构.

当前模拟器默认每个游戏刻最多执行 65536 个计划方块刻. 100 万观察者启动后会持续达到该上限, 因此大规模场景不会保持小型观察者时钟的全局同步相位. 这个场景测量的是计划刻队列饱和时的持续吞吐, 不是无限制执行全部到期计划刻的理论耗时.

首次完整 release 验证构建了 1000000 个观察者并通过全部断言. 100 tick 的 `ticking_elapsed_ms` 为 8839.001 ms, `ticks_per_second` 为 11.313. tick 2 到 tick 100 均达到 65536 个计划方块刻上限, 与原版 `ServerLevel.MAX_SCHEDULED_TICKS_PER_TICK` 一致.

## 16 位画线 CPU 场景

`assets/scenarios/frostbyte-cpu-16bit-line-drawing.toml` 包含 1551125 个方块. 漏斗容器对齐最初引入了每 tick 扫描全部方块实体的碰撞路径, 使该场景的 `active_ticks_per_second` 降到约 478. 改为由存活物品实体枚举相交漏斗方块后, 活动速率恢复到约 1560-1640. 该场景区域内没有漏斗, 因此剩余差距来自原有红石活动路径, 而不是漏斗传输本身.

30 秒主线程 profile 获得 56807 个加权样本. 主要包含热点如下. 路径互相包含, 不能相加.

| 热点 | 包含占比 | 叶样本占比 |
| --- | ---: | ---: |
| `process_neighbor_tasks_with_changes` | 93.19% | 25.36% |
| `on_neighbor_update` | 65.28% | 15.91% |
| `update_wire` | 41.66% | 5.47% |
| `wire_target_power` | 27.09% | 23.64% |

第一轮把 `Multi` 邻居任务改为留在栈顶原地推进, 并在创建 `EventContext` 前读取一次目标状态. Java 规则在该阶段执行信号缓存失效和静态目标筛选, 活跃目标继续复用同一个状态 ID. 这避免了旧预筛选实验中的重复方块读取, 同时保持所有 neighbor trace, 连锁计数和嵌套任务优先顺序.

完整 225000 tick 运行通过全部场景断言. `stable_tick` 为 191672, `active_ticks_per_second` 从修复漏斗回归后的约 1560-1640 提升到 1993.404. 总 `ticks_per_second` 为 2339.963, 但本场景只用活动速率作为验收指标.

第二轮继续让 `Multi` 路径直接产生 `NeighborUpdate`, 不再临时构造大尺寸 `NeighborTask::Single`. 低频 deferred 任务的堆拥有数据改为 boxed slice 或 box, 缩小高频任务栈元素. trace 开关和连锁更新上限也移出循环重复读取.

第二轮完成后连续两次完整运行的 `active_ticks_per_second` 为 2104.210 和 2165.149, 中位数为 2134.680. 两次 `stable_tick` 均为 191672, 全部场景断言通过. 相对第一轮 1993.404 提升约 7.1%, 相对漏斗碰撞修复后的 1560-1640 区间提升约 30%-37%, 并稳定超过 2000 目标.

| 完整 drawline 验证 | 第一次 | 第二次 |
| --- | ---: | ---: |
| `stable_tick` | 191672 | 191672 |
| `active_ticks_per_second` | 2104.210 | 2165.149 |
| `ticks_per_second` | 2470.036 | 2541.568 |
| 场景断言 | 通过 | 通过 |

## 跨实现 Frostbyte 基准

### 测试范围

本节使用同一台 Apple M1 8 核, 16 GB 机器在 2026-07-12 测量. `redstone-rs` 数据在性能优化后的 commit [`7f8344e70ed78c38e535db7eeaf9851d73ed45fc`](https://github.com/azazo1/redstone-rs/commit/7f8344e70ed78c38e535db7eeaf9851d73ed45fc) 上全部重跑, 旧测量不进入本节统计.

固定实现如下:

- `redstone-rs`: `7f8344e70ed78c38e535db7eeaf9851d73ed45fc`, release profile, 单个 CLI 进程.
- MCHPRS: [`8734f72bcf48be492c39e657d549e054255bed31`](https://github.com/MCHPR/MCHPRS/commit/8734f72bcf48be492c39e657d549e054255bed31), 单个 headless plot. 普通引擎和 Redpiler 分开测量.
- Minecraft Java: 26.1.2 GameTest, `oracle_micro_trace=false`, 仅在最终 tick 读取 probe.
- [3D Redstone Simulator `d52c5ca0`](https://github.com/GuilhermeRossato/3D-Redstone-Simulator/commit/d52c5ca09ad62f18abdcccc9b6eb18cae12b5478) 只有三维世界和浏览器交互, 该 commit 尚未实现 redstone simulation, 因此只进入功能矩阵.

共同负载如下:

| 场景 | 程序 | game tick | 按钮动作 | 最终断言 |
| --- | --- | ---: | ---: | ---: |
| `frostbyte-cpu-16bit-hello-world.toml` | Frostbyte hello-world | 8800 | tick 100 | 206 个屏幕灯 |
| `frostbyte-cpu-16bit-line-drawing.toml` | Frostbyte line-drawing | 225000 | tick 100 | 36 个屏幕灯 |

每个已发布的实现和场景都启动独立进程运行 3 次, 不删除异常值, 摘要对每一列取中位数. `redstone-rs`, MCHPRS 普通引擎和 Redpiler 均完整执行两个场景并通过最终灯断言. Java GameTest 完整执行 hello-world, 三轮输出与 Rust 的 206 个最终 probe 逐项一致. Java 不执行 225000 tick 的完整 line-drawing, 也不使用截断运行代替完整结果.

### 指标定义

- `tick 阶段`: 世界和程序装载完成后, 完整 tick 循环的耗时.
- `总 tick/s`: 配置的全部 game tick 除以 tick 阶段耗时. 机器提前稳定时, 该值包含空闲尾部.
- `稳定 tick`: 最后一个外部 action 之后, 连续 20 tick 没有模拟事件且没有待执行计划刻时, 取空闲窗口的第一个 tick.
- `稳定前 tick/s`: `stable_tick / active_ticking_elapsed`. 它衡量机器仍在传播或刚刚进入空闲点之前的平均速度.
- `wall`: 外部资源测量工具报告的端到端 real time.
- `峰值 RSS`: 外部资源测量工具报告的最大常驻内存, 原始表保留 byte, README 换算为 MiB.

稳定前速度可以消除 line-drawing 最后约 33328 个空闲 tick 对总吞吐的抬高, 但它不是单 tick 分位数. `redstone-rs` 观察方块变化, 方块实体变化, 规则事件和计划刻队列. MCHPRS 普通引擎只用计划刻队列判断空闲, Redpiler 使用内部 pending tick 状态, 并以 1 redstone tick 对应 2 game tick 换算. 本次 MCHPRS 的稳定点比 `redstone-rs` 早 2 game tick, 因此两个稳定点不应解释为微观事件完全一致.

### 执行命令

构建和验证当前项目:

```shell
cargo build --release -p redstone-cli
cargo test --release -p redstone-cli stability_tracker -- --nocapture
cargo clippy -p redstone-cli --all-targets
```

`redstone-rs` 每轮使用:

```shell
target/release/redstone run \
  assets/scenarios/frostbyte-cpu-16bit-hello-world.toml

target/release/redstone run \
  assets/scenarios/frostbyte-cpu-16bit-line-drawing.toml
```

MCHPRS 的源码, 结构归一化, 构建步骤和完整命令见 [mchprs-benchmark.md](mchprs-benchmark.md). Java 每轮使用:

```shell
just oracle \
  assets/scenarios/frostbyte-cpu-16bit-hello-world.toml \
  frostbyte-hello-oracle.jsonl
```

Java 正式三轮在依赖准备完成后开始. `wall` 包含 `just oracle` 的运行库存在性检查, oracle adapter 构建, 场景转换, Minecraft 初始化和 GameTest tick. `GAMETEST_TICKS` 只包含 GameTest tick 阶段, 两者不能相减后直接称为纯 Minecraft 初始化时间.

### `redstone-rs` 原始结果

Hello-world:

| 轮次 | tick ms | 总 tick/s | stable tick | 稳定前 ms | 稳定前 tick/s | wall s | RSS byte | 断言 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | 3577.965 | 2459.498 | 8688 | 3577.884 | 2428.251 | 4.36 | 95666176 | 通过 |
| 2 | 3420.551 | 2572.685 | 8688 | 3420.468 | 2540.003 | 4.05 | 97878016 | 通过 |
| 3 | 3583.236 | 2455.881 | 8688 | 3583.151 | 2424.682 | 4.19 | 98549760 | 通过 |
| 中位数 | 3577.965 | 2459.498 | 8688 | 3577.884 | 2428.251 | 4.19 | 97878016 | 通过 |

Line-drawing:

| 轮次 | tick ms | 总 tick/s | stable tick | 稳定前 ms | 稳定前 tick/s | wall s | RSS byte | 断言 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | 88953.650 | 2529.407 | 191672 | 88951.655 | 2154.788 | 89.60 | 101744640 | 通过 |
| 2 | 89950.290 | 2501.382 | 191672 | 89948.291 | 2130.913 | 90.61 | 100696064 | 通过 |
| 3 | 94815.110 | 2373.039 | 191672 | 94813.042 | 2021.578 | 95.51 | 102137856 | 通过 |
| 中位数 | 89950.290 | 2501.382 | 191672 | 89948.291 | 2130.913 | 90.61 | 101744640 | 通过 |

### MCHPRS 原始结果

MCHPRS 输入保留完整 CPU 和程序结构. 其 parser 无法表示的 1 个 chest, 1 个 lectern 和 9 个孤立 moving piston 被映射为空气. 这些方块不在活动数据通路中. 除这 11 个方块外没有替换其他元件. 测试使用固定源码 commit 构建 headless adapter.

Hello-world:

| 后端 | 轮次 | load ms | compile ms | tick ms | 总 game tick/s | stable game tick | 稳定前 ms | 稳定前 game tick/s | total ms | wall s | RSS byte |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 普通 | 1 | 850.362 | - | 5343.892 | 1646.740 | 8686 | 5343.866 | 1625.415 | 6195.075 | 6.84 | 132923392 |
| 普通 | 2 | 761.634 | - | 4589.972 | 1917.223 | 8686 | 4589.953 | 1892.394 | 5352.935 | 5.37 | 134823936 |
| 普通 | 3 | 723.112 | - | 4554.286 | 1932.246 | 8686 | 4554.261 | 1907.225 | 5278.014 | 5.28 | 147439616 |
| 普通中位数 | - | 761.634 | - | 4589.972 | 1917.223 | 8686 | 4589.953 | 1892.394 | 5352.935 | 5.37 | 134823936 |
| Redpiler | 1 | 798.626 | 572.451 | 896.891 | 9811.675 | 8686 | 886.173 | 9801.703 | 2270.510 | 2.29 | 219578368 |
| Redpiler | 2 | 708.000 | 478.831 | 736.390 | 11950.183 | 8686 | 726.445 | 11956.859 | 1925.976 | 1.94 | 215252992 |
| Redpiler | 3 | 837.723 | 537.970 | 861.000 | 10220.672 | 8686 | 849.628 | 10223.299 | 2243.661 | 2.26 | 200671232 |
| Redpiler 中位数 | - | 798.626 | 537.970 | 861.000 | 10220.672 | 8686 | 849.628 | 10223.299 | 2243.661 | 2.26 | 215252992 |

Line-drawing:

| 后端 | 轮次 | load ms | compile ms | tick ms | 总 game tick/s | stable game tick | 稳定前 ms | 稳定前 game tick/s | total ms | wall s | RSS byte |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 普通 | 1 | 727.801 | - | 128668.250 | 1748.683 | 191670 | 128667.329 | 1489.656 | 129397.337 | 129.40 | 136675328 |
| 普通 | 2 | 736.614 | - | 124777.226 | 1803.214 | 191670 | 124776.357 | 1536.108 | 125515.390 | 125.52 | 134823936 |
| 普通 | 3 | 696.169 | - | 131268.019 | 1714.050 | 191670 | 131267.149 | 1460.152 | 131966.727 | 131.99 | 135888896 |
| 普通中位数 | - | 727.801 | - | 128668.250 | 1748.683 | 191670 | 128667.329 | 1489.656 | 129397.337 | 129.40 | 135888896 |
| Redpiler | 1 | 849.972 | 521.425 | 19300.466 | 11657.750 | 191670 | 15913.710 | 12044.332 | 20677.185 | 20.69 | 175046656 |
| Redpiler | 2 | 923.033 | 478.994 | 20034.210 | 11230.790 | 191670 | 17255.016 | 11108.074 | 21439.926 | 21.46 | 205111296 |
| Redpiler | 3 | 710.814 | 527.216 | 23817.182 | 9446.961 | 191670 | 20352.092 | 9417.705 | 25060.487 | 25.10 | 213729280 |
| Redpiler 中位数 | - | 849.972 | 521.425 | 20034.210 | 11230.790 | 191670 | 17255.016 | 11108.074 | 21439.926 | 21.46 | 205111296 |

全部 MCHPRS hello-world 运行通过 206 个 probe, 全部 line-drawing 运行通过 36 个 probe. Redpiler 的 `compile_ms` 不包含在 `tick_ms` 和 tick/s 中, 但包含在 `total_ms` 和 wall 中.

### Minecraft Java 26.1.2 原始结果

Hello-world:

| 轮次 | GameTest tick ms | tick/s | wall s | RSS byte | 最终 probe |
| ---: | ---: | ---: | ---: | ---: | --- |
| 1 | 32283.587 | 272.584 | 86.78 | 1956036608 | 与 Rust 一致 |
| 2 | 34159.140 | 257.618 | 88.42 | 1697398784 | 与 Rust 一致 |
| 3 | 38000.301 | 231.577 | 91.89 | 1781465088 | 与 Rust 一致 |
| 中位数 | 34159.140 | 257.618 | 88.42 | 1781465088 | 与 Rust 一致 |

每轮输出 1 行 `probe_samples_v1` header 和 206 行 tick 8800 probe. 三个 Java 输出文件字节一致. Rust JSONL 按结构化字段投影为 `tick`, `probe`, `value` 后与 Java 输出无差异.

Java GameTest 未加入稳定点检测, 因而不报告稳定前速度. 完整 line-drawing 需要原版执行 225000 tick, 本次不运行该长任务. 表格保留空项, 不发布 Java line-drawing 速度或相对倍数.

### 结果解释

Hello-world 的 `redstone-rs` 稳定前中位数为 2428.251 tick/s, MCHPRS 普通引擎为 1892.394 等价 game tick/s, Redpiler 为 10223.299 等价 game tick/s. Line-drawing 对应值分别为 2130.913, 1489.656 和 11108.074. Redpiler 更高的吞吐建立在预计算连接, `optimize` 和 `io_only` 上, 不能外推为完整规则模拟的通用领先幅度.

总 tick/s 在两个场景中都高于或接近稳定前速度, 原因是完整运行包含稳定后的空闲 tick. line-drawing 的差异最明显: `redstone-rs` 总速率为 2501.382 tick/s, 活动区间为 2130.913 tick/s. 如果只报告完整运行平均值, 会高估持续传播阶段的计算能力.

端到端 wall 同时受结构解析, 注册表初始化, MCHPRS 编译, JVM 启动和操作系统调度影响. tick 阶段适合观察稳态执行, wall 适合估计一次性工具调用成本, 两者不能互相替代.

### 通用性能分析规则

1. 正确性先于速度. 只有达到相同最终状态, 或通过逐 tick oracle 的实现才能进入对应速度表.
2. 统一 tick 定义. game tick, redstone tick, 子步和编译图求值必须显式换算, 原始单位也要保留.
3. 单线程和多线程分开报告. 本节比较单个 CLI, 单个 GameTest server thread 和单个 MCHPRS plot, 不把 plot 级并发吞吐混入单机器速度.
4. 初始化和稳态分离. 世界加载, 注册表初始化, JIT, 图编译和首轮缓存应单列, 不能混入 tick/s.
5. 活动区间和空闲尾部分离. 报告稳定点, 稳定前速度和完整运行速度, 避免空闲 tick 抬高复杂机器吞吐.
6. trace, replay 和网络分离. 正式吞吐关闭微轨迹和客户端网络, 另行测量观测与导出成本.
7. 微基准和真实机器并存. 元件微基准用于定位局部热路径, Frostbyte 这类大型 CPU 用于验证组合行为和缓存压力.
8. 保留原始值和异常轮次. 摘要使用中位数, 但不能静默删除调度抖动, 热降频或页面错误造成的慢轮次.
9. 不生成跨语义范围的单一总排名. 完整原版规则, 计算红石子集, 预编译连接图和纯三维可视化项目应分别解释.

## 动态编译执行器验收方法

### 第一阶段实现证据

最初的混合执行器在 `Simulation::load` 内立即构图. Frostbyte program paste 和 `update_region` 随后被当作动态拓扑逐项同步, 使 1 tick 的 8-bit CPU 缩短场景超过 30 s 仍无法完成. 执行器生命周期拆为 configure 和 prepare 后, CLI 在初始 initialize, paste 和 update 完成后只构图一次. 同一缩短场景的 compile 为 530.818 ms, tick 为 0.385 ms.

第二个数量级瓶颈位于状态同步后的图校验. 旧实现即使只有 wire power 属性变化, 也会扫描完整 node 和 dependency 索引. Frostbyte 图约有 68.9 万 node 和 643 万 dependency, 每个规则回调因此退化为 O(E). StateOnly 同步移除全图校验后, 8-bit CPU 的 60 tick 编译运行恢复为 24.234 ms. 同轮独立解释运行为 29.564 ms. 这个阶段只证明灾难性回归已经消除, 不作为最终吞吐结果.

逐 tick 差分随后发现 wire 批量固定点传播改变分支顺序并可能触发任务风暴. 该路径已经停用. 安全编译路径保留 core 邻居任务栈, 只使用预计算输入计划替代重复输入搜索. T 形 wire, wire-repeater-lamp, triple piston 和 flying machine 的完整 `WorldDelta` 差分通过.

Frostbyte hello-world 的安全路径诊断同时发现一项输入计划错误. Redstone block 被错误当作可经普通导体转发的 strong source, 会使隔着 wool 的 wire 凭空获得 15 强度. 移除该常量折叠后, 8800 tick 的解释和强制编译运行在每一 tick 的 `WorldDelta`, probe 和 pending scheduled tick 数量上完全一致. 该次图包含 688931 个 node 和 6436394 条 dependency, compile 为 3.231 s. 此结果只作为正确性 oracle, 高速加权网络仍需单独验收.

动态编译执行器的正式结果使用 release profile, 关闭 trace, VCD, replay 和 Java oracle. 每个场景启动 3 个独立进程, 保留全部原始输出, 并对每项指标分别取中位数. Frostbyte 基准统一使用以下命令:

```shell
just bench-frostbyte compiled 3
```

`compile_ms` 单列记录从世界状态生成执行图的耗时, 不计入 `ticking_elapsed_ms` 和 tick/s. 吞吐门槛使用 `active_ticks_per_second`, 避免稳定后的空闲尾部抬高结果. 两个 Frostbyte 场景都达到 `10000 game tick/s` 才算通过静态电路性能验收.

| 场景 | 轮次 | engine | compile ms | nodes | edges | stable tick | 稳定前 tick/s | 断言 |
| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | --- |
| hello-world | 1 | compiled | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 |
| hello-world | 2 | compiled | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 |
| hello-world | 3 | compiled | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 |
| hello-world 中位数 | - | compiled | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 |
| line-drawing | 1 | compiled | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 |
| line-drawing | 2 | compiled | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 |
| line-drawing | 3 | compiled | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 |
| line-drawing 中位数 | - | compiled | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 |

动态拓扑回归分别以 `interpreted` 和 `compiled` 运行 `flying-roof`, `flying-machine` 和 `piston-gate-3x3`. 每个组合同样独立运行 3 次, 比较完整场景的 `ticks_per_second`, 局部重编译次数和重编译节点数. 编译模式耗时中位数不得超过同版本解释模式的 `110%`.

| 场景 | interpreted tick/s 中位数 | compiled tick/s 中位数 | compiled / interpreted 耗时 | topology rebuilds | recompiled nodes | 断言 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| flying-roof | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 |
| flying-machine | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 |
| piston-gate-3x3 | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 |
