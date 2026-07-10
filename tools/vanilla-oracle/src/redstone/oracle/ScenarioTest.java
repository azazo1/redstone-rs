package redstone.oracle;

import com.google.gson.JsonObject;
import com.google.gson.JsonElement;
import com.google.gson.JsonNull;
import com.google.gson.JsonPrimitive;
import java.io.BufferedWriter;
import java.io.IOException;
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
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructurePlaceSettings;

final class ScenarioTest {
    private final Scenario scenario;
    private final Path output;
    private final List<Scenario.Action> actions;
    private BufferedWriter writer;
    private int actionIndex;

    ScenarioTest(Scenario scenario, Path output) {
        this.scenario = scenario;
        this.output = output;
        this.actions = new ArrayList<>(scenario.actions);
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
            writer.write("{\"format\":\"probe_samples_v1\"}");
            writer.newLine();
            writer.flush();
            for (int tick = 0; tick <= scenario.maxTicks; tick++) {
                helper.runAtTickTime(tick, () -> tick(helper));
            }
        } catch (IOException error) {
            throw new IllegalStateException("无法创建 oracle 输出: " + output, error);
        }
    }

    private static void initialize(GameTestHelper helper) {
        var template = helper.getLevel()
            .getStructureManager()
            .get(Identifier.parse("redstone:scenario"))
            .orElseThrow(() -> new IllegalStateException("缺少 redstone:scenario structure"));
        BlockPos origin = helper.absolutePos(BlockPos.ZERO);
        StructurePlaceSettings settings = new StructurePlaceSettings()
            .setIgnoreEntities(false)
            .setKnownShape(true);
        if (!template.placeInWorld(helper.getLevel(), origin, origin, settings, helper.getLevel().getRandom(), 818)) {
            throw new IllegalStateException("放置 redstone:scenario structure 失败");
        }
    }

    private void tick(GameTestHelper helper) {
        int tick = Math.toIntExact(helper.getTick());
        try {
            if (tick == 0) {
                initialize(helper);
            }
            if (tick > 0) {
                for (Scenario.Probe probe : scenario.probes) {
                    writeSample(helper, tick, probe);
                }
            }
            int nextTick = tick + 1;
            while (actionIndex < actions.size() && actions.get(actionIndex).tick() == nextTick) {
                applyAction(helper, actions.get(actionIndex));
                actionIndex++;
            }
            writer.flush();
            if (tick >= scenario.maxTicks) {
                writer.close();
                writer = null;
                helper.succeed();
            }
        } catch (Exception error) {
            closeOutput();
            throw new IllegalStateException("oracle 场景 tick " + tick + " 执行失败", error);
        }
    }

    private static void applyAction(GameTestHelper helper, Scenario.Action action) {
        BlockPos pos = blockPos(action.pos());
        switch (action.type()) {
            case "set_block" -> helper.setBlock(pos, resolveBlockState(helper, action));
            case "break_block" -> helper.destroyBlock(pos);
            case "use_block" -> helper.useBlock(pos);
            case "press_button" -> helper.pressButton(pos);
            case "pull_lever" -> helper.pullLever(pos);
            default -> throw new IllegalArgumentException("Java oracle 尚不支持动作: " + action.type());
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
        writer.write(sample.toString());
        writer.newLine();
    }

    private static JsonElement readProbe(GameTestHelper helper, Scenario.Probe probe) {
        BlockPos relative = blockPos(probe.pos());
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
        if (writer == null) {
            return;
        }
        try {
            writer.close();
        } catch (IOException ignored) {
        }
        writer = null;
    }
}
