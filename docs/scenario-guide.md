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
| `source` | 表 | 是 | 无 | 输入结构和装载方式 |
| `actions` | 表数组 | 否 | 空 | 定时执行的操作 |
| `probes` | 表数组 | 否 | 空 | 每刻结束后采样的观测项 |
| `expectations` | 表数组 | 否 | 空 | 对指定 tick 探针值的断言 |

`max_ticks = 0` 不会执行任何游戏刻. 动作 tick 应从 `1` 开始. 超过 `max_ticks` 的动作不会执行, 对应的断言也会因为没有样本而失败.

同一 tick 的动作按 TOML 中的声明顺序执行. 动作发生在该 tick 的 `pre_tick` 阶段, 然后才执行计划方块刻, 方块事件, 实体刻和方块实体刻. 探针在 `post_tick` 阶段采样.

## 结构来源

```toml
[source]
path = "../schematics/machine.nbt"
origin = { x = 10, y = 64, z = -20 }
initialization = "notify"
rotation = "clockwise90"
mirror = "none"
```

支持 `.litematic`, `.nbt` 和 `.structure` 文件.

| 字段 | 可选值 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `path` | 文件路径 | 无 | 相对于场景文件所在目录 |
| `origin` | `{ x, y, z }` | 原点 | 结构变换后的世界原点 |
| `initialization` | `"notify"`, `"raw"` | `"notify"` | 是否在装载后执行初始化更新 |
| `rotation` | `"none"`, `"clockwise90"`, `"clockwise180"`, `"counterclockwise90"` | `"none"` | 绕 Y 轴旋转结构 |
| `mirror` | `"none"`, `"left_right"`, `"front_back"` | `"none"` | 镜像结构 |

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
