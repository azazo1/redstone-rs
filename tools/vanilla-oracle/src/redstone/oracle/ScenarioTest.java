package redstone.oracle;

import com.google.gson.JsonObject;
import com.google.gson.JsonElement;
import com.google.gson.JsonNull;
import com.google.gson.JsonPrimitive;
import java.io.BufferedWriter;
import java.io.IOException;
import java.lang.reflect.Field;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.ArrayList;
import java.util.List;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.gametest.framework.GameTestHelper;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtUtils;
import net.minecraft.resources.Identifier;
import net.minecraft.world.Container;
import net.minecraft.world.entity.EntitySpawnReason;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.projectile.Projectile;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructurePlaceSettings;
import net.minecraft.world.level.storage.ServerLevelData;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.Vec3;

final class ScenarioTest {
    private final Scenario scenario;
    private final Path output;
    private final List<Scenario.Action> actions;
    private final FrameGeometry frame;
    private final ScenarioEntityStore entities;
    private BufferedWriter writer;
    private OracleTraceRecorder microTrace;
    private int actionIndex;

    ScenarioTest(Scenario scenario, Path output, FrameGeometry frame) {
        this.scenario = scenario;
        this.output = output;
        this.actions = new ArrayList<>(scenario.actions);
        this.frame = frame;
        this.entities = new ScenarioEntityStore(scenario, frame);
    }

    void run(GameTestHelper helper) {
        try {
            Path parent = output.getParent();
            if (parent != null) {
                Files.createDirectories(parent);
            }
            writer = Files.newBufferedWriter(
                output,
                StandardCharsets.UTF_8,
                StandardOpenOption.CREATE,
                StandardOpenOption.TRUNCATE_EXISTING,
                StandardOpenOption.WRITE
            );
            writer.write(
                scenario.oracleMicroTrace
                    ? "{\"format\":\"oracle_samples_v2\"}"
                    : "{\"format\":\"probe_samples_v1\"}"
            );
            writer.newLine();
            writer.flush();
            for (int tick = 0; tick <= scenario.maxTicks; tick++) {
                helper.runAtTickTime(tick, () -> tick(helper));
            }
        } catch (IOException error) {
            throw new IllegalStateException("无法创建 oracle 输出: " + output, error);
        }
    }

    private void initialize(GameTestHelper helper) {
        if (helper.getLevel().getSeed() != scenario.seed) {
            throw new IllegalStateException(
                "世界 seed 不匹配: expected " + scenario.seed + ", actual " + helper.getLevel().getSeed()
            );
        }
        boolean expectedExperimental = scenario.mode.equals("experimental");
        boolean actualExperimental = helper.getLevel()
            .enabledFeatures()
            .contains(FeatureFlags.REDSTONE_EXPERIMENTS);
        if (expectedExperimental != actualExperimental) {
            throw new IllegalStateException(
                "redstone_experiments feature flag 不匹配: expected "
                    + expectedExperimental
                    + ", actual "
                    + actualExperimental
            );
        }
        if (scenario.oracleMicroTrace) {
            BlockPos actualOrigin = helper.absolutePos(blockPos(frame.offset()));
            BlockPos expectedOrigin = blockPos(scenario.source.origin());
            if (!actualOrigin.equals(expectedOrigin)) {
                throw new IllegalStateException(
                    "场景绝对原点不匹配: expected " + expectedOrigin + ", actual " + actualOrigin
                );
            }
        }
        var template = helper.getLevel()
            .getStructureManager()
            .get(Identifier.parse("redstone:scenario"))
            .orElseThrow(() -> new IllegalStateException("缺少 redstone:scenario structure"));
        BlockPos origin = helper.absolutePos(blockPos(frame.offset()));
        StructurePlaceSettings settings = new StructurePlaceSettings()
            .setIgnoreEntities(false)
            .setKnownShape(scenario.source.initialization().equals("raw"))
            .setMirror(scenario.source.mirrorValue())
            .setRotation(scenario.source.rotationValue());
        if (!template.placeInWorld(helper.getLevel(), origin, origin, settings, helper.getLevel().getRandom(), 818)) {
            throw new IllegalStateException("放置 redstone:scenario structure 失败");
        }
    }

    private void tick(GameTestHelper helper) {
        int tick = Math.toIntExact(helper.getTick());
        try {
            if (tick == 0) {
                if (scenario.oracleMicroTrace) {
                    microTrace = new OracleTraceRecorder(scenario, frame, helper);
                    microTrace.setTick(0);
                }
                initialize(helper);
            }
            if (tick > 0) {
                for (Scenario.Probe probe : scenario.probes) {
                    writeSample(helper, tick, probe);
                }
            }
            int nextTick = tick + 1;
            if (microTrace != null) {
                microTrace.setTick(nextTick);
            }
            if (actionIndex < actions.size() && actions.get(actionIndex).tick() == nextTick) {
                withNextGameTime(helper, () -> {
                    while (actionIndex < actions.size() && actions.get(actionIndex).tick() == nextTick) {
                        applyAction(helper, actions.get(actionIndex));
                        actionIndex++;
                    }
                });
            }
            if (microTrace != null) {
                microTrace.writePending(writer);
            }
            writer.flush();
            if (tick >= scenario.maxTicks) {
                closeNeighborTrace();
                writer.close();
                writer = null;
                helper.succeed();
            }
        } catch (Exception error) {
            closeOutput();
            throw new IllegalStateException("oracle 场景 tick " + tick + " 执行失败", error);
        }
    }

    private void applyAction(GameTestHelper helper, Scenario.Action action) {
        switch (action.type()) {
            case "set_block" -> helper.setBlock(actionPos(action), resolveBlockState(helper, action));
            case "break_block" -> helper.destroyBlock(actionPos(action));
            case "use_block" -> helper.useBlock(actionPos(action));
            case "press_button" -> helper.pressButton(actionPos(action));
            case "pull_lever" -> helper.pullLever(actionPos(action));
            case "spawn_entity" -> entities.spawn(helper, action);
            case "move_entity" -> entities.move(helper, action.entityId(), action.position());
            case "remove_entity" -> entities.remove(action.entityId());
            case "set_entity_field" ->
                entities.setField(action.entityId(), action.field(), action.value());
            case "hit_target" -> hitTarget(helper, actionPos(action), action);
            default -> throw new IllegalArgumentException("Java oracle 尚不支持动作: " + action.type());
        }
    }

    private BlockPos actionPos(Scenario.Action action) {
        return blockPos(scenario.source.toFrame(action.pos(), frame));
    }

    private void hitTarget(GameTestHelper helper, BlockPos relativePos, Scenario.Action action) {
        Projectile projectile = (action.arrow() ? EntityType.ARROW : EntityType.SNOWBALL)
            .create(helper.getLevel(), EntitySpawnReason.STRUCTURE);
        if (projectile == null) {
            throw new IllegalStateException("无法创建 target projectile");
        }
        Vec3 location = new Vec3(action.location()[0], action.location()[1], action.location()[2]);
        BlockPos absolutePos = helper.absolutePos(relativePos);
        BlockHitResult hit = new BlockHitResult(
            location,
            parseDirection(action.face()),
            absolutePos,
            false
        );
        BlockState state = helper.getLevel().getBlockState(absolutePos);
        state.onProjectileHit(helper.getLevel(), state, hit, projectile);
        projectile.discard();
    }

    private static void withNextGameTime(GameTestHelper helper, Runnable action) throws Exception {
        Field field = helper.getLevel().getClass().getDeclaredField("serverLevelData");
        field.setAccessible(true);
        ServerLevelData levelData = (ServerLevelData)field.get(helper.getLevel());
        long current = levelData.getGameTime();
        levelData.setGameTime(Math.addExact(current, 1L));
        try {
            action.run();
        } finally {
            levelData.setGameTime(current);
        }
    }

    private static BlockState resolveBlockState(GameTestHelper helper, Scenario.Action action) {
        Identifier id = Identifier.parse(action.blockName());
        Block block = BuiltInRegistries.BLOCK.getValue(id);
        if (block == null || block == Blocks.AIR && !id.equals(Identifier.parse("minecraft:air"))) {
            throw new IllegalArgumentException("未知方块: " + action.blockName());
        }
        CompoundTag tag = new CompoundTag();
        tag.putString("Name", id.toString());
        if (!action.properties().isEmpty()) {
            CompoundTag properties = new CompoundTag();
            action.properties().forEach(properties::putString);
            tag.put("Properties", properties);
        }
        BlockState state = NbtUtils.readBlockState(
            helper.getLevel().registryAccess().lookupOrThrow(Registries.BLOCK),
            tag
        );
        for (var entry : action.properties().entrySet()) {
            Property<?> property = block.getStateDefinition().getProperty(entry.getKey());
            if (property == null || !property.value(state).valueName().equals(entry.getValue())) {
                throw new IllegalArgumentException(
                    "无效方块属性: " + action.blockName() + " " + entry.getKey() + "=" + entry.getValue()
                );
            }
        }
        return state;
    }

    private void writeSample(GameTestHelper helper, int tick, Scenario.Probe probe) throws IOException {
        JsonObject sample = new JsonObject();
        sample.addProperty("tick", tick);
        sample.addProperty("probe", probe.name());
        sample.add("value", readProbe(helper, probe));
        if (scenario.oracleMicroTrace) {
            sample.addProperty("kind", "probe");
        }
        writer.write(sample.toString());
        writer.newLine();
    }

    private JsonElement readProbe(GameTestHelper helper, Scenario.Probe probe) {
        if (probe.type().equals("entity_count")) {
            return new JsonPrimitive(entities.entityCount(helper, probe.entityKind()));
        }
        if (probe.type().equals("entity_field")) {
            return entities.readField(probe.entityId(), probe.field());
        }
        if (probe.type().equals("entity_container_count")) {
            return new JsonPrimitive(entities.containerCount(probe.entityId()));
        }
        BlockPos relative = blockPos(scenario.source.toFrame(probe.pos(), frame));
        BlockPos absolute = helper.absolutePos(relative);
        BlockState state = helper.getBlockState(relative);
        return switch (probe.type()) {
            case "signal" -> new JsonPrimitive(readSignal(helper, absolute, probe.direction()));
            case "block_state" -> new JsonPrimitive(Block.getId(state));
            case "property" -> {
                String value = readProperty(state, probe.property());
                yield value == null ? JsonNull.INSTANCE : new JsonPrimitive(value);
            }
            case "container_count" -> new JsonPrimitive(readContainerCount(helper, absolute));
            default -> throw new IllegalArgumentException("Java oracle 尚不支持探针: " + probe.type());
        };
    }

    private static int readSignal(GameTestHelper helper, BlockPos pos, String direction) {
        if (direction != null) {
            return helper.getLevel().getSignal(pos, parseDirection(direction));
        }
        int best = 0;
        Direction[] order = {
            Direction.WEST,
            Direction.EAST,
            Direction.DOWN,
            Direction.UP,
            Direction.NORTH,
            Direction.SOUTH
        };
        for (Direction candidate : order) {
            best = Math.max(best, helper.getLevel().getSignal(pos, candidate));
        }
        return best;
    }

    private static String readProperty(BlockState state, String propertyName) {
        if (propertyName == null) {
            throw new IllegalArgumentException("property 探针缺少 property 字段");
        }
        Property<?> property = state.getBlock().getStateDefinition().getProperty(propertyName);
        if (property == null) {
            return null;
        }
        return property.value(state).valueName();
    }

    private static int readContainerCount(GameTestHelper helper, BlockPos pos) {
        BlockEntity blockEntity = helper.getLevel().getBlockEntity(pos);
        if (!(blockEntity instanceof Container container)) {
            return 0;
        }
        int count = 0;
        for (int slot = 0; slot < container.getContainerSize(); slot++) {
            count += container.getItem(slot).getCount();
        }
        return count;
    }

    private static Direction parseDirection(String value) {
        return switch (value) {
            case "west" -> Direction.WEST;
            case "east" -> Direction.EAST;
            case "down" -> Direction.DOWN;
            case "up" -> Direction.UP;
            case "north" -> Direction.NORTH;
            case "south" -> Direction.SOUTH;
            default -> throw new IllegalArgumentException("未知方向: " + value);
        };
    }

    private static BlockPos blockPos(Scenario.Pos pos) {
        return new BlockPos(pos.x(), pos.y(), pos.z());
    }

    private void closeOutput() {
        closeNeighborTrace();
        if (writer == null) {
            return;
        }
        try {
            writer.close();
        } catch (IOException ignored) {
        }
        writer = null;
    }

    private void closeNeighborTrace() {
        if (microTrace == null) {
            return;
        }
        microTrace.close();
        microTrace = null;
    }
}
