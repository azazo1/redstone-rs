# TOML 场景编写手册

TOML 场景用于描述一次可重复的红石仿真. 一个场景由结构文件, 仿真参数, 定时动作, 探针和断言组成. 当前场景格式只支持 Java `26.1.2`.

## 最小场景

```toml
version = "26.1.2"
mode = "default"
max_ticks = 20

[source]
path = "machine.litematic"

[[actions]]
tick = 1
type = "use_block"
pos = { x = 0, y = 1, z = 0 }

[[probes]]
name = "lamp"
type = "property"
pos = { x = 0, y = 3, z = 0 }
property = "lit"

[[expectations]]
tick = 20
probe = "lamp"
equals = "true"
```

`source.path` 相对于场景 TOML 所在目录解析. 上例会在第 1 游戏刻开始前右键方块, 每刻结束后读取红石灯的 `lit` 属性, 并在运行结束时检查第 20 刻的值.

## 顶层字段

| 字段 | 类型 | 必填 | 默认值 | 说明 |
| --- | --- | --- | --- | --- |
| `version` | 字符串 | 是 | 无 | 必须为 `"26.1.2"` |
| `mode` | 字符串 | 是 | 无 | `"default"` 或 `"experimental"` |
| `seed` | 非负整数 | 否 | `0` | 场景随机种子 |
| `max_ticks` | 非负整数 | 否 | `100` | 仿真运行到的最终游戏刻 |
| `strict` | 布尔值 | 否 | `true` | 是否拒绝包含未实现主动行为的结构 |
| `oracle_micro_trace` | 布尔值 | 否 | `false` | 仅供 Java oracle 使用, 开启微时序采样 |
| `skip_oracle` | 布尔值 | 否 | `false` | 使用 `test --oracle` 时仅跳过 Java 对照 |
| `environment` | 表 | 否 | 默认 Overworld 环境 | 初始时间和基础天空光设置 |
| `monitor` | 表 | 否 | 立即监视 | trace, probe 和 oracle 的监视起点 |
| `replay` | 表 | 否 | 自动配置 | Replay Mod 时间轴和初始摄像头设置 |
| `source` | 表 | 是 | 无 | 输入结构和装载方式 |
| `actions` | 表数组 | 否 | 空 | 定时执行的操作 |
| `probes` | 表数组 | 否 | 空 | 每刻结束后采样的观测项 |
| `expectations` | 表数组 | 否 | 空 | 对指定 tick 探针值的断言 |

`max_ticks = 0` 不会执行任何游戏刻. 动作 tick 应从 `1` 开始. 超过 `max_ticks` 的动作不会执行, 对应的断言也会因为没有样本而失败.

`skip_oracle = true` 不会跳过 Rust 仿真和断言. 当命令启用 `--oracle` 时, CLI 会通过 `tracing` 记录该场景已跳过 Java 对照, 并按普通 Rust 场景测试处理结果. 该字段适合包含无法与完整 Java 服务端后台随机流对齐的装置.

同一 tick 的动作按 TOML 中的声明顺序执行. 动作发生在该 tick 的 `pre_tick` 阶段, 然后才执行计划方块刻, 方块事件, 实体刻和方块实体刻. 探针在 `post_tick` 阶段采样.

## 环境时间和天空光

```toml
[environment]
game_time = 8
overworld_time = 148
advance_time = true
sky_light = 15
```

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `game_time` | 非负整数 | `0` | 计划刻和绝对刷新周期使用的游戏时间 |
| `overworld_time` | 非负整数 | `0` | Overworld 的 24000 tick 天空时间线位置 |
| `advance_time` | 布尔值 | `true` | 是否在每个场景 tick 推进 Overworld 时钟 |
| `sky_light` | `0..=15` 整数 | `15` | 日光探测器位置的原始天空光 |

时间在每个场景 tick 开始时推进. tick 1 使用初始 `game_time + 1`, 并在 `advance_time = true` 时使用初始 `overworld_time + 1`. `game_time` 始终推进, 因此日光探测器只在绝对 `game_time % 20 == 0` 时刷新. 时间溢出会终止仿真并返回错误.

`sky_light` 是基础环境值, 不包含方块遮挡和光照传播. 日光探测器方块实体中的 `sky_signal` 是单个探测器的原始天空光覆盖值, 存在时优先于环境值. 当前 Java oracle 只支持 `sky_light = 15`; 其他值使用 `--oracle` 时会明确报错.

## 延迟监视

```toml
[monitor]
skip_ticks = 100
```

`skip_ticks` 是非负整数, 默认值为 `0`. 默认配置从场景初始化阶段开始记录, 因而 trace 可以包含 tick 0 事件. 当值为 N 且 N > 0 时, 初始化和 tick 1..N 仍会完整执行, 但 trace, VCD, probe 采样和 Java oracle 输出均从 tick N+1 开始.

`tick <= skip_ticks` 的 assertions 不会执行, CLI 会聚合输出一条 warning, 包含忽略数量和 tick 范围. `skip_ticks >= max_ticks` 合法, 此时 JSONL trace 为空, VCD 不含探针信号, 所有 assertions 均被忽略. Replay 使用独立的世界事件记录, 不受监视起点影响, 其导出区间只由 `[replay]` 的时间轴字段控制.

## Replay 时间轴

```toml
[replay]
start_tick = 20
end_tick = 80
duration_ms = 1500
```

`[replay]` 只影响 `run --replay` 和单场景 `test --replay` 生成的录像. 三个时间轴字段均可省略.

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `start_tick` | 非负整数 | `0` | 导出源区间的起始 tick |
| `end_tick` | 非负整数 | `max_ticks` | 导出源区间的结束 tick |
| `duration_ms` | 非负整数 | `(end_tick - start_tick) * 50` | 导出录像播放条的目标毫秒数 |

`start_tick..=end_tick` 是包含首尾的闭区间. 剪切后的 `start_tick` 固定映射到 Replay 时间轴 0, `end_tick` 映射到 `duration_ms`. 中间源 tick 按完整区间比例映射并四舍五入到毫秒. 压缩后的多个事件可以落在同一毫秒, 此时仍保持原始事件顺序.

tick 0 表示初始化完成但 tick 1 尚未执行的世界. 当 `start_tick > 0` 时, writer 会在该 tick 完成后创建完整世界快照, 因而录像首帧表示该 tick 的 post-tick 状态. 该 tick 内的瞬时 block event 不会重新播放, 但最终方块和方块实体状态会进入快照. `end_tick` 的执行结果包含在录像中, 后续仿真仍会运行到 `max_ticks`, 不影响断言, trace, VCD 或 Java oracle.

配置必须满足 `start_tick <= end_tick <= max_ticks`. 非空源区间要求 `duration_ms > 0`. 当起止 tick 相同时, 只能省略 `duration_ms` 或将其设为 0, 此时导出零时长静态快照. 目标时长还必须处于 MCPR 的 `i32` 毫秒时间戳范围内.

## Replay 初始摄像头

```toml
[replay.camera]
view_distance = 8
position = [12.5, 20.0, -6.5]
yaw = 135.0
pitch = 35.0
```

省略 `[replay.camera]` 时使用自动取景.

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `view_distance` | `2..=32` 整数 | `8` | 自动摄像头所依据的 Replay 播放视距, 单位为 chunk |
| `position` | `[x, y, z]` | 自动 | 初始摄像头世界坐标 |
| `yaw` | 浮点数 | 自动 | 水平观察角度, Minecraft 角度制 |
| `pitch` | `-90..=90` 浮点数 | 自动 | 垂直观察角度, 正值向下 |

`yaw` 和 `pitch` 必须同时设置. 只设置 `position` 时会从该位置自动朝向机器中心. 只设置完整的 `yaw` 和 `pitch` 时会自动选择位置, 然后使用指定角度. 手动位置不会被自动视距预算裁剪.

自动焦点使用录像开始时实际存在的非空气方块边界, 因此未来 tick 才粘贴的远端结构不会拉偏初始摄像头. 摄像头优先保持在较近距离, 不要求把大型机器完整放入首帧. 水平偏移会按 `view_distance` 预留一个 chunk 后限制, 避免自动机位落到预期播放视距之外.

当最薄尺寸至少比第二薄尺寸小 3 倍时, 结构会被视为明显扁平. 水平平面主要从上方观察, 竖直平面主要沿最薄轴正面观察. 线状结构和普通立体结构继续使用斜上方视角.

自动取景会对候选观察面评分. 被断言引用的方块探针权重最高, 其他方块探针次之, 初始世界中的红石灯和铜灯提供较低权重. 这使摄像头倾向于朝向观测结果和输出元件较集中的一面. 没有足够提示或两面得分相同时使用稳定的默认侧.

录像仍会按内容需要扩大服务端 chunk cache radius, 不会按 `view_distance` 裁剪远端录像数据. 播放端最终有效渲染距离还会受到本地视频设置限制.

## 结构来源

```toml
[source]
path = "../schematics/machine.nbt"
origin = { x = 10, y = 64, z = -20 }
initialization = "notify"
rotation = "clockwise90"
mirror = "none"
```

支持 `.litematic`, `.schem`, `.nbt` 和 `.structure` 文件.

Java 26.1.2 世界目录或 ZIP 也可以直接作为 `source.path` 或 paste 路径. ZIP 直接在内存中读取 entry, 不创建中间解压目录, 并支持存档散放在根目录或包含一个顶层文件夹. 当前固定读取 `minecraft:overworld`. 普通世界必须声明有限 `region`, 严格虚空世界可以省略:

```toml
[source]
path = "../worlds/redstone-lab"
initialization = "raw"
region = { min = { x = -128, y = -64, z = -128 }, max = { x = 127, y = 319, z = 127 } }
```

`region` 使用包含首尾坐标的闭区间. 它在 `origin`, rotation 和 mirror 之前应用于存档绝对坐标. 世界导入保留方块状态, 方块实体 NBT 和普通实体, 但不导入 scheduled ticks, POI, biome, lighting, 玩家数据或世界规则.

旧版 chunk 应先在 Minecraft Java 26.1.2 的世界编辑界面执行 `优化世界`, 等待全部 region 处理完成. 仅在使用 `redstone convert` 准备场景且允许丢失旧版 region 时, 才使用 `--skip-old-regions`; 命令会为每个跳过的 `.mca` 输出 `WARN`.

### 粘贴附加结构

主结构加载完成后, 可以按顺序粘贴 ROM 或其他附加结构:

```toml
[source]
path = "computer.schem"
initialization = "raw"

[[source.pastes]]
path = "rom.schem"
tick = 20
origin = { x = 10, y = 64, z = -20 }
rotation = "none"
mirror = "none"
ignore_air = true
paste_entities = false
update = true
```

每个 `source.pastes` 都使用自己的 `origin`, `rotation` 和 `mirror`. `tick` 省略时在仿真开始前粘贴, 设置为 `1..=max_ticks` 时在指定 tick 的 PreTick 阶段粘贴. 同 tick 中, 结构粘贴和选区更新先于普通 `actions`, 计划刻及方块事件执行, 探针在这些操作全部完成后采样.

`ignore_air = true` 对应 WorldEdit 的 `//paste -a`, 保留目标区域中与剪贴板空气重叠的现有方块. `paste_entities = true` 对应 `//paste -e`, 默认不粘贴实体.

附加结构的完整变换后区域会作为 pasted selection, 不会因为 `ignore_air` 而缩小. `update = true` 会在该次粘贴后立即对这个选区应用方块形状刷新和邻居更新, 对应 `//paste -as` 后执行 `//update`. 多个 `source.pastes` 严格按声明顺序粘贴和更新, 路径相对于场景 TOML 文件解析.

| 字段 | 可选值 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `path` | 文件路径 | 无 | 相对于场景文件所在目录 |
| `origin` | `{ x, y, z }` | 原点 | 结构变换后的世界原点 |
| `initialization` | `"notify"`, `"raw"` | `"notify"` | 是否在装载后执行初始化更新 |
| `rotation` | `"none"`, `"clockwise90"`, `"clockwise180"`, `"counterclockwise90"` | `"none"` | 绕 Y 轴旋转结构 |
| `mirror` | `"none"`, `"left_right"`, `"front_back"` | `"none"` | 镜像结构 |
| `region` | `{ min = { x, y, z }, max = { x, y, z } }` | 严格虚空世界自动确定 | 世界目录读取范围, 普通世界必须提供 |

变换顺序为先镜像, 再旋转, 最后加上 `origin`. `left_right` 翻转 Z 轴, `front_back` 翻转 X 轴.

`notify` 会在首个游戏刻之前执行形状修复和邻居通知, 适合模拟 structure 正常放置后的状态. `raw` 会原样保留导入状态且不触发初始化更新, 适合精确控制初始条件.

动作和探针使用变换完成后的绝对世界坐标. 它们不会再自动套用 `source` 的旋转, 镜像或原点偏移.

## 动作

每个 `[[actions]]` 必须包含 `tick` 和 `type`. 方块位置统一写成 `pos = { x = 0, y = 0, z = 0 }`.

### 放置和破坏方块

```toml
[[actions]]
tick = 1
type = "set_block"
pos = { x = 2, y = 1, z = 0 }
name = "minecraft:repeater"
properties = { facing = "east", delay = "2", locked = "false", powered = "false" }

[[actions]]
tick = 10
type = "break_block"
pos = { x = 2, y = 1, z = 0 }
```

`set_block` 的 `name` 使用完整方块 ID. `properties` 可省略, 属性名和值都必须写成字符串. 不存在的方块或非法属性会使场景加载失败.

### 使用方块

```toml
[[actions]]
tick = 1
type = "use_block"
pos = { x = 0, y = 1, z = 0 }

[[actions]]
tick = 2
type = "press_button"
pos = { x = 1, y = 1, z = 0 }

[[actions]]
tick = 3
type = "pull_lever"
pos = { x = 2, y = 1, z = 0 }
```

`use_block` 表示普通玩家右键. `press_button` 和 `pull_lever` 是带类型校验的专用动作, 目标不是对应方块时会报错.

### 修改方块实体

```toml
[[actions]]
tick = 1
type = "set_block_entity"
pos = { x = 0, y = 1, z = 0 }
data = { kind = "minecraft:dropper", fields = { item_count = 1, inventory = [{ slot = 0, item_id = "minecraft:stone", count = 1 }] } }
```

`data.kind` 是方块实体类型. `data.fields` 接受 TOML 可表达的布尔值, 整数, 浮点数, 字符串, 数组和内联表. 具体字段取决于当前最小方块实体模型.

### 生成和控制实体

```toml
[[actions]]
tick = 1
type = "spawn_entity"
id = 20
kind = "minecraft:item"
position = [4.5, 2.0, 0.5]
fields = { item_id = "minecraft:stone", item_count = 2, pickup_delay = 10, no_gravity = true }

[[actions]]
tick = 2
type = "move_entity"
id = 20
position = [5.5, 2.0, 0.5]

[[actions]]
tick = 3
type = "set_entity_field"
id = 20
field = "item_count"
value = 4

[[actions]]
tick = 4
type = "remove_entity"
id = 20
```

`spawn_entity.id` 可省略, 此时仿真器自动分配 ID. 后续动作或探针需要引用实体时, 应显式指定唯一 ID. `position` 使用 `[x, y, z]` 浮点坐标.

### 命中目标方块

```toml
[[actions]]
tick = 1
type = "hit_target"
pos = { x = 0, y = 0, z = 0 }
face = "north"
location = [0.9, 0.5, 0.0]
arrow = false
```

`face` 可取 `west`, `east`, `down`, `up`, `north`, `south`. `location` 是命中点的世界坐标. `arrow` 默认为 `false`, 设为 `true` 时模拟箭命中.

## 探针

探针在每个游戏刻结束后采样. `name` 在一个场景中应保持唯一, 断言通过该名称引用探针.

### 信号强度

```toml
[[probes]]
name = "rear_signal"
type = "signal"
pos = { x = 4, y = 1, z = 0 }
direction = "west"
```

返回 `0` 到 `15` 的整数. `direction` 可省略, 省略时读取该位置可输出的最大信号.

### 方块状态和属性

```toml
[[probes]]
name = "lamp_lit"
type = "property"
pos = { x = 0, y = 3, z = 0 }
property = "lit"

[[probes]]
name = "lamp_state"
type = "block_state"
pos = { x = 0, y = 3, z = 0 }
```

`property` 返回字符串, 属性不存在时返回空值. 因此布尔方块属性的断言也要写成字符串, 例如 `equals = "true"`. `block_state` 返回内部状态 ID, 该 ID 适合轨迹比较, 不适合作为跨版本或手写场景中的稳定常量.

### 容器和实体

```toml
[[probes]]
name = "hopper_items"
type = "container_count"
pos = { x = 0, y = 1, z = 0 }

[[probes]]
name = "items"
type = "entity_count"
kind = "minecraft:item"

[[probes]]
name = "all_entities"
type = "entity_count"

[[probes]]
name = "cart_enabled"
type = "entity_field"
id = 30
field = "enabled"

[[probes]]
name = "cart_items"
type = "entity_container_count"
id = 30
```

`container_count` 和 `entity_container_count` 返回物品总数, 不是已占用槽位数. `entity_count.kind` 可省略, 省略后统计所有实体. `entity_field` 返回字段值, 实体或字段不存在时返回空值.

### 事件计数

```toml
[[probes]]
name = "notes_played"
type = "event_count"
kind = "note_block_play"
```

事件计数是从场景开始累计的整数. 可用事件名称取决于已实现的方块行为, 例如 `note_block_play`, `bell_ring`, `dropper_transfer`, `dropper_eject`.

## 断言

```toml
[[expectations]]
tick = 20
probe = "lamp_lit"
equals = "true"

[[expectations]]
tick = 20
probe = "hopper_items"
equals = 3

[[expectations]]
tick = 20
probe = "cart_enabled"
equals = false
```

`tick` 必须在 `1..=max_ticks` 内, `probe` 必须与某个探针名称完全一致. `equals` 支持布尔值, 整数和字符串, 类型必须与探针返回值一致. 特别注意, `property` 始终返回字符串, 而 `signal`, 数量和事件计数返回整数.

场景会完整运行到 `max_ticks`, 然后统一检查断言. 任意断言失败都会让命令以失败状态退出, 并输出期望值和实际值.

## 选择器示例

下面的模板右键一个音符盒, 等待机器稳定, 然后确认对应红石灯点亮且另一盏灯熄灭. `path` 和坐标需要按实际结构调整. 如果机器依赖容器中的预置物品, 结构文件或更早的 `set_block_entity` 动作还必须提供对应库存.

```toml
version = "26.1.2"
mode = "default"
seed = 0
max_ticks = 24
strict = true

[source]
path = "machine.litematic"
origin = { x = 0, y = 0, z = 0 }
initialization = "notify"
rotation = "none"
mirror = "none"

[[actions]]
tick = 1
type = "use_block"
pos = { x = 0, y = 2, z = 0 }

[[probes]]
name = "selected_lamp"
type = "property"
pos = { x = 0, y = 4, z = 0 }
property = "lit"

[[probes]]
name = "other_lamp"
type = "property"
pos = { x = 1, y = 4, z = 0 }
property = "lit"

[[expectations]]
tick = 24
probe = "selected_lamp"
equals = "true"

[[expectations]]
tick = 24
probe = "other_lamp"
equals = "false"
```

完整的 one-hot 断言应为每盏灯各添加一个 `property` 探针, 对目标灯断言 `"true"`, 对其余灯逐一断言 `"false"`. 多个选择输入可以拆成多个场景文件, 交给 `redstone test` 并行执行, 这样失败时更容易定位具体输入.

## 运行场景

项目提供了 `just` recipe:

```shell
just run path/to/scenario.toml
just run path/to/scenario.toml --trace output.jsonl --vcd output.vcd
```

也可以直接运行 CLI:

```shell
cargo run -p redstone-cli -- run path/to/scenario.toml
cargo run -p redstone-cli -- trace path/to/scenario.toml --output output.jsonl --vcd output.vcd
cargo run -p redstone-cli -- test path/to/scenarios
cargo run -p redstone-cli -- test path/to/scenarios --oracle
```

`run` 执行单个场景. `trace` 强制输出 JSONL 轨迹. `test` 接受单个 TOML 文件或目录, 目录中的场景按文件名稳定排序并并行执行. `--oracle` 还会与测试专用 Java 参考实现比较轨迹, 使用前需要配置 `REDSTONE_ORACLE`.

严格模式遇到未实现的主动方块时会拒绝启动. 临时检查纯静态结构时, 可以使用 `--allow-static-fallback` 覆盖场景中的 `strict = true`:

```shell
cargo run -p redstone-cli -- run path/to/scenario.toml --allow-static-fallback
```

## 常见问题

- TOML 中方块属性值需要加引号, 例如 `powered = "false"`.
- `property` 探针的布尔结果是字符串, 断言应写 `equals = "true"`.
- 动作和探针坐标是最终世界坐标, 不会随结构再次变换.
- 第 0 刻动作不会执行, 定时动作应从第 1 刻开始.
- `raw` 不会修复结构形状或通知邻居, 初始红石状态可能保持文件中的旧值.
- Litematic 不保存可恢复的计划方块刻, 初始计划刻只能由初始化更新或场景动作产生.
- 省略实体 ID 后无法在 TOML 中预先知道自动分配值, 需要后续引用时请显式设置 ID.
- 新于 Java `26.1.2` 的 structure DataVersion 会被拒绝.
