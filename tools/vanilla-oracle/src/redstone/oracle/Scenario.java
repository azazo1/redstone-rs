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
                if (type.equals("set_block")) {
                    blockName = requiredString(action, "name");
                    properties = stringMap(action.getTable("properties"));
                } else if (!type.equals("break_block")
                    && !type.equals("use_block")
                    && !type.equals("press_button")
                    && !type.equals("pull_lever")) {
                    throw new IllegalArgumentException("Java oracle 尚不支持动作: " + type);
                }
                actions.add(new Action(tick, index, type, pos, blockName, properties));
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
        if (!source.origin.equals(Pos.ZERO)) {
            throw new IllegalArgumentException("Java oracle 尚不支持非零 origin");
        }
        if (!source.rotation.equals("none")) {
            throw new IllegalArgumentException("Java oracle 尚不支持场景 rotation: " + source.rotation);
        }
        if (!source.mirror.equals("none")) {
            throw new IllegalArgumentException("Java oracle 尚不支持场景 mirror: " + source.mirror);
        }
        if (mode.equals("experimental")) {
            throw new IllegalArgumentException("GameTestServer 尚未启用 redstone_experiments feature flag");
        }
        if (seed != 0L) {
            throw new IllegalArgumentException("Java oracle 首批仅支持 seed = 0");
        }
        if (!source.initialization.equals("raw")) {
            throw new IllegalArgumentException("Java oracle 首批仅支持 raw 初始化");
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
    }

    record Action(
        int tick,
        int order,
        String type,
        Pos pos,
        String blockName,
        Map<String, String> properties
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
