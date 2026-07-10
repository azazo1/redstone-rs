package redstone.oracle;

import com.google.gson.JsonObject;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.Comparator;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.gametest.framework.GameTestMainUtil;
import net.minecraft.gametest.framework.TestFunctionLoader;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.IntTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Blocks;

public final class Main {
    private static final String EXPECTED_VERSION = "26.1.2";
    private static final int EXPECTED_DATA_VERSION = 4790;

    private Main() {
    }

    public static void main(String[] args) throws Exception {
        if (args.length > 0 && args[0].equals("--scenario-child")) {
            runScenarioChild(Arrays.copyOfRange(args, 1, args.length));
            return;
        }
        if (args.length == 1 && args[0].equals("--self-test")) {
            selfTest();
            return;
        }
        if (args.length == 1 && args[0].equals("--server-self-test")) {
            serverSelfTest();
            return;
        }
        if (args.length == 1 && args[0].equals("--scenario-self-test")) {
            scenarioSelfTest();
            return;
        }
        if (args.length != 2) {
            System.err.println("usage: vanilla-oracle SCENARIO OUTPUT_JSONL");
            System.err.println("       vanilla-oracle --self-test");
            System.err.println("       vanilla-oracle --server-self-test");
            System.err.println("       vanilla-oracle --scenario-self-test");
            System.exit(2);
        }
        Path scenario = Path.of(args[0]).toAbsolutePath().normalize();
        Path output = Path.of(args[1]).toAbsolutePath().normalize();
        if (!Files.isRegularFile(scenario)) {
            throw new IllegalArgumentException("场景文件不存在: " + scenario);
        }
        runScenario(scenario, output);
    }

    private static void runScenario(Path scenarioPath, Path output) throws Exception {
        Scenario scenario = Scenario.load(scenarioPath);
        scenario.validateSupported();
        initializeMinecraft();
        Path work = Files.createTempDirectory("redstone-oracle-scenario-");
        try {
            Path packSource = work.resolve("packs");
            Path pack = packSource.resolve("redstone-oracle");
            FrameGeometry frame = createPack(pack, scenario);
            Files.deleteIfExists(output);

            String java = Path.of(System.getProperty("java.home"), "bin", "java").toString();
            Path agent = Path.of(
                Main.class.getProtectionDomain().getCodeSource().getLocation().toURI()
            );
            if (!Files.isRegularFile(agent)) {
                throw new IllegalStateException("Java oracle agent 必须从 JAR 运行: " + agent);
            }
            ProcessBuilder processBuilder = new ProcessBuilder(
                java,
                "-Xmx4g",
                "-javaagent:" + agent,
                "-cp",
                System.getProperty("java.class.path"),
                Main.class.getName(),
                "--scenario-child",
                "--universe",
                work.resolve("universe").toString(),
                "--packs",
                packSource.toString(),
                "--tests",
                "redstone:scenario"
            );
            processBuilder.environment().put(ScenarioTestLoader.SCENARIO_ENV, scenario.path.toString());
            processBuilder.environment().put(ScenarioTestLoader.OUTPUT_ENV, output.toString());
            processBuilder.environment().put(FrameGeometry.ENV, frame.encode());
            Path serverLog = work.resolve("gametest.log");
            processBuilder.redirectErrorStream(true);
            processBuilder.redirectOutput(serverLog.toFile());
            System.out.println("启动 Java " + EXPECTED_VERSION + " GameTest 场景");
            int status = processBuilder.start().waitFor();
            if (status != 0) {
                throw new IllegalStateException(
                    "GameTestServer 返回失败状态: " + status + "\n" + Files.readString(serverLog)
                );
            }
            if (!Files.isRegularFile(output)) {
                throw new IllegalStateException("GameTestServer 未生成 oracle 输出: " + output);
            }
            System.out.println("Java GameTest 场景执行完成");
        } finally {
            deleteTree(work);
        }
    }

    private static void scenarioSelfTest() throws Exception {
        Path work = Files.createTempDirectory("redstone-oracle-self-test-");
        try {
            Path structure = work.resolve("machine.nbt");
            Path scenario = work.resolve("scenario.toml");
            Path output = work.resolve("oracle.jsonl");
            writeScenarioSelfTestStructure(structure);
            Files.writeString(
                scenario,
                "version = \"26.1.2\"\n"
                    + "mode = \"default\"\n"
                    + "seed = 0\n"
                    + "max_ticks = 2\n\n"
                    + "[source]\n"
                    + "path = \"machine.nbt\"\n"
                    + "initialization = \"raw\"\n\n"
                    + "[[actions]]\n"
                    + "tick = 2\n"
                    + "type = \"break_block\"\n"
                    + "pos = { x = 0, y = 0, z = 0 }\n\n"
                    + "[[probes]]\n"
                    + "name = \"state\"\n"
                    + "type = \"block_state\"\n"
                    + "pos = { x = 0, y = 0, z = 0 }\n"
            );
            runScenario(scenario, output);
            var lines = Files.readAllLines(output);
            if (lines.size() != 3
                || !lines.get(0).equals("{\"format\":\"probe_samples_v1\"}")
                || !lines.get(1).equals("{\"tick\":1,\"probe\":\"state\",\"value\":11311}")
                || !lines.get(2).equals("{\"tick\":2,\"probe\":\"state\",\"value\":0}")) {
                throw new IllegalStateException("GameTest 场景自检输出不符合预期: " + lines);
            }
            System.out.println("scenario_gametest: ok");
        } finally {
            deleteTree(work);
        }
    }

    private static void writeScenarioSelfTestStructure(Path output) throws Exception {
        CompoundTag root = new CompoundTag();
        root.putInt("DataVersion", EXPECTED_DATA_VERSION);
        ListTag size = new ListTag();
        size.add(IntTag.valueOf(1));
        size.add(IntTag.valueOf(1));
        size.add(IntTag.valueOf(1));
        root.put("size", size);

        CompoundTag state = new CompoundTag();
        state.putString("Name", "minecraft:redstone_block");
        ListTag palette = new ListTag();
        palette.add(state);
        root.put("palette", palette);

        CompoundTag block = new CompoundTag();
        ListTag pos = new ListTag();
        pos.add(IntTag.valueOf(0));
        pos.add(IntTag.valueOf(0));
        pos.add(IntTag.valueOf(0));
        block.put("pos", pos);
        block.putInt("state", 0);
        ListTag blocks = new ListTag();
        blocks.add(block);
        root.put("blocks", blocks);
        root.put("entities", new ListTag());
        NbtIo.write(root, output);
    }

    private static void runScenarioChild(String[] gameTestArgs) throws Exception {
        SharedConstants.tryDetectVersion();
        ScenarioTestLoader loader = ScenarioTestLoader.fromEnvironment();
        OracleServerOptions.apply(loader.scenario());
        TestFunctionLoader.registerLoader(loader);
        GameTestMainUtil.runGameTestServer(gameTestArgs, ignored -> {
        });
    }

    private static FrameGeometry createPack(Path pack, Scenario scenario) throws Exception {
        Path testInstance = pack.resolve("data/redstone/test_instance/scenario.json");
        Path structure = pack.resolve("data/redstone/structure/scenario.nbt");
        Path framePath = pack.resolve("data/redstone/structure/scenario_frame.nbt");
        Files.createDirectories(testInstance.getParent());
        Files.createDirectories(structure.getParent());

        JsonObject packRoot = new JsonObject();
        JsonObject packMetadata = new JsonObject();
        packMetadata.addProperty("description", "redstone-rs Java oracle");
        packMetadata.add("min_format", formatVersion(101, 0));
        packMetadata.add("max_format", formatVersion(101, 1));
        packRoot.add("pack", packMetadata);
        Files.writeString(pack.resolve("pack.mcmeta"), packRoot.toString());

        JsonObject instance = new JsonObject();
        instance.addProperty("type", "minecraft:function");
        instance.addProperty("environment", "minecraft:default");
        instance.addProperty("function", "redstone:scenario");
        instance.addProperty("max_ticks", scenario.maxTicks);
        instance.addProperty("required", true);
        instance.addProperty("setup_ticks", 0);
        instance.addProperty("structure", "redstone:scenario_frame");
        Files.writeString(testInstance, instance.toString());

        CompoundTag root = readStructure(scenario.source.path());
        int dataVersion = root.getIntOr("DataVersion", 0);
        if (dataVersion > EXPECTED_DATA_VERSION) {
            throw new IllegalArgumentException(
                "结构 DataVersion " + dataVersion + " 高于 Java " + EXPECTED_DATA_VERSION
            );
        }
        FrameGeometry frame = FrameGeometry.from(root, scenario);
        NbtIo.writeCompressed(root, structure);
        CompoundTag frameRoot = root.copy();
        frameRoot.put("palette", airPalette());
        frameRoot.put("blocks", new ListTag());
        frameRoot.put("entities", new ListTag());
        frameRoot.put("size", integerList(frame.size().x(), frame.size().y(), frame.size().z()));
        NbtIo.writeCompressed(frameRoot, framePath);
        return frame;
    }

    private static ListTag airPalette() {
        CompoundTag air = new CompoundTag();
        air.putString("Name", "minecraft:air");
        ListTag palette = new ListTag();
        palette.add(air);
        return palette;
    }

    private static ListTag integerList(int... values) {
        ListTag result = new ListTag();
        for (int value : values) {
            result.add(IntTag.valueOf(value));
        }
        return result;
    }

    private static com.google.gson.JsonArray formatVersion(int major, int minor) {
        com.google.gson.JsonArray result = new com.google.gson.JsonArray();
        result.add(major);
        result.add(minor);
        return result;
    }

    private static CompoundTag readStructure(Path path) throws Exception {
        byte[] magic = new byte[2];
        try (var input = Files.newInputStream(path)) {
            int count = input.read(magic);
            if (count < 2) {
                throw new IllegalArgumentException("结构 NBT 文件过短: " + path);
            }
        }
        if ((magic[0] & 0xff) == 0x1f && (magic[1] & 0xff) == 0x8b) {
            return NbtIo.readCompressed(path, NbtAccounter.unlimitedHeap());
        }
        CompoundTag root = NbtIo.read(path);
        if (root == null) {
            throw new IllegalArgumentException("无法读取结构 NBT: " + path);
        }
        return root;
    }

    private static void serverSelfTest() throws Exception {
        Path universe = Files.createTempDirectory("redstone-oracle-gametest-");
        try {
            String java = Path.of(System.getProperty("java.home"), "bin", "java").toString();
            Process process = new ProcessBuilder(
                java,
                "-Xmx2g",
                "-cp",
                System.getProperty("java.class.path"),
                "net.minecraft.gametest.Main",
                "--universe",
                universe.toString(),
                "--tests",
                "minecraft:always_pass"
            ).inheritIO().start();
            int status = process.waitFor();
            if (status != 0) {
                throw new IllegalStateException("GameTestServer 返回失败状态: " + status);
            }
            System.out.println("gametest_server: ok");
        } finally {
            deleteTree(universe);
        }
    }

    private static void deleteTree(Path root) throws Exception {
        if (!Files.exists(root)) {
            return;
        }
        try (var paths = Files.walk(root)) {
            paths.sorted(Comparator.reverseOrder()).forEach(path -> {
                try {
                    Files.deleteIfExists(path);
                } catch (Exception error) {
                    throw new RuntimeException(error);
                }
            });
        }
    }

    private static void selfTest() {
        initializeMinecraft();
        int blockCount = BuiltInRegistries.BLOCK.size();
        if (blockCount <= 0 || Blocks.REDSTONE_WIRE == Blocks.AIR) {
            throw new IllegalStateException("Minecraft 方块注册表初始化失败");
        }
        if (!BuiltInRegistries.BLOCK.containsKey(Identifier.parse("minecraft:redstone_wire"))) {
            throw new IllegalStateException("红石线未出现在方块注册表中");
        }
        System.out.println("version: " + EXPECTED_VERSION);
        System.out.println("data_version: " + EXPECTED_DATA_VERSION);
        System.out.println("blocks: " + blockCount);
        System.out.println("bootstrap: ok");
    }

    private static void initializeMinecraft() {
        SharedConstants.tryDetectVersion();
        var version = SharedConstants.getCurrentVersion();
        if (!EXPECTED_VERSION.equals(version.name())) {
            throw new IllegalStateException(
                "Minecraft 版本不匹配: expected " + EXPECTED_VERSION + ", actual " + version.name()
            );
        }
        int dataVersion = version.dataVersion().version();
        if (dataVersion != EXPECTED_DATA_VERSION) {
            throw new IllegalStateException(
                "DataVersion 不匹配: expected " + EXPECTED_DATA_VERSION + ", actual " + dataVersion
            );
        }
        Bootstrap.bootStrap();
        Bootstrap.validate();
    }
}
