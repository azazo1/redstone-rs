package redstone.oracle;

import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import java.util.Set;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.SupportType;
import net.minecraft.world.level.block.state.BlockState;

final class BlockTraitsReport {
    private static final Direction[] DIRECTIONS = {
        Direction.WEST,
        Direction.EAST,
        Direction.DOWN,
        Direction.UP,
        Direction.NORTH,
        Direction.SOUTH,
    };

    private BlockTraitsReport() {
    }

    static void write(Path output, String version, int dataVersion) throws Exception {
        List<BlockState> states = new ArrayList<>();
        BuiltInRegistries.BLOCK.forEach(block ->
            states.addAll(block.getStateDefinition().getPossibleStates())
        );
        states.sort(Comparator.comparingInt(Block::getId));
        Set<String> doesNotBlockHoppers = new TagResourceLoader("block")
            .resolve("minecraft:does_not_block_hoppers");

        JsonObject root = new JsonObject();
        root.addProperty("version", version);
        root.addProperty("data_version", dataVersion);
        JsonArray directionOrder = new JsonArray();
        for (Direction direction : DIRECTIONS) {
            directionOrder.add(direction.getName());
        }
        root.add("direction_order", directionOrder);

        JsonArray entries = new JsonArray();
        for (int index = 0; index < states.size(); index++) {
            BlockState state = states.get(index);
            int stateId = Block.getId(state);
            if (stateId != index) {
                throw new IllegalStateException(
                    "方块状态 ID 不连续: expected " + index + ", actual " + stateId
                );
            }
            JsonObject entry = new JsonObject();
            entry.addProperty("id", stateId);
            entry.addProperty(
                "redstone_conductor",
                state.isRedstoneConductor(EmptyBlockGetter.INSTANCE, BlockPos.ZERO)
            );
            entry.addProperty(
                "collision_full_block",
                state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO)
            );
            entry.addProperty(
                "does_not_block_hoppers",
                doesNotBlockHoppers.contains(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString())
            );
            entry.addProperty("full_support", supportMask(state, SupportType.FULL));
            entry.addProperty("center_support", supportMask(state, SupportType.CENTER));
            entry.addProperty("rigid_support", supportMask(state, SupportType.RIGID));
            entries.add(entry);
        }
        root.add("states", entries);

        Path normalized = output.toAbsolutePath().normalize();
        Path parent = normalized.getParent();
        if (parent != null) {
            Files.createDirectories(parent);
        }
        String json = new GsonBuilder()
            .disableHtmlEscaping()
            .setPrettyPrinting()
            .create()
            .toJson(root);
        Files.writeString(normalized, json + "\n");
        System.out.println("方块特征报告已生成: " + normalized);
        System.out.println("方块状态数量: " + states.size());
    }

    private static int supportMask(BlockState state, SupportType supportType) {
        int mask = 0;
        for (int index = 0; index < DIRECTIONS.length; index++) {
            if (state.isFaceSturdy(
                EmptyBlockGetter.INSTANCE,
                BlockPos.ZERO,
                DIRECTIONS[index],
                supportType
            )) {
                mask |= 1 << index;
            }
        }
        return mask;
    }
}
