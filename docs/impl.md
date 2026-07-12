# 已实现功能

本文以当前源码和测试为准, 说明 `redstone-rs` 已经具备的能力及其实现边界. 路线图和未完成任务另见 [todo.md](todo.md), 场景字段的使用方法另见 [scenario-guide.md](scenario-guide.md).

## 版本与系统边界

- 规则目标固定为 Minecraft Java `26.1.2`.
- 世界存档目标 DataVersion 为 `4790`.
- Replay 网络协议号为 `775`, MCPR 格式版本为 `14`.
- Rust 仿真和 CLI 正常运行不需要 JVM. Java oracle 只用于开发和 CI 差分测试.
- 仿真步进 API 本身是同步的. 异步集成由 Tokio delta 广播, CLI 并行场景和 Java oracle 子进程编排提供.

## Workspace 分层

| crate | 已实现职责 |
| --- | --- |
| `redstone-core` | 稀疏世界, tick 阶段, 计划刻, 方块事件, 邻居更新, 实体索引, 探针, delta 和轨迹 |
| `redstone-java-26` | Java `26.1.2` 方块状态注册表, 支撑面数据, 红石与相关方块规则 |
| `redstone-io` | TOML 场景, Litematic, Sponge schematic, vanilla structure 读写和 Minecraft 世界读取 |
| `redstone-replay-26` | Minecraft `26.1.2` 网络包编码和 Replay Mod MCPR 生成 |
| `redstone-cli` | `inspect`, `run`, `test`, `trace`, `bench`, `convert` 命令和 Java oracle 编排 |

## 仿真内核

### 世界存储

- 方块世界按 `16x16x16` section 组织, 每个 section 使用本地 palette 和 `u16` 索引.
- 纯空气 section 不分配, section 恢复为全空气后会释放.
- 默认使用有序稀疏 section map. 当区域足够紧凑时, 加载阶段会切换为稠密 section slot 数组, 超出稠密边界的新 section 仍回退到稀疏 map.
- 负坐标使用欧几里得 section 划分, 与 Java 方块坐标语义一致.
- 方块实体保留注册顺序, 用于稳定的方块实体 tick.
- 实体使用稳定 `EntityId` 和 section 级空间索引. 生成, 移动和删除会同步更新索引, 可按 AABB 查询实体.

主要实现位于 `crates/redstone-core/src/world.rs`.

### tick 阶段与事件顺序

每个游戏 tick 依次执行:

1. `PreTick`: 定时 paste 和场景 action.
2. `ScheduledTicks`: 已到期的计划方块刻.
3. `BlockEvents`: 有序方块事件, 当前主要由活塞使用.
4. `Entities`: 最小实体逻辑与实体传感器.
5. `BlockEntities`: 按注册顺序执行需要 tick 的方块实体.
6. `PostTick`: 探针采样与断言数据收集.

具体顺序特性:

- 计划刻按 `trigger_tick`, 优先级, `sub_tick_order`, 坐标和方块类型排序.
- 同一位置和方块类型的计划刻在执行开始前去重. 计划刻执行中新增的同 tick 任务留到下一个游戏 tick.
- 计划刻使用 256 槽环形时间轮和远期 overflow 队列. 时间轮保留 7 级优先级, `sub_tick_order`, 去重和同 tick 延迟语义.
- 方块事件使用有序队列和集合去重. 成功执行的方块事件在该事件产生的世界变化之前进入 delta.
- 邻居更新使用显式任务栈, 不依赖 Rust 调用栈递归. 嵌套更新会在外层六方向广播继续前抢占执行, 固定方向顺序为 west, east, down, up, north, south.
- 规则可将计划刻, 批量方块变化和后续任务延迟到当前同步邻居链结束后执行.
- 单 tick 计划刻和连锁邻居更新都有可配置上限, 达到上限时记录 warning.
- `game_time` 每 tick 推进. `overworld_time` 可独立暂停, 两者在 tick callback 前更新并检查溢出.

主要实现位于 `crates/redstone-core/src/simulation.rs`, `event.rs` 和 `rules.rs`.

### 混合编译执行器

公共执行策略由 `ExecutionMode` 和 `SimulationConfig.execution_mode` 选择. 默认模式为 `Auto`, 场景 TOML 不保存该策略.

- `Auto` 尝试构建编译图. 诊断模式不兼容时通过 `tracing::warn` 记录原因并使用解释器. 初始编译或动态拓扑同步失败时记录原因并永久切换到解释器.
- `Interpreted` 始终走 Java 26.1.2 规则解释路径.
- `Compiled` 强制构建和使用编译图. 遇到 trace, VCD, Java oracle, experimental redstone 或无法安全编译和同步的状态时返回明确错误, 不静默回退.
- trace, VCD 和 Java oracle 需要完整微事件顺序, experimental redstone 有独立更新顺序, 因而 `Auto` 在这些模式下实际使用 `Interpreted`.
- Replay MCPR 由有序 `WorldDelta` 驱动. Replay 事件记录本身不会触发诊断回退, 因而可继续使用 `Compiled`.

编译器只加速电气传播. `SparseWorld`, 计划刻, 方块事件和活塞规则仍是权威状态, 普通活塞, 黏性活塞, QC, zero-tick, 观察者及移动红石元件继续由 Java 26.1.2 规则处理. 编译节点的计划刻仍由 core 调度器按位置和方块类型调度.

每次规则回调都会在内部收集有序 `BlockChange`. 回调结束后, 下一项邻居任务执行前存在同步屏障, 用于把最新世界状态和结构变化同步到编译拓扑. 普通 power 变化只更新节点状态, 类型, 朝向, wire 连接, 导体性质和 moving piston 状态等结构变化才使拓扑失效. 动态结构优先局部重编译, 规模过大时全量重编译. 局部同步失败后, `Auto` 恢复解释器缓存并永久回退, `Compiled` 返回错误, 两者都不会继续使用损坏的图.

`Simulation::execution_report()` 返回当前 `ExecutionReport` 快照, 包含请求模式, 实际后端, 回退原因, 编译耗时, 节点和边数量, 编译/解释更新数, 局部/全量重编译次数, 重编译节点数和重编译耗时. `compiled_hit_rate()` 根据两类更新数计算命中率.

主要实现位于 `crates/redstone-core/src/execution.rs`, `scheduler.rs` 和 `crates/redstone-java-26/src/compiled`.

### action, probe, delta 和轨迹

内核 action 已覆盖:

- 设置和破坏方块.
- 使用方块, 按按钮, 拉动拉杆.
- 设置方块实体.
- 生成, 移动, 删除实体和修改实体字段.
- 按命中面和局部命中位置触发标靶.

探针已覆盖:

- 六向最大信号或指定方向信号.
- 官方方块状态 ID 和单个 property.
- 方块容器物品总数.
- 可按类型筛选的实体数量, 实体字段和实体容器物品总数.
- 命名规则事件的累计数量.

每个 `WorldDelta` 保留方块变化, 方块实体 create/update/remove, 成功方块事件和探针样本. 订阅者可通过 Tokio broadcast 异步接收 delta.

微轨迹可记录 action, 方块写入原因, 邻居更新, 计划刻入队与执行, 方块事件入队与执行, probe, unsupported trigger 和 message. 输出支持:

- 按事件一行一个 JSON 的 JSONL.
- 将 bool, 数值, 方块状态和可解析数字 property 编码为波形的 VCD.
- 相同输入, seed 和模式下字节级稳定的 JSONL 和 VCD.

## Java 26.1.2 方块状态与规则

### 官方状态注册表

- `Java26Registry` 使用官方 `blocks.json`, `registries.json` 和构建期生成的方块特征表.
- 状态名和 properties 解析为官方全局 ID. 支持从官方默认状态补全未提供的 properties, 并缓存单 property 状态转移.
- 生成数据覆盖 `29873` 个官方方块状态的红石导体属性, 完整碰撞属性, 漏斗阻挡标签和 `Full`, `Center`, `Rigid` 六方向支撑面.
- 方块还会被分类为 `Normal`, `Block`, `Destroy`, `PushOnly` 活塞反应.
- 能解析某个官方方块状态不等于已实现其主动行为. 没有进入显式行为族的方块以 `Static` 状态参与导电, 支撑, 活塞移动和比较器输出判定.

主要实现位于 `crates/redstone-java-26/src/registry.rs`.

### 信号传播与红石线

- 区分弱信号与直接信号. 红石导体可取相邻直接信号的最大值向外传递.
- 红石块恒定输出 15. 拉杆和按钮开启时向外弱充能, 并强充能所附着方块.
- 红石火把避开附着方向输出. 中继器, 比较器和观察者只从输出端输出.
- 压力板, 探测铁轨, 讲台和陷阱箱可向上直接充能. 标靶, 阳光探测器和铜灯泡不通过强充能穿透导体.
- 红石线从相邻非红石信号和同层, 上坡, 下坡线网取最大功率, 线间每格衰减 1, 并排除经导体回到自身的反馈.
- `Default` 模式复现 Java 风格的坐标 hash bucket 更新顺序.
- `Experimental` 模式使用 48 种 `Orientation`, 固定左侧偏置和分离的 turn-off/turn-on 队列. 未传入方向时使用 Java 兼容随机源选择初始方向.

### 基础元件和二极管

- 拉杆即时切换.
- 石质按钮保持 20 tick, 木质按钮保持 30 tick.
- 红石火把延迟 2 tick 切换. 同一位置在 60 tick 窗口内第 8 次熄灭会触发烧毁, 并在 160 tick 后重新检查.
- 中继器支持 1 到 4 档延迟, 即 2, 4, 6, 8 tick.
- 中继器只会被侧面中继器或比较器锁定. 普通信号源和强充能导体不会锁定.
- 中继器和比较器实现 Java tick priority 差异, 包括面向交叉二极管时的优先级提升.
- 比较器支持比较与减法模式, 2 tick 更新和独立输出缓存. 世界加载时可从 `comparator_output` 或 `OutputSignal` 恢复输出.
- 观察者只观察朝向前方的变化, 延迟 2 tick 开启并保持 2 tick. 放置重置, 激活状态移除, 活塞移动后落位和活塞头移除会进入其生命周期处理.

### 消费元件与模拟信号

- 红石灯通电立即点亮, 断电延迟 4 tick 熄灭.
- 铜灯泡仅在上升沿翻转 `lit`, 同步维护 `powered`. 点亮状态可被比较器读取.
- 活板门和栅栏门根据输入维护打开状态. 门同时检查上下两半并同步 `open` 和 `powered`.
- 动力铁轨和激活铁轨沿相同铁轨类型和轴向传播供电, 最远检查 8 段.
- 音符盒和钟仅在上升沿记录播放或敲响事件. 使用音符盒会让 note 在 `0..24` 间循环. 当前不模拟声音传播和乐器音色.
- 标靶根据命中面内离中心的距离输出 `1..15`. 普通命中保持 8 tick, 箭命中保持 20 tick.
- TNT 方块受电后被移除并记录 `tnt_primed`. 生成的 TNT 实体只倒数引信, 爆炸本身只记录 unsupported 事件.
- 阳光探测器在绝对 `game_time % 20 == 0` 时更新, 使用 Overworld 时间, 天空光和反相状态. 方块实体 `sky_signal` 可覆盖场景环境天空光.
- 陷阱箱使用方块实体 `open_count` 输出 `0..15`, 开启数变化时通知自身和下方邻居.
- 使用讲台会产生 2 tick 的 15 强度脉冲.

### 比较器模拟输出

比较器可直接读取或隔一个导体读取下列信号源:

- 状态型: 各类炼药锅, 堆肥桶, 蛋糕与蜡烛蛋糕, 蜂巢与蜂箱, 末地传送门框架, 重生锚, 铜灯泡, 铜傀儡雕像姿态.
- 方块实体型: 雕纹书架, 命令方块, 合成器, `creaking_heart`, 唱片机, 讲台, 幽匿感测体, 校频幽匿感测体, 装饰陶罐和 shelf.
- 容器型: 箱子, 陷阱箱, 铜箱, 正确配对的双箱, 熔炉, 高炉, 烟熏炉, 木桶, 酿造台, 投掷器, 发射器, 漏斗和潜影盒.
- 实体型: 探测铁轨上的命令方块矿车或容器矿车.
- 展示框: 导体另一侧恰好有一个朝向匹配的物品展示框时, 有物品输出 `rotation % 8 + 1`, 空展示框输出 0.

容器信号使用原版填充率公式. 物品最大堆叠数来自生成的官方物品表, 也允许 `components.minecraft:max_stack_size` 覆盖. 显式 `comparator_output` 字段优先级最高并被截断到 `0..15`.

主要实现位于 `crates/redstone-java-26/src/rules/comparator.rs`.

### 实体传感器和最小实体模型

- 木质压力板可感应物品实体, 石质和磨制黑石压力板忽略物品实体.
- 轻质和重质测重压力板分别按 15 和 150 个实体的上限映射到 `1..15`. 压力板释放复查周期为 20 tick.
- 探测铁轨只感应 minecart 实体, 释放复查周期为 20 tick.
- 绊线感应任意实体, 释放复查周期为 10 tick. 实现沿方向最多 42 格搜索绊线并驱动朝向匹配的挂钩.
- 物品实体每 tick 增加 `age`, 将正数 `pickup_delay` 减到 0, 保持永不拾取值 `32767`, 并在 6000 tick 后移除.
- TNT 实体只递减 `fuse`, 到期后移除并记录 unsupported 爆炸.
- 漏斗矿车每个实体 tick 最多完成一次吸取, 可从上方容器取出 1 件, 或一次接收完整掉落物堆叠. 激活铁轨可切换其启用状态.
- 当前没有通用速度积分, 碰撞响应, 矿车轨道运动, 投射物飞行或玩家物理.

### 容器与物品转移

运行时库存使用 `inventory` 数组, 每个条目包含 `slot`, `item_id`, `count` 和可选 `components`.

- 漏斗的 `enabled` 在放置和邻居更新阶段立即同步. 受电锁定期间仍递减冷却并记录本 tick 执行时间.
- 未受电且冷却结束时, 漏斗先向朝向容器输出 1 件, 随后只要仍未装满就继续从上方容器或掉落物吸取. 推出和吸入可在同一 tick 同时成功.
- 上方存在方块容器或容器实体时不会回退到周期掉落物吸收. 无容器时使用原版吸取 AABB, 完整碰撞方块会阻挡吸取, 蜂巢和蜂箱除外.
- 物品实体进入漏斗碰撞区域时会在实体阶段触发传输. 掉落物按完整堆叠跨槽插入, 部分插入会保留实体余量和 components.
- 漏斗成功转移后进入 8 tick 冷却, 使用持久 tick 时间实现漏斗链的 7/8 tick 冷却修正.
- 方块容器, 正确配对的双箱和容器实体共用统一传输接口. 双箱按 `RIGHT` 半箱在前组成 54 个逻辑槽.
- 唱片机, 雕纹书架, 饰纹陶罐和堆肥桶实现专用槽位, 物品限制和必要的方块状态副作用.
- 同一传输调用内, 每个方块实体只提交一次最终库存和冷却数据. 比较器脏通知仍按实际发生的容器变化顺序执行.
- 熔炉, 高炉和烟熏炉实现顶部原料, 侧面燃料, 底部产物的槽位暴露规则.
- 酿造台实现顶部原料, 侧面药水瓶与燃料, 底部药水瓶的槽位规则, 并对瓶, 原料和烈焰粉做最小类型限制.
- 燃料, 酿造原料和堆肥概率由 Java oracle 从 26.1.2 官方运行时和物品标签生成, 不再依赖规则层手写集合.
- 合成器跳过禁用槽, 对同种物品使用简化的槽位均衡插入.
- 潜影盒拒绝嵌套潜影盒.
- 投掷器在上升沿后 4 tick 从随机非空栈取 1 件. 面向容器时尝试插入, 面向非容器时生成物品实体, 目标容器拒绝时保留原物品.
- 发射器可生成箭, 蛋, 雪球, 经验瓶, 药水, 烟花, 火焰弹和风弹等简化投射物实体, 也可点燃 TNT 和在合适铁轨上生成矿车.
- 未进入发射器显式行为表的物品不消耗, 并记录 `dispenser_item:<id>` unsupported.
- 合成器是最小模型. 它在上升沿后 4 tick 消耗第一个未禁用槽中的 1 件物品, 根据 `output_item_id` 和 `output_count` 输出, 并让 `crafting` 保持 1 tick. 当前未实现配方匹配, 九宫格消耗和剩余物.
- 带 LootTable 的容器会显式记录 unsupported, 并作为存在但不可传输的容器阻止错误回退. 当前不会展开战利品表.
- 漏斗矿车当前按静态实体每 tick 执行一次吸取. 沿轨道单 tick 多段移动时的重复吸取仍依赖尚未实现的矿车物理.

主要实现位于 `crates/redstone-java-26/src/rules/inventory.rs`.

### 活塞

- 普通活塞和黏性活塞支持伸出, 收回, 移动活塞头和移动方块的 2 tick 过渡状态.
- 支持准连接. 供电检查包含活塞本体除朝向面外的邻居, 以及活塞上方方块的全部邻居.
- 移动结构最多包含 12 个可移动方块, 并处理 `Normal`, `Block`, `Destroy`, `PushOnly` 四类反应.
- 黏液块和蜂蜜块会沿非推动轴吸附分支, 两者互不黏连, 分支与主线共享 12 方块上限.
- 釉陶属于 `PushOnly`, 可向前推动但不被黏性活塞拉回.
- 实现黏性活塞正常回拉, 伸出取消, 快速收回和 zero-tick 收回/丢块路径.
- 移动方块实体记录 moved state, 方向, 伸出状态, source 和 settle tick, 并在结算时恢复方块与邻居更新.
- 有官方方块实体类型的普通方块当前不可移动. 活塞反应也仍是项目内显式规则表, 不是完整原版 tag 体系.

主要实现位于 `crates/redstone-java-26/src/rules/piston.rs`.

### 支撑与连接形状

- 使用官方状态级支撑面数据判定红石线, 火把, 拉杆, 按钮, 压力板, 中继器, 比较器, 绊线钩, 铁轨和门的存活.
- 初始化, 放置, 移除和活塞移动后会修复红石线, 绊线, 栅栏, 玻璃板, 铁栏杆, 墙, 铁轨和门的连接 properties.
- 红石线可形成点, 直线和分支, 可连接中继器前后, 比较器水平侧面和观察者输出端, 并可沿完整支撑面, 活板门或漏斗上爬.
- 普通铁轨可形成弯道. 动力, 探测和激活铁轨只形成直线或斜坡. 修复会搜索同层, 上层和下层铁轨并处理三向连接优先级.
- 墙根据上方遮盖, 直线连接和相邻墙柱计算 `low`, `tall` 和中心柱 `up`.
- 这些是影响红石和连接属性的专用修复规则, 不是通用 voxel shape 或完整碰撞几何系统.

主要实现位于 `crates/redstone-java-26/src/rules/shape.rs`.

## 结构和世界 IO

### 统一加载模型

`StructureLoader` 将不同输入统一转换为 `LoadedStructure`. 结果包含 `SparseWorld`, 实际非空气边界, 声明 region 边界, 格式, DataVersion, 方块计数和原始方块实体 NBT 的 JSON 镜像.

已实现输入:

- Vanilla structure `.nbt` 和 `.structure`.
- Litematic `.litematic`.
- Sponge schematic `.schem`.
- Minecraft 世界目录和世界 ZIP.
- gzip 和未压缩的普通结构 NBT.

结构变换支持平移原点, 90/180/-90 度旋转, `left_right` 和 `front_back` 镜像. 变换同步应用于:

- 方块坐标和实体浮点坐标.
- `facing`, `axis`, 门 `hinge`, 楼梯/铁轨 `shape`.
- 以 north, east, south, west 命名的连接属性.
- 实体朝向与变换后包围盒.

方块实体和实体 NBT 会规范化为运行时字段, 包括库存, 物品总数, 槽位数, 容量, 冷却, 掉落物字段和展示框字段. 原始 NBT 仍保留给 inspect 和重新导出.

### 结构格式细节

- Vanilla structure 读写 palette, blocks, size, entities 和块内 NBT. 写出时可选完整空气区域或仅非空气方块.
- Litematic 可读取多个 Regions, 处理负 Size 语义, 稳定合并各 region, 并解码可跨 long 边界的 palette bit array. 写出为 gzip Litematic `v7.1` 和单个 `main` region.
- Sponge 可读取 v1, v2 和 v3, 处理旧 offset, WorldEdit offset, VarInt BlockData 和旧新方块实体布局. 写出为 gzip Sponge v3.
- 三种 writer 都保留方块, 方块实体和实体语义, 执行体积溢出检查, 显示编码进度, 并通过同目录临时文件, `sync_all` 和 rename 发布目标文件.

### Minecraft 世界读取

- 只读取 `dimensions/minecraft/overworld`.
- 世界设置和每个 chunk 都必须为 DataVersion `4790`.
- 普通世界必须提供有限闭区间 region. 只有可证明为 `the_void` flat, 全空气 layer, 无结构且无 lake 的世界可以省略 region.
- region, chunk, 方块, 方块实体和实体均按请求 region 裁剪.
- 如果世界有独立 `entities` region, 实体从该目录读取, 否则从 block chunk 的 entities 读取.
- ZIP 可将世界文件放在根目录或唯一的一层顶级目录. 实现会拒绝多个世界根, 更深前缀和不安全 entry 路径.
- Anvil region 支持 gzip, zlib, raw, Java LZ4 和外置 `.mcc` chunk, 并校验 header, sector 边界, chunk 长度和坐标溢出.
- `skip_old_regions` 预检所选 region. 任一 chunk 低于 `4790` 时警告并跳过整个 `.mca`, 不会部分合并该 region.
- 世界读取是只读的, 当前不写回存档和 chunk ticket.

主要实现位于 `crates/redstone-io/src/structure`.

## TOML 场景

场景可描述:

- 版本, 红石模式, seed, `max_ticks` 和 strict 模式.
- 初始 `game_time`, `overworld_time`, 时钟推进开关和 `sky_light`.
- `raw` 或 `notify` 初始化.
- 主结构的原点, 旋转, 镜像和世界 region.
- 仿真开始前或指定 tick 执行的附加结构 paste, 可控制 `ignore_air`, `paste_entities` 和 `update`. 逐块 update 同时执行形状更新和普通邻居更新.
- 同 tick 按声明顺序执行的 action.
- 每 tick `post_tick` 采样的 probe 和按 tick 检查的 expectation.
- `monitor.skip_ticks`, Replay 时间轴, Replay 摄像机, `oracle_micro_trace` 和 `skip_oracle`.

场景的 source 和 paste 相对路径均相对场景 TOML 所在目录解析. `monitor.skip_ticks` 之前仍会完整仿真, 但不记录 trace, VCD, probe 和 oracle 样本, 该区间的 expectation 会警告后忽略.

详细字段见 [scenario-guide.md](scenario-guide.md).

## CLI

### `inspect`

- 接受结构文件, 世界目录和世界 ZIP.
- 输出格式, DataVersion, 实际边界, region 边界, 非空气数量, section 数, 方块类型计数和未支持主动方块.
- 可按单个坐标, 闭区间 region, 方块 ID 或全部非空气方块查询.
- 明细包含官方 state ID, 完整 properties, supported 状态和原始方块实体 NBT, 支持 text 和 JSON.
- points 和 region 结果会合并, 去重并稳定排序. 无 namespace 的方块 ID 自动补 `minecraft:`.

### `run`, `trace`, `test` 和执行器

- `run`, `test`, `trace` 和 `bench` 接受 `--engine auto|interpreted|compiled`, 默认使用 `auto`.
- `run` 加载单个场景, 执行 paste 与 action, 采样 probe, 检查全部 expectation, 并输出 tick 数, 方块数, 轨迹数量, 耗时和 TPS.
- `run` 在最后一次外部 action 或定时 paste 之后检测稳定点. 连续 20 tick 没有 delta 事件且没有计划刻时, 输出 `stable_tick`, `active_ticking_elapsed_ms` 和 `active_ticks_per_second`, 从而把活动传播和稳定后的空闲尾部分开.
- `trace` 是强制写出 JSONL 的单场景入口, 可同时写 VCD. `run` 也支持可选 JSONL 和 VCD.
- `test` 接受单文件或目录. 目录模式仅扫描直接子文件中的 `.toml`, 按文件名排序, 使用系统可用并行度运行, 最后稳定汇总所有 PASS/FAIL.
- `--allow-static-fallback` 可在 CLI 层关闭场景 strict 检查.
- `test --oracle` 在 Rust 断言通过后调用 Java oracle 比较轨迹.
- 目录批量测试不支持共享 Replay 输出路径.
- 运行摘要输出请求模式, 实际后端, 回退原因, 编译耗时, 图规模, 编译命中率和动态重编译统计.

### `convert`

- 在 Litematic, Sponge v3 和 vanilla structure 之间转换, 也可将 Minecraft 世界有限 region 转为这三种结构格式.
- 场景 TOML 转换会建立独立 assets 目录, 将 source 和全部 paste 烘焙为 Java `26.1.2` vanilla structure, 并重写场景路径与已应用的变换.
- 输出格式在读取输入前校验. 当前不输出 Minecraft 世界目录.

### `bench`

- 构造指定方块数和活跃压力板数的立方世界.
- 输出世界构建时间, tick P50/P95/P99 和 Linux/macOS 常驻内存.
- 世界构建, 场景 tick, 批量测试, 世界 region, 结构写出, Replay 压缩和 Java oracle 都有 tracing 或进度输出.

## Replay Mod 导出

### 协议和区块

- 生成完整 Login -> Configuration -> Play 包序列, 包含 known pack, 28 组内嵌注册表, 必需 tags, feature flags, play login, 时间, 摄像机和初始区块.
- MCPR 归档包含 `recording.tmcpr`, 十进制 CRC32, `metaData.json` 和 `timelines.json`.
- 录像包时间始终按 `tick * 50 ms` 记录, 协议状态与 timestamp 单调性会显式校验.
- 区块编码覆盖 Overworld `Y=-64..319` 的 24 个 section, 支持单值, 局部 palette 和 15 bit 全局 palette. biome 固定为 plains, 天空光固定全亮, block light 为空.
- 初始源 region 和全部已占用区块向外扩 1 个区块作为渲染范围. 运行中变化进入未加载区域时, 先扩大缓存半径并发送目标周围的区块批次, 再发送更新.

### 增量事件和方块实体

- 方块变化编码为 block update.
- 方块实体 create/update 编码为 block entity data. remove 不单独写包, 由方块状态更新表达.
- 事件顺序与 `WorldDelta` 一致, 因此 moving piston 数据可紧跟在对应方块更新之后.
- `piston_animation` 可选将普通和黏性活塞的 block event 写入录像, 让客户端播放伸缩动画和声音.
- 支持 moving piston, 告示牌, 悬挂告示牌, 箱子, 桶, 漏斗, 熔炉, 合成器和潜影盒等网络方块实体 NBT.
- 未知方块实体类型, moving piston 缺少关键字段, 异构 NBT list 和过大 NBT 会明确失败.
- 当前 `WorldDelta` 不包含实体生成, 移动和删除事件, 因此 Replay 主要呈现方块和方块实体状态, 不是完整实体录像.

### 时间轴和摄像机

- `ReplayTimeline` 不改写底层 packet 时间. `timelines.json` 将源 `[start_tick,end_tick]` 线性映射到编辑 `duration_ms`, 用于 Replay Mod 内选段和变速.
- 静态区间只允许 `duration_ms=0`, 非空区间必须大于 0, tick 与时长都检查溢出.
- 当前 Position Path 的起止 pose 相同, 即生成固定机位, 不自动生成运镜.
- 自动取景使用实际非空气内容边界. 平面结构从薄轴观察, 普通体积使用斜上方候选位, 并按 expectation 引用的方块 probe, 其他 probe, 红石灯和铜灯泡的权重选择观察面.
- 可完全指定 position, 或同时指定 yaw 和 pitch. pitch 限制为 `-90..90`, view distance 限制为 `2..32`.

### 落盘语义

- 在目标同目录使用 `create_new` 建立 recording 和 archive 临时文件.
- recording flush 和 `sync_all` 后, 使用 256 KiB buffer 流式压缩进 ZIP. archive finish 和 `sync_all` 完成后才 rename 到目标.
- 成功后删除 recording 临时文件. 任意未 finish 的 writer 在 Drop 中清理两个临时文件.
- 断言在 Replay finish 之后检查, 因此断言失败仍保留已完成录像. 编码失败不会用半成品替换目标.

主要实现位于 `crates/redstone-replay-26/src`.

## Java vanilla oracle

### 执行模型

- oracle 验证 Minecraft `26.1.2` 和 DataVersion `4790`, 初始化 Bootstrap 与注册表.
- 主进程解析场景和 vanilla structure, 生成临时 pack, test instance 和 frame, 再启动带 `-javaagent` 的 GameTestServer 子进程.
- 支持 default 和 experimental 红石模式, 指定 seed, 场景时间, 暂停 Overworld 时钟和固定晴天. 当前 `sky_light` 只接受 15.
- 主结构支持非零 origin, `raw`/`notify`, 旋转和镜像. paste 支持初始或指定 tick, 并支持 `ignore_air`, `paste_entities` 和逐块 update.

### action 和 probe

- Java 场景可执行 Rust 场景中的全部 action 类型, 同 tick 按声明顺序执行.
- 实体可使用原版注册 EntityType. `minecraft:generic_collision` 映射为 armor stand 供传感器对照.
- 实体字段显式适配通用 `no_gravity`, 物品实体字段, 容器 inventory, 漏斗矿车 enabled, TNT fuse 和展示框物品/旋转. 其他字段可保存在 oracle 逻辑表中, 但不代表写入原版实体内部状态.
- 方块 probe 支持 signal, 官方 block state ID, property 和 container count.
- 实体 probe 支持可按 kind 筛选的 count, entity field 和 entity container count.

### 输出与 ASM 微轨迹

- bundled Java oracle 的普通模式输出 `probe_samples_v1`.
- `oracle_micro_trace=true` 时输出 `oracle_samples_v2`, 同一有序流中包含 probe, neighbor update, scheduled tick queued/executed, block event queued/executed 和 block changed.
- CLI 还兼容无 format marker 的第三方完整 `TraceEvent` JSONL oracle, 并执行字节级比较. bundled Java JAR 自身只产生 v1 和 v2.
- javaagent 在原版 `NeighborUpdater`, `GameTestServer`, `Level`, `LevelTicks` 和 `ServerLevel` 中注入 hook.
- 邻居更新记录真实执行顺序, `Orientation` 和 `moved_by_piston`.
- 计划刻记录相对 trigger tick, priority 和 `sub_tick_order`, 并分离记录入队与执行.
- 方块事件只在原版有序去重集合接受后记录入队, 执行样本记录事件参数.
- block changed hook 位于 `Level.setBlock` 入口, 记录 old/new 官方 ID. 该样本表示写入请求, 不应解读为每条都已成功改变最终世界.
- frame 覆盖主结构和全部 paste 的变换后边界并加 16 格 padding. 只采集同 Level 且 frame 内的事件.
- 当前尚未采集完整 shape update 内部轨迹, 实体内部步骤和信号求值调用栈.

### 自检和差分

- Java 工具提供 Bootstrap/registry 自检, GameTestServer 自检, 真实场景自检和全量方块特征报告.
- CLI 对 v1 比较逐 tick probe 序列.
- CLI 对 v2 同时比较全局微轨迹和每类子序列, 差异报告包含首个不一致样本及前后上下文.
- `skip_oracle=true` 仍执行 Rust 仿真和全部断言, 只跳过 Java 进程.
- 真实 Java 集成测试覆盖变换, notify, experimental seed, 实体, 标靶, 邻居顺序, 红石线顺序, 计划刻优先级, 活塞事件, 取消伸出, zero-tick, 黏性回拉, QC/BUD 和初始 paste. 这些用例需要先构建 Java oracle, 默认 Rust 测试中标记为 ignored.

主要实现位于 `tools/vanilla-oracle` 和 `crates/redstone-cli/src/main.rs`.

## 明确未覆盖的范围

以下范围不应从已实现功能中推断出来:

- 完整 Minecraft 世界模拟. 当前没有世界生成, 随机刻, 流体, 火传播, 爆炸改变世界, 生物 AI 和完整玩家物理.
- 完整实体与矿车物理. 实体系统主要服务于红石传感器, 展示框, 物品寿命, TNT 引信和漏斗矿车.
- 完整发射器物品行为, 完整合成器配方和剩余物, 完整物品 components/tag 语义.
- 通用 voxel shape 和精确实体碰撞几何. 现有 shape 逻辑只覆盖会影响红石, 支撑和连接 properties 的方块族.
- Minecraft 世界写回, 非 Overworld 维度, 旧版本数据修复和多版本兼容层.
- 保留源结构文件的所有容器形态与 metadata. 转换器目标是保留世界内容语义, Litematic 固定写为单 region, Sponge 固定写为 v3.
- Replay 中的完整实体时间线, 自动运镜, 原版动态光照和生物群系分区.
- Java oracle 的全量原版内部轨迹. 当前 ASM 范围专注于邻居更新, 计划刻, 方块事件和方块写入.

## 实现与测试索引

- 内核顺序与确定性: `crates/redstone-core/tests/engine.rs`.
- 红石元件和比较器: `crates/redstone-java-26/tests/components.rs`.
- 活塞大型场景: `crates/redstone-java-26/tests/tripple-piston-extender.rs`.
- 实体与矿车容器: `crates/redstone-java-26/tests/entities.rs`.
- 容器侧面规则和发射器: `crates/redstone-java-26/tests/inventory_sides.rs`, `dispenser.rs`.
- 连接形状与铁轨: `crates/redstone-java-26/tests/shapes.rs`, `rail_shapes.rs`.
- 结构格式与世界: `crates/redstone-io/tests/structure_*.rs`.
- CLI 场景, paste, inspect, convert, Replay 和 oracle: `crates/redstone-cli/tests`.
- Replay 协议和归档: `crates/redstone-replay-26/src` 中的单元测试.
