package io.redstoners.oracle;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.mojang.logging.LogUtils;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.gametest.framework.GameTestHelper;
import net.minecraft.gametest.framework.TestFunctionLoader;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.Container;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;

import java.io.IOException;
import java.io.Reader;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import java.util.Optional;
import java.util.function.BiConsumer;
import java.util.function.Consumer;
import org.slf4j.Logger;

public final class OracleMain {
    private static final String NAMESPACE = "redstone_oracle";
    private static final String TEST_ID = "trace";
    private static final String CASE_ENV = "REDSTONE_ORACLE_CASE";
    private static final String OUTPUT_ENV = "REDSTONE_ORACLE_OUTPUT";
    private static final Logger LOGGER = LogUtils.getLogger();

    private OracleMain() {}

    public static void main(String[] args) throws Exception {
        LOGGER.info("Registering redstone-rs GameTest function loader");
        TestFunctionLoader.registerLoader(new OracleFunctionLoader());
        net.minecraft.gametest.Main.main(args);
    }

    private static final class OracleFunctionLoader extends TestFunctionLoader {
        @Override
        public void load(BiConsumer<ResourceKey<Consumer<GameTestHelper>>, Consumer<GameTestHelper>> registrar) {
            ResourceKey<Consumer<GameTestHelper>> key = ResourceKey.create(
                Registries.TEST_FUNCTION,
                Identifier.fromNamespaceAndPath(NAMESPACE, TEST_ID)
            );
            LOGGER.info("Registering redstone-rs GameTest function {}", key.identifier());
            registrar.accept(key, OracleMain::runTrace);
        }
    }

    private static void runTrace(GameTestHelper helper) {
        OracleCase oracleCase = OracleCase.load(casePath());
        TraceWriter trace = new TraceWriter(outputPath());
        for (JsonElement action : oracleCase.actions()) {
            JsonObject input = action.getAsJsonObject().getAsJsonObject("action");
            long tick = input.getAsJsonPrimitive("tick").getAsLong();
            helper.runAtTickTime(tick, () -> applyAction(helper, input.getAsJsonObject("operation")));
        }
        for (long tick = 1; tick <= oracleCase.ticks(); tick++) {
            long captureTick = tick;
            helper.runAtTickTime(tick + 1, () -> trace.capture(helper, oracleCase, captureTick));
        }
        helper.runAtTickTime(oracleCase.ticks() + 2, () -> {
            trace.write();
            helper.succeed();
        });
    }

    private static Path casePath() {
        return Path.of(System.getenv().getOrDefault(CASE_ENV, ".gametest/oracle-case/oracle-case.json"));
    }

    private static Path outputPath() {
        return Path.of(System.getenv().getOrDefault(OUTPUT_ENV, ".gametest/oracle-case/vanilla-trace.json"));
    }

    private static void applyAction(GameTestHelper helper, JsonObject operation) {
        Map.Entry<String, JsonElement> entry = operation.entrySet().iterator().next();
        JsonObject value = entry.getValue().getAsJsonObject();
        switch (entry.getKey()) {
            case "SetBlock" -> helper.setBlock(position(value.getAsJsonObject("position")), stateFromJson(value.getAsJsonObject("state")));
            case "UseBlock" -> helper.useBlock(position(value.getAsJsonObject("position")));
            case "SetPowered" -> setProperty(helper, position(value.getAsJsonObject("position")), "powered", Boolean.toString(value.getAsJsonPrimitive("powered").getAsBoolean()));
            case "SetSignal" -> setProperty(helper, position(value.getAsJsonObject("position")), "power", Integer.toString(value.getAsJsonPrimitive("signal").getAsInt()));
            case "SetComparatorSignal" -> setProperty(helper, position(value.getAsJsonObject("position")), "powered", Boolean.toString(value.getAsJsonPrimitive("signal").getAsInt() > 0));
            case "SetInventory" -> setInventory(helper, value);
            case "TriggerNeighborUpdate" -> triggerNeighborUpdate(helper, value);
            default -> throw new IllegalArgumentException("unsupported oracle action: " + entry.getKey());
        }
    }

    private static void setInventory(GameTestHelper helper, JsonObject value) {
        BlockPos relative = position(value.getAsJsonObject("position"));
        BlockEntity entity = helper.getLevel().getBlockEntity(helper.absolutePos(relative));
        if (!(entity instanceof Container container)) {
            throw new IllegalArgumentException("inventory action requires a container at " + relative);
        }
        container.clearContent();
        int items = value.getAsJsonPrimitive("items").getAsInt();
        for (int slot = 0; slot < container.getContainerSize() && items > 0; slot++) {
            int count = Math.min(items, container.getMaxStackSize());
            container.setItem(slot, new ItemStack(Items.REDSTONE, count));
            items -= count;
        }
        container.setChanged();
        helper.getLevel().updateNeighbourForOutputSignal(helper.absolutePos(relative), helper.getBlockState(relative).getBlock());
    }

    private static void triggerNeighborUpdate(GameTestHelper helper, JsonObject value) {
        BlockPos relative = position(value.getAsJsonObject("position"));
        BlockPos absolute = helper.absolutePos(relative);
        BlockState state = helper.getLevel().getBlockState(absolute);
        helper.getLevel().neighborChanged(state, absolute, blockForKind(value.getAsJsonPrimitive("changed_block").getAsString()), null, false);
    }

    private static void setProperty(GameTestHelper helper, BlockPos position, String name, String value) {
        BlockState state = withProperty(helper.getBlockState(position), name, value);
        helper.setBlock(position, state);
    }

    private static BlockState stateFromJson(JsonObject state) {
        String kind = state.getAsJsonPrimitive("kind").getAsString();
        int bits = state.getAsJsonPrimitive("bits").getAsInt();
        BlockState result = blockForState(kind, bits).defaultBlockState();
        result = withProperty(result, "facing", directionName(bits & 7));
        result = withProperty(result, "powered", Boolean.toString((bits & (1 << 3)) != 0));
        result = withProperty(result, "power", Integer.toString((bits >> 4) & 15));
        result = withProperty(result, "delay", Integer.toString(((bits >> 8) & 3) + 1));
        result = withProperty(result, "mode", (bits & (1 << 11)) != 0 ? "subtract" : "compare");
        result = withProperty(result, "extended", Boolean.toString((bits & (1 << 12)) != 0));
        result = withProperty(result, "lit", Boolean.toString((bits & (1 << 13)) != 0));
        result = withProperty(result, "open", Boolean.toString((bits & (1 << 16)) != 0));
        return result;
    }

    private static BlockState withProperty(BlockState state, String name, String value) {
        for (Property<?> property : state.getProperties()) {
            if (!property.getName().equals(name)) {
                continue;
            }
            Optional<?> parsed = property.getValue(value);
            if (parsed.isPresent()) {
                return setPropertyUnchecked(state, property, parsed.get());
            }
        }
        return state;
    }

    @SuppressWarnings({"rawtypes", "unchecked"})
    private static BlockState setPropertyUnchecked(BlockState state, Property property, Object value) {
        return state.setValue(property, (Comparable) value);
    }

    private static BlockPos position(JsonObject position) {
        return new BlockPos(
            position.getAsJsonPrimitive("x").getAsInt(),
            position.getAsJsonPrimitive("y").getAsInt(),
            position.getAsJsonPrimitive("z").getAsInt()
        );
    }

    private static Block blockForKind(String kind) {
        return switch (kind) {
            case "Air" -> Blocks.AIR;
            case "Solid" -> Blocks.STONE;
            case "Glass" -> Blocks.GLASS;
            case "Immovable" -> Blocks.OBSIDIAN;
            case "RedstoneBlock" -> Blocks.REDSTONE_BLOCK;
            case "RedstoneWire" -> Blocks.REDSTONE_WIRE;
            case "RedstoneTorch" -> Blocks.REDSTONE_TORCH;
            case "Lever" -> Blocks.LEVER;
            case "Button" -> Blocks.STONE_BUTTON;
            case "PressurePlate" -> Blocks.STONE_PRESSURE_PLATE;
            case "Repeater" -> Blocks.REPEATER;
            case "Comparator" -> Blocks.COMPARATOR;
            case "Observer" -> Blocks.OBSERVER;
            case "Lamp" -> Blocks.REDSTONE_LAMP;
            case "CopperBulb" -> Blocks.COPPER_BULB;
            case "DaylightDetector" -> Blocks.DAYLIGHT_DETECTOR;
            case "Target" -> Blocks.TARGET;
            case "Door" -> Blocks.OAK_DOOR;
            case "Trapdoor" -> Blocks.OAK_TRAPDOOR;
            case "FenceGate" -> Blocks.OAK_FENCE_GATE;
            case "NoteBlock" -> Blocks.NOTE_BLOCK;
            case "Dropper" -> Blocks.DROPPER;
            case "Dispenser" -> Blocks.DISPENSER;
            case "Crafter" -> Blocks.CRAFTER;
            case "Tnt" -> Blocks.TNT;
            case "Hopper" -> Blocks.HOPPER;
            case "Container" -> Blocks.BARREL;
            case "Piston" -> Blocks.PISTON;
            case "StickyPiston" -> Blocks.STICKY_PISTON;
            case "PistonHead" -> Blocks.PISTON_HEAD;
            case "MovingPiston" -> Blocks.MOVING_PISTON;
            case "SlimeBlock" -> Blocks.SLIME_BLOCK;
            case "HoneyBlock" -> Blocks.HONEY_BLOCK;
            default -> throw new IllegalArgumentException("unsupported block kind: " + kind);
        };
    }

    private static Block blockForState(String kind, int bits) {
        return switch (kind) {
            case "RedstoneTorch" -> (bits & (1 << 15)) != 0 ? Blocks.REDSTONE_WALL_TORCH : Blocks.REDSTONE_TORCH;
            case "Button" -> (bits & (1 << 14)) != 0 ? Blocks.OAK_BUTTON : Blocks.STONE_BUTTON;
            case "PressurePlate" -> (bits & (1 << 17)) != 0 ? Blocks.LIGHT_WEIGHTED_PRESSURE_PLATE : Blocks.STONE_PRESSURE_PLATE;
            default -> blockForKind(kind);
        };
    }

    private static String directionName(int direction) {
        return switch (direction) {
            case 0 -> "north";
            case 1 -> "south";
            case 2 -> "west";
            case 3 -> "east";
            case 4 -> "down";
            default -> "up";
        };
    }

    private record OracleCase(BlockPos observeMin, BlockPos observeMax, long ticks, JsonArray actions) {
        static OracleCase load(Path path) {
            try (Reader reader = Files.newBufferedReader(path, StandardCharsets.UTF_8)) {
                JsonObject root = JsonParser.parseReader(reader).getAsJsonObject();
                return new OracleCase(
                    position(root.getAsJsonObject("observe_min")),
                    position(root.getAsJsonObject("observe_max")),
                    root.getAsJsonPrimitive("ticks").getAsLong(),
                    root.getAsJsonArray("actions")
                );
            } catch (IOException exception) {
                throw new IllegalStateException("failed to read oracle case: " + path, exception);
            }
        }
    }

    private static final class TraceWriter {
        private final Path output;
        private final JsonArray frames = new JsonArray();

        TraceWriter(Path output) {
            this.output = output;
        }

        void capture(GameTestHelper helper, OracleCase oracleCase, long tick) {
            JsonObject frame = new JsonObject();
            frame.addProperty("game_tick", tick);
            JsonArray blocks = new JsonArray();
            for (int y = oracleCase.observeMin().getY(); y <= oracleCase.observeMax().getY(); y++) {
                for (int z = oracleCase.observeMin().getZ(); z <= oracleCase.observeMax().getZ(); z++) {
                    for (int x = oracleCase.observeMin().getX(); x <= oracleCase.observeMax().getX(); x++) {
                        BlockPos position = new BlockPos(x, y, z);
                        BlockState state = helper.getBlockState(position);
                        if (!state.isAir()) {
                            blocks.add(snapshotBlock(position, state));
                        }
                    }
                }
            }
            frame.add("blocks", blocks);
            frames.add(frame);
        }

        void write() {
            JsonObject trace = new JsonObject();
            trace.add("frames", frames);
            trace.add("events", new JsonArray());
            try {
                Path parent = output.getParent();
                if (parent != null) {
                    Files.createDirectories(parent);
                }
                try (Writer writer = Files.newBufferedWriter(output, StandardCharsets.UTF_8)) {
                    writer.write(trace.toString());
                }
            } catch (IOException exception) {
                throw new IllegalStateException("failed to write vanilla trace: " + output, exception);
            }
        }

        private static JsonObject snapshotBlock(BlockPos position, BlockState state) {
            JsonObject block = new JsonObject();
            block.add("position", positionJson(position));
            block.add("state", stateJson(state));
            block.add("block_entity", null);
            return block;
        }

        private static JsonObject positionJson(BlockPos position) {
            JsonObject result = new JsonObject();
            result.addProperty("x", position.getX());
            result.addProperty("y", position.getY());
            result.addProperty("z", position.getZ());
            return result;
        }

        private static JsonObject stateJson(BlockState state) {
            JsonObject result = new JsonObject();
            String name = BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString();
            String kind = kindForState(name);
            result.addProperty("kind", kind);
            result.addProperty("bits", stateBits(state, name, kind));
            return result;
        }

        private static String kindForState(String name) {
            if (name.endsWith("_trapdoor")) return "Trapdoor";
            if (name.endsWith("_door")) return "Door";
            if (name.endsWith("_fence_gate")) return "FenceGate";
            if (name.endsWith("_button")) return "Button";
            return switch (name) {
                case "minecraft:glass" -> "Glass";
                case "minecraft:obsidian" -> "Immovable";
                case "minecraft:redstone_block" -> "RedstoneBlock";
                case "minecraft:redstone_wire" -> "RedstoneWire";
                case "minecraft:redstone_torch", "minecraft:redstone_wall_torch" -> "RedstoneTorch";
                case "minecraft:lever" -> "Lever";
                case "minecraft:stone_pressure_plate", "minecraft:light_weighted_pressure_plate", "minecraft:heavy_weighted_pressure_plate" -> "PressurePlate";
                case "minecraft:repeater" -> "Repeater";
                case "minecraft:comparator" -> "Comparator";
                case "minecraft:observer" -> "Observer";
                case "minecraft:redstone_lamp" -> "Lamp";
                case "minecraft:copper_bulb", "minecraft:exposed_copper_bulb", "minecraft:weathered_copper_bulb", "minecraft:oxidized_copper_bulb", "minecraft:waxed_copper_bulb" -> "CopperBulb";
                case "minecraft:daylight_detector" -> "DaylightDetector";
                case "minecraft:target" -> "Target";
                case "minecraft:note_block" -> "NoteBlock";
                case "minecraft:dropper" -> "Dropper";
                case "minecraft:dispenser" -> "Dispenser";
                case "minecraft:crafter" -> "Crafter";
                case "minecraft:tnt" -> "Tnt";
                case "minecraft:hopper" -> "Hopper";
                case "minecraft:barrel", "minecraft:chest", "minecraft:trapped_chest" -> "Container";
                case "minecraft:piston" -> "Piston";
                case "minecraft:sticky_piston" -> "StickyPiston";
                case "minecraft:piston_head" -> "PistonHead";
                case "minecraft:moving_piston" -> "MovingPiston";
                case "minecraft:slime_block" -> "SlimeBlock";
                case "minecraft:honey_block" -> "HoneyBlock";
                default -> "Solid";
            };
        }

        private static int stateBits(BlockState state, String name, String kind) {
            int bits = directionBits(value(state, "facing"));
            if (booleanValue(state, "powered") || (kind.equals("RedstoneTorch") && booleanValue(state, "lit"))) bits |= 1 << 3;
            bits |= integerValue(state, "power") << 4;
            bits |= Math.max(0, integerValue(state, "delay") - 1) << 8;
            if (booleanValue(state, "locked")) bits |= 1 << 10;
            if ("subtract".equals(value(state, "mode"))) bits |= 1 << 11;
            if (booleanValue(state, "extended")) bits |= 1 << 12;
            if (booleanValue(state, "lit") && !kind.equals("RedstoneTorch")) bits |= 1 << 13;
            if (kind.equals("Button") && !name.equals("minecraft:stone_button")) bits |= 1 << 14;
            if (name.equals("minecraft:redstone_wall_torch")) bits |= 1 << 15;
            if (booleanValue(state, "open")) bits |= 1 << 16;
            if (name.equals("minecraft:light_weighted_pressure_plate") || name.equals("minecraft:heavy_weighted_pressure_plate")) bits |= 1 << 17;
            return bits;
        }

        private static int directionBits(String direction) {
            return switch (direction) {
                case "south" -> 1;
                case "west" -> 2;
                case "east" -> 3;
                case "down" -> 4;
                case "up" -> 5;
                default -> 0;
            };
        }

        private static boolean booleanValue(BlockState state, String property) {
            return Boolean.parseBoolean(value(state, property));
        }

        private static int integerValue(BlockState state, String property) {
            try {
                return Integer.parseInt(value(state, property));
            } catch (NumberFormatException ignored) {
                return 0;
            }
        }

        private static String value(BlockState state, String name) {
            for (Property<?> property : state.getProperties()) {
                if (property.getName().equals(name)) {
                    return propertyNameUnchecked(state, property);
                }
            }
            return "";
        }

        @SuppressWarnings({"rawtypes", "unchecked"})
        private static String propertyNameUnchecked(BlockState state, Property property) {
            return property.getName((Comparable) state.getValue(property));
        }
    }
}
