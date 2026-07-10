package redstone.oracle;

import com.google.gson.JsonNull;
import com.google.gson.JsonObject;
import java.io.BufferedWriter;
import java.io.IOException;
import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.List;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.gametest.framework.GameTestHelper;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.BlockEventData;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.redstone.Orientation;
import net.minecraft.world.ticks.ScheduledTick;

final class OracleTraceRecorder implements AutoCloseable {
    private static final Field SUB_TICK_COUNT = field(Level.class, "subTickCount");

    private final Scenario scenario;
    private final FrameGeometry frame;
    private final Level level;
    private final BlockPos scenarioOrigin;
    private final long baseGameTime;
    private final long baseSubTickOrder;
    private final List<JsonObject> samples = new ArrayList<>();
    private int tick;

    OracleTraceRecorder(Scenario scenario, FrameGeometry frame, GameTestHelper helper) {
        this.scenario = scenario;
        this.frame = frame;
        this.level = helper.getLevel();
        if (helper.getTestRotation() != Rotation.NONE) {
            throw new IllegalStateException("微轨迹要求 GameTest 框架旋转为 none");
        }
        this.scenarioOrigin = helper.absolutePos(new BlockPos(
            frame.offset().x(),
            frame.offset().y(),
            frame.offset().z()
        ));
        this.baseGameTime = level.getGameTime();
        this.baseSubTickOrder = subTickCount(level);
        OracleHooks.setTraceRecorder(this);
    }

    void setTick(int tick) {
        this.tick = tick;
    }

    void writePending(BufferedWriter writer) throws IOException {
        for (JsonObject sample : samples) {
            writer.write(sample.toString());
            writer.newLine();
        }
        samples.clear();
    }

    @Override
    public void close() {
        OracleHooks.clearTraceRecorder(this);
    }

    void recordNeighbor(
        Level updatedLevel,
        BlockPos absolutePos,
        Block sourceBlock,
        Orientation orientation,
        boolean movedByPiston
    ) {
        Scenario.Pos pos = normalize(updatedLevel, absolutePos);
        if (pos == null) {
            return;
        }
        JsonObject root = sample("neighbor_update", pos);
        root.addProperty("source_block", blockName(sourceBlock));
        root.addProperty("moved_by_piston", movedByPiston);
        if (orientation == null) {
            root.add("orientation", JsonNull.INSTANCE);
        } else {
            root.addProperty("orientation", orientation.getIndex());
        }
        samples.add(root);
    }

    void recordScheduledTickQueued(ScheduledTick<?> scheduled) {
        if (!(scheduled.type() instanceof Block block)) {
            return;
        }
        Scenario.Pos pos = normalize(level, scheduled.pos());
        if (pos == null) {
            return;
        }
        JsonObject root = sample("scheduled_tick_queued", pos);
        root.addProperty("block", blockName(block));
        root.addProperty(
            "trigger_tick",
            Math.subtractExact(scheduled.triggerTick(), baseGameTime)
        );
        root.addProperty("priority", scheduled.priority().getValue());
        root.addProperty(
            "sub_tick_order",
            Math.subtractExact(scheduled.subTickOrder(), baseSubTickOrder)
        );
        samples.add(root);
    }

    void recordScheduledBlockTick(Level updatedLevel, BlockPos absolutePos, Block block) {
        Scenario.Pos pos = normalize(updatedLevel, absolutePos);
        if (pos == null) {
            return;
        }
        JsonObject root = sample("scheduled_tick_executed", pos);
        root.addProperty("block", blockName(block));
        samples.add(root);
    }

    void recordBlockEventQueued(
        Level updatedLevel,
        BlockPos absolutePos,
        Block block,
        int paramA,
        int paramB
    ) {
        recordBlockEvent(
            "block_event_queued",
            updatedLevel,
            absolutePos,
            block,
            paramA,
            paramB
        );
    }

    void recordBlockEventExecuted(Level updatedLevel, BlockEventData event) {
        recordBlockEvent(
            "block_event_executed",
            updatedLevel,
            event.pos(),
            event.block(),
            event.paramA(),
            event.paramB()
        );
    }

    private void recordBlockEvent(
        String kind,
        Level updatedLevel,
        BlockPos absolutePos,
        Block block,
        int paramA,
        int paramB
    ) {
        Scenario.Pos pos = normalize(updatedLevel, absolutePos);
        if (pos == null) {
            return;
        }
        JsonObject root = sample(kind, pos);
        root.addProperty("block", blockName(block));
        root.addProperty("param_a", paramA);
        root.addProperty("param_b", paramB);
        samples.add(root);
    }

    private JsonObject sample(String kind, Scenario.Pos pos) {
        JsonObject root = new JsonObject();
        root.addProperty("kind", kind);
        root.addProperty("tick", tick);
        JsonObject position = new JsonObject();
        position.addProperty("x", pos.x());
        position.addProperty("y", pos.y());
        position.addProperty("z", pos.z());
        root.add("pos", position);
        return root;
    }

    private Scenario.Pos normalize(Level updatedLevel, BlockPos absolutePos) {
        if (updatedLevel != level) {
            return null;
        }
        int x = Math.subtractExact(absolutePos.getX(), scenarioOrigin.getX());
        int y = Math.subtractExact(absolutePos.getY(), scenarioOrigin.getY());
        int z = Math.subtractExact(absolutePos.getZ(), scenarioOrigin.getZ());
        if (!insideFrame(x, y, z)) {
            return null;
        }
        return new Scenario.Pos(
            Math.addExact(scenario.source.origin().x(), x),
            Math.addExact(scenario.source.origin().y(), y),
            Math.addExact(scenario.source.origin().z(), z)
        );
    }

    private boolean insideFrame(int x, int y, int z) {
        int frameX = Math.addExact(frame.offset().x(), x);
        int frameY = Math.addExact(frame.offset().y(), y);
        int frameZ = Math.addExact(frame.offset().z(), z);
        return frameX >= 0
            && frameY >= 0
            && frameZ >= 0
            && frameX < frame.size().x()
            && frameY < frame.size().y()
            && frameZ < frame.size().z();
    }

    private static long subTickCount(Level level) {
        try {
            return SUB_TICK_COUNT.getLong(level);
        } catch (IllegalAccessException error) {
            throw new IllegalStateException("无法读取世界 sub_tick_order", error);
        }
    }

    private static String blockName(Block block) {
        Identifier id = BuiltInRegistries.BLOCK.getKey(block);
        return id == null ? "minecraft:unknown" : id.toString();
    }

    private static Field field(Class<?> owner, String name) {
        try {
            Field field = owner.getDeclaredField(name);
            field.setAccessible(true);
            return field;
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }
}
