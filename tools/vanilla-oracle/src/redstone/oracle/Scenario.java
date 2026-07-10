package redstone.oracle;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
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
    final Source source;
    final List<Action> actions;
    final List<Probe> probes;

    private Scenario(
        Path path,
        String mode,
        long seed,
        int maxTicks,
        Source source,
        List<Action> actions,
        List<Probe> probes
    ) {
        this.path = path;
        this.mode = mode;
        this.seed = seed;
        this.maxTicks = maxTicks;
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
        if (maxTicks <= 0) {
            throw new IllegalArgumentException("max_ticks 必须大于 0");
        }

        TomlTable sourceTable = requiredTable(document, "source");
        Path sourcePath = Path.of(requiredString(sourceTable, "path"));
        if (!sourcePath.isAbsolute()) {
            Path parent = normalized.getParent();
            sourcePath = (parent == null ? sourcePath : parent.resolve(sourcePath)).normalize();
        }
        Source source = new Source(
            sourcePath,
            optionalPos(sourceTable, "origin", Pos.ZERO),
            optionalString(sourceTable, "initialization", "notify"),
            optionalString(sourceTable, "rotation", "none"),
            optionalString(sourceTable, "mirror", "none")
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
                Pos pos = requiredPos(action, "pos");
                String blockName = null;
                Map<String, String> properties = Map.of();
                String face = null;
                double[] location = null;
                boolean arrow = false;
                if (type.equals("set_block")) {
                    blockName = requiredString(action, "name");
                    properties = stringMap(action.getTable("properties"));
                } else if (type.equals("hit_target")) {
                    face = requiredString(action, "face");
                    location = requiredDoubleArray(action, "location");
                    arrow = optionalBoolean(action, "arrow", false);
                } else if (!type.equals("break_block")
                    && !type.equals("use_block")
                    && !type.equals("press_button")
                    && !type.equals("pull_lever")) {
                    throw new IllegalArgumentException("Java oracle 尚不支持动作: " + type);
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
                    arrow
                ));
            }
        }

        List<Probe> probes = new ArrayList<>();
        TomlArray probeArray = document.getArray("probes");
        if (probeArray != null) {
            for (int index = 0; index < probeArray.size(); index++) {
                TomlTable probe = probeArray.getTable(index);
                String type = requiredString(probe, "type");
                if (!type.equals("signal")
                    && !type.equals("block_state")
                    && !type.equals("property")
                    && !type.equals("container_count")) {
                    throw new IllegalArgumentException("Java oracle 尚不支持探针: " + type);
                }
                probes.add(new Probe(
                    requiredString(probe, "name"),
                    type,
                    requiredPos(probe, "pos"),
                    optionalString(probe, "direction", null),
                    optionalString(probe, "property", null)
                ));
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
            source,
            List.copyOf(actions),
            List.copyOf(probes)
        );
    }

    void validateSupported() {
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

    private static boolean optionalBoolean(TomlTable table, String key, boolean defaultValue) {
        Boolean value = table.getBoolean(key);
        return value == null ? defaultValue : value;
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
        return Map.copyOf(values);
    }

    record Source(Path path, Pos origin, String initialization, String rotation, String mirror) {
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

    record Action(
        int tick,
        int order,
        String type,
        Pos pos,
        String blockName,
        Map<String, String> properties,
        String face,
        double[] location,
        boolean arrow
    ) {
    }

    record Probe(String name, String type, Pos pos, String direction, String property) {
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
