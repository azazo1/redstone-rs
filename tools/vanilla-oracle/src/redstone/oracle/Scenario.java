package redstone.oracle;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonNull;
import com.google.gson.JsonObject;
import com.google.gson.JsonPrimitive;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.tomlj.Toml;
import org.tomlj.TomlArray;
import org.tomlj.TomlParseResult;
import org.tomlj.TomlTable;
import net.minecraft.world.level.block.Mirror;
import net.minecraft.world.level.block.Rotation;

final class Scenario {
    static final String VERSION = "26.1.2";

    final Path path;
    final String mode;
    final long seed;
    final int maxTicks;
    final boolean oracleMicroTrace;
    final Environment environment;
    final Source source;
    final List<Action> actions;
    final List<Probe> probes;

    private Scenario(
        Path path,
        String mode,
        long seed,
        int maxTicks,
        boolean oracleMicroTrace,
        Environment environment,
        Source source,
        List<Action> actions,
        List<Probe> probes
    ) {
        this.path = path;
        this.mode = mode;
        this.seed = seed;
        this.maxTicks = maxTicks;
        this.oracleMicroTrace = oracleMicroTrace;
        this.environment = environment;
        this.source = source;
        this.actions = actions;
        this.probes = probes;
    }

    static Scenario load(Path path) throws IOException {
        Path normalized = path.toAbsolutePath().normalize();
        TomlParseResult document = Toml.parse(normalized);
        if (document.hasErrors()) {
            throw new IllegalArgumentException("TOML 解析失败: " + document.errors());
        }
        String version = requiredString(document, "version");
        if (!VERSION.equals(version)) {
            throw new IllegalArgumentException("场景版本必须是 " + VERSION + ", 收到 " + version);
        }
        String mode = requiredString(document, "mode");
        if (!mode.equals("default") && !mode.equals("experimental")) {
            throw new IllegalArgumentException("不支持的红石模式: " + mode);
        }
        long seed = optionalLong(document, "seed", 0L);
        int maxTicks = Math.toIntExact(optionalLong(document, "max_ticks", 100L));
        boolean oracleMicroTrace = optionalBoolean(document, "oracle_micro_trace", false);
        TomlTable environmentTable = document.getTable("environment");
        Environment environment = environmentTable == null
            ? Environment.DEFAULT
            : new Environment(
                optionalLong(environmentTable, "game_time", 0L),
                optionalLong(environmentTable, "overworld_time", 0L),
                optionalBoolean(environmentTable, "advance_time", true),
                Math.toIntExact(optionalLong(environmentTable, "sky_light", 15L))
            );
        if (environment.gameTime < 0L || environment.overworldTime < 0L) {
            throw new IllegalArgumentException("环境时间不能为负数");
        }
        if (environment.skyLight < 0 || environment.skyLight > 15) {
            throw new IllegalArgumentException("sky_light 必须在 0..=15 范围内: " + environment.skyLight);
        }
        if (maxTicks <= 0) {
            throw new IllegalArgumentException("max_ticks 必须大于 0");
        }

        TomlTable sourceTable = requiredTable(document, "source");
        Path sourcePath = resolvePath(normalized, requiredString(sourceTable, "path"));
        List<Paste> pastes = new ArrayList<>();
        TomlArray pasteArray = sourceTable.getArray("pastes");
        if (pasteArray != null) {
            for (int index = 0; index < pasteArray.size(); index++) {
                TomlTable paste = pasteArray.getTable(index);
                Long tickValue = paste.getLong("tick");
                Integer tick = tickValue == null ? null : Math.toIntExact(tickValue);
                pastes.add(new Paste(
                    resolvePath(normalized, requiredString(paste, "path")),
                    tick,
                    optionalPos(paste, "origin", Pos.ZERO),
                    optionalString(paste, "rotation", "none"),
                    optionalString(paste, "mirror", "none"),
                    optionalBoolean(paste, "ignore_air", false),
                    optionalBoolean(paste, "paste_entities", false),
                    optionalBoolean(paste, "update", false)
                ));
            }
        }
        Source source = new Source(
            sourcePath,
            optionalPos(sourceTable, "origin", Pos.ZERO),
            optionalString(sourceTable, "initialization", "notify"),
            optionalString(sourceTable, "rotation", "none"),
            optionalString(sourceTable, "mirror", "none"),
            List.copyOf(pastes)
        );

        List<Action> actions = new ArrayList<>();
        TomlArray actionArray = document.getArray("actions");
        if (actionArray != null) {
            for (int index = 0; index < actionArray.size(); index++) {
                TomlTable action = actionArray.getTable(index);
                int tick = Math.toIntExact(requiredLong(action, "tick"));
                if (tick <= 0) {
                    throw new IllegalArgumentException("actions[" + index + "].tick 必须大于 0");
                }
                String type = requiredString(action, "type");
                Pos pos = null;
                String blockName = null;
                Map<String, String> properties = Map.of();
                String face = null;
                double[] location = null;
                boolean arrow = false;
                Long entityId = null;
                String entityKind = null;
                double[] position = null;
                Map<String, JsonElement> fields = Map.of();
                String field = null;
                JsonElement value = JsonNull.INSTANCE;
                switch (type) {
                    case "set_block" -> {
                        pos = requiredPos(action, "pos");
                        blockName = requiredString(action, "name");
                        properties = stringMap(action.getTable("properties"));
                    }
                    case "set_block_entity" -> {
                        pos = requiredPos(action, "pos");
                        TomlTable data = requiredTable(action, "data");
                        entityKind = requiredString(data, "kind");
                        fields = jsonMap(data.getTable("fields"));
                    }
                    case "break_block", "use_block", "press_button", "pull_lever" ->
                        pos = requiredPos(action, "pos");
                    case "spawn_entity" -> {
                        entityId = optionalEntityId(action, "id");
                        entityKind = requiredString(action, "kind");
                        position = requiredDoubleArray(action, "position");
                        fields = jsonMap(action.getTable("fields"));
                    }
                    case "move_entity" -> {
                        entityId = requiredEntityId(action, "id");
                        position = requiredDoubleArray(action, "position");
                    }
                    case "remove_entity" -> entityId = requiredEntityId(action, "id");
                    case "set_entity_field" -> {
                        entityId = requiredEntityId(action, "id");
                        field = requiredString(action, "field");
                        value = requiredJsonValue(action, "value");
                    }
                    case "hit_target" -> {
                        pos = requiredPos(action, "pos");
                        face = requiredString(action, "face");
                        location = requiredDoubleArray(action, "location");
                        arrow = optionalBoolean(action, "arrow", false);
                    }
                    default -> throw new IllegalArgumentException("Java oracle 尚不支持动作: " + type);
                }
                actions.add(new Action(
                    tick,
                    index,
                    type,
                    pos,
                    blockName,
                    properties,
                    face,
                    location,
                    arrow,
                    entityId,
                    entityKind,
                    position,
                    fields,
                    field,
                    value
                ));
            }
        }

        List<Probe> probes = new ArrayList<>();
        TomlArray probeArray = document.getArray("probes");
        if (probeArray != null) {
            for (int index = 0; index < probeArray.size(); index++) {
                TomlTable probe = probeArray.getTable(index);
                String type = requiredString(probe, "type");
                String name = requiredString(probe, "name");
                Probe parsed = switch (type) {
                    case "signal" -> new Probe(
                        name,
                        type,
                        requiredPos(probe, "pos"),
                        optionalString(probe, "direction", null),
                        null,
                        null,
                        null,
                        null
                    );
                    case "block_state", "container_count" -> new Probe(
                        name,
                        type,
                        requiredPos(probe, "pos"),
                        null,
                        null,
                        null,
                        null,
                        null
                    );
                    case "property" -> new Probe(
                        name,
                        type,
                        requiredPos(probe, "pos"),
                        null,
                        requiredString(probe, "property"),
                        null,
                        null,
                        null
                    );
                    case "entity_count" -> new Probe(
                        name,
                        type,
                        null,
                        null,
                        null,
                        null,
                        optionalString(probe, "kind", null),
                        null
                    );
                    case "entity_field" -> new Probe(
                        name,
                        type,
                        null,
                        null,
                        null,
                        requiredEntityId(probe, "id"),
                        null,
                        requiredString(probe, "field")
                    );
                    case "entity_container_count" -> new Probe(
                        name,
                        type,
                        null,
                        null,
                        null,
                        requiredEntityId(probe, "id"),
                        null,
                        null
                    );
                    default -> throw new IllegalArgumentException("Java oracle 尚不支持探针: " + type);
                };
                probes.add(parsed);
            }
        }
        actions.sort((left, right) -> {
            int byTick = Integer.compare(left.tick, right.tick);
            return byTick != 0 ? byTick : Integer.compare(left.order, right.order);
        });
        return new Scenario(
            normalized,
            mode,
            seed,
            maxTicks,
            oracleMicroTrace,
            environment,
            source,
            List.copyOf(actions),
            List.copyOf(probes)
        );
    }

    void validateSupported() {
        if (environment.skyLight != 15) {
            throw new IllegalArgumentException(
                "Java oracle 仅支持 sky_light = 15, 收到 " + environment.skyLight
            );
        }
        if (!Files.isRegularFile(source.path)) {
            throw new IllegalArgumentException("结构文件不存在: " + source.path);
        }
        String fileName = source.path.getFileName().toString();
        if (!fileName.endsWith(".nbt") && !fileName.endsWith(".structure")) {
            throw new IllegalArgumentException("Java oracle 首批仅支持原版 structure NBT: " + source.path);
        }
        source.rotationValue();
        source.mirrorValue();
        if (!source.initialization.equals("raw") && !source.initialization.equals("notify")) {
            throw new IllegalArgumentException("不支持的初始化策略: " + source.initialization);
        }
        for (Paste paste : source.pastes) {
            if (!Files.isRegularFile(paste.path)) {
                throw new IllegalArgumentException("附加结构文件不存在: " + paste.path);
            }
            String pasteName = paste.path.getFileName().toString();
            if (!pasteName.endsWith(".nbt") && !pasteName.endsWith(".structure")) {
                throw new IllegalArgumentException("Java oracle 附加结构需要转换为 vanilla NBT: " + paste.path);
            }
            if (paste.tick != null && (paste.tick <= 0 || paste.tick > maxTicks)) {
                throw new IllegalArgumentException("附加结构 tick 必须在 1..=" + maxTicks + ": " + paste.tick);
            }
            paste.rotationValue();
            paste.mirrorValue();
        }
        for (Action action : actions) {
            if (action.tick > maxTicks) {
                throw new IllegalArgumentException("动作 tick 超过 max_ticks: " + action.tick);
            }
        }
    }

    private static TomlTable requiredTable(TomlTable table, String key) {
        TomlTable value = table.getTable(key);
        if (value == null) {
            throw new IllegalArgumentException("缺少 TOML 表: " + key);
        }
        return value;
    }

    private static String requiredString(TomlTable table, String key) {
        String value = table.getString(key);
        if (value == null) {
            throw new IllegalArgumentException("缺少字符串字段: " + key);
        }
        return value;
    }

    private static String optionalString(TomlTable table, String key, String defaultValue) {
        String value = table.getString(key);
        return value == null ? defaultValue : value;
    }

    private static long requiredLong(TomlTable table, String key) {
        Long value = table.getLong(key);
        if (value == null) {
            throw new IllegalArgumentException("缺少整数段: " + key);
        }
        return value;
    }

    private static long optionalLong(TomlTable table, String key, long defaultValue) {
        Long value = table.getLong(key);
        return value == null ? defaultValue : value;
    }

    private static Long optionalEntityId(TomlTable table, String key) {
        Long value = table.getLong(key);
        if (value != null && value < 0L) {
            throw new IllegalArgumentException("实体 id 不能是负数: " + value);
        }
        return value;
    }

    private static long requiredEntityId(TomlTable table, String key) {
        long value = requiredLong(table, key);
        if (value < 0L) {
            throw new IllegalArgumentException("实体 id 不能是负数: " + value);
        }
        return value;
    }

    private static boolean optionalBoolean(TomlTable table, String key, boolean defaultValue) {
        Boolean value = table.getBoolean(key);
        return value == null ? defaultValue : value;
    }

    private static Path resolvePath(Path scenario, String value) {
        Path path = Path.of(value);
        if (path.isAbsolute()) {
            return path.normalize();
        }
        Path parent = scenario.getParent();
        return (parent == null ? path : parent.resolve(path)).normalize();
    }

    private static double[] requiredDoubleArray(TomlTable table, String key) {
        TomlArray values = table.getArray(key);
        if (values == null || values.size() != 3) {
            throw new IllegalArgumentException("字段必须是 3 元数字数组: " + key);
        }
        double[] result = new double[3];
        for (int index = 0; index < result.length; index++) {
            Object value = values.get(index);
            if (!(value instanceof Number number)) {
                throw new IllegalArgumentException("数组元素必须是数字: " + key + "[" + index + "]");
            }
            result[index] = number.doubleValue();
        }
        return result;
    }

    private static Pos requiredPos(TomlTable table, String key) {
        TomlTable value = table.getTable(key);
        if (value == null) {
            throw new IllegalArgumentException("缺少坐标字段: " + key);
        }
        return Pos.from(value);
    }

    private static Pos optionalPos(TomlTable table, String key, Pos defaultValue) {
        TomlTable value = table.getTable(key);
        return value == null ? defaultValue : Pos.from(value);
    }

    private static Map<String, String> stringMap(TomlTable table) {
        if (table == null) {
            return Map.of();
        }
        Map<String, String> values = new LinkedHashMap<>();
        for (String key : table.keySet().stream().sorted().toList()) {
            Object value = table.get(key);
            if (!(value instanceof String text)) {
                throw new IllegalArgumentException("属性值必须是字符串: " + key);
            }
            values.put(key, text);
        }
        return Collections.unmodifiableMap(values);
    }

    private static Map<String, JsonElement> jsonMap(TomlTable table) {
        if (table == null) {
            return Map.of();
        }
        Map<String, JsonElement> values = new LinkedHashMap<>();
        for (String key : table.keySet().stream().sorted().toList()) {
            values.put(key, jsonValue(table.get(key), key));
        }
        return Collections.unmodifiableMap(values);
    }

    private static JsonElement requiredJsonValue(TomlTable table, String key) {
        Object value = table.get(key);
        if (value == null) {
            throw new IllegalArgumentException("缺少字段: " + key);
        }
        return jsonValue(value, key);
    }

    private static JsonElement jsonValue(Object value, String path) {
        if (value instanceof String text) {
            return new JsonPrimitive(text);
        }
        if (value instanceof Boolean flag) {
            return new JsonPrimitive(flag);
        }
        if (value instanceof Number number) {
            return new JsonPrimitive(number);
        }
        if (value instanceof TomlArray array) {
            JsonArray result = new JsonArray(array.size());
            for (int index = 0; index < array.size(); index++) {
                result.add(jsonValue(array.get(index), path + "[" + index + "]"));
            }
            return result;
        }
        if (value instanceof TomlTable table) {
            JsonObject result = new JsonObject();
            for (String key : table.keySet().stream().sorted().toList()) {
                result.add(key, jsonValue(table.get(key), path + "." + key));
            }
            return result;
        }
        throw new IllegalArgumentException("不支持的 TOML 字段类型: " + path);
    }

    record Source(
        Path path,
        Pos origin,
        String initialization,
        String rotation,
        String mirror,
        List<Paste> pastes
    ) {
        Rotation rotationValue() {
            return switch (rotation) {
                case "none" -> Rotation.NONE;
                case "clockwise90" -> Rotation.CLOCKWISE_90;
                case "clockwise180" -> Rotation.CLOCKWISE_180;
                case "counterclockwise90" -> Rotation.COUNTERCLOCKWISE_90;
                default -> throw new IllegalArgumentException("不支持的 rotation: " + rotation);
            };
        }

        Mirror mirrorValue() {
            return switch (mirror) {
                case "none" -> Mirror.NONE;
                case "left_right" -> Mirror.LEFT_RIGHT;
                case "front_back" -> Mirror.FRONT_BACK;
                default -> throw new IllegalArgumentException("不支持的 mirror: " + mirror);
            };
        }

        Pos toFrame(Pos absolute, FrameGeometry frame) {
            return new Pos(
                Math.addExact(frame.offset().x(), Math.subtractExact(absolute.x(), origin.x())),
                Math.addExact(frame.offset().y(), Math.subtractExact(absolute.y(), origin.y())),
                Math.addExact(frame.offset().z(), Math.subtractExact(absolute.z(), origin.z()))
            );
        }

        double[] toFrame(double[] absolute, FrameGeometry frame) {
            return new double[] {
                frame.offset().x() + absolute[0] - origin.x(),
                frame.offset().y() + absolute[1] - origin.y(),
                frame.offset().z() + absolute[2] - origin.z()
            };
        }

    }

    record Environment(long gameTime, long overworldTime, boolean advanceTime, int skyLight) {
        static final Environment DEFAULT = new Environment(0L, 0L, true, 15);
    }

    record Paste(
        Path path,
        Integer tick,
        Pos origin,
        String rotation,
        String mirror,
        boolean ignoreAir,
        boolean pasteEntities,
        boolean update
    ) {
        Rotation rotationValue() {
            return switch (rotation) {
                case "none" -> Rotation.NONE;
                case "clockwise90" -> Rotation.CLOCKWISE_90;
                case "clockwise180" -> Rotation.CLOCKWISE_180;
                case "counterclockwise90" -> Rotation.COUNTERCLOCKWISE_90;
                default -> throw new IllegalArgumentException("不支持的 rotation: " + rotation);
            };
        }

        Mirror mirrorValue() {
            return switch (mirror) {
                case "none" -> Mirror.NONE;
                case "left_right" -> Mirror.LEFT_RIGHT;
                case "front_back" -> Mirror.FRONT_BACK;
                default -> throw new IllegalArgumentException("不支持的 mirror: " + mirror);
            };
        }
    }

    record Action(
        int tick,
        int order,
        String type,
        Pos pos,
        String blockName,
        Map<String, String> properties,
        String face,
        double[] location,
        boolean arrow,
        Long entityId,
        String entityKind,
        double[] position,
        Map<String, JsonElement> fields,
        String field,
        JsonElement value
    ) {
    }

    record Probe(
        String name,
        String type,
        Pos pos,
        String direction,
        String property,
        Long entityId,
        String entityKind,
        String field
    ) {
    }

    record Pos(int x, int y, int z) {
        static final Pos ZERO = new Pos(0, 0, 0);

        static Pos from(TomlTable table) {
            return new Pos(
                Math.toIntExact(requiredLong(table, "x")),
                Math.toIntExact(requiredLong(table, "y")),
                Math.toIntExact(requiredLong(table, "z"))
            );
        }
    }
}
