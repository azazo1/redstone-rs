package redstone.oracle;

import com.google.gson.JsonObject;
import java.io.BufferedWriter;
import java.io.IOException;
import java.lang.reflect.Field;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.List;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.gametest.framework.GameTestHelper;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.redstone.CollectingNeighborUpdater;
import net.minecraft.world.level.redstone.NeighborUpdater;

final class NeighborTraceRecorder implements AutoCloseable {
    private static final Field LEVEL_UPDATER = field(Level.class, "neighborUpdater");
    private static final Field UPDATER_STACK = field(CollectingNeighborUpdater.class, "stack");

    private final Scenario scenario;
    private final FrameGeometry frame;
    private final GameTestHelper helper;
    private final CollectingNeighborUpdater updater;
    private final List<Sample> samples = new ArrayList<>();
    private int tick;

    NeighborTraceRecorder(Scenario scenario, FrameGeometry frame, GameTestHelper helper) {
        this.scenario = scenario;
        this.frame = frame;
        this.helper = helper;
        this.updater = updater(helper.getLevel());
        updater.setDebugListener(this::record);
    }

    void setTick(int tick) {
        this.tick = tick;
    }

    void writePending(BufferedWriter writer) throws IOException {
        for (Sample sample : samples) {
            JsonObject root = new JsonObject();
            root.addProperty("kind", "neighbor_update");
            root.addProperty("tick", sample.tick);
            JsonObject pos = new JsonObject();
            pos.addProperty("x", sample.pos.x());
            pos.addProperty("y", sample.pos.y());
            pos.addProperty("z", sample.pos.z());
            root.add("pos", pos);
            writer.write(root.toString());
            writer.newLine();
        }
        samples.clear();
    }

    @Override
    public void close() {
        updater.setDebugListener(null);
    }

    private void record(BlockPos absolutePos) {
        Object task = currentTask();
        String taskName = task.getClass().getSimpleName();
        if (taskName.equals("ShapeUpdate")) {
            return;
        }
        if (taskName.equals("MultiNeighborUpdate") && !isCurrentMultiPosition(task, absolutePos)) {
            return;
        }
        BlockPos relative = helper.relativePos(absolutePos);
        samples.add(new Sample(tick, scenario.source.fromFrame(relative, frame)));
    }

    private Object currentTask() {
        try {
            ArrayDeque<?> stack = (ArrayDeque<?>)UPDATER_STACK.get(updater);
            Object task = stack.peek();
            if (task == null) {
                throw new IllegalStateException("邻居更新调试回调缺少当前任务");
            }
            return task;
        } catch (IllegalAccessException error) {
            throw new IllegalStateException("无法读取邻居更新任务栈", error);
        }
    }

    private static boolean isCurrentMultiPosition(Object task, BlockPos position) {
        try {
            Field indexField = field(task.getClass(), "idx");
            Field sourceField = field(task.getClass(), "sourcePos");
            int index = indexField.getInt(task);
            Direction direction = NeighborUpdater.UPDATE_ORDER[index];
            BlockPos source = (BlockPos)sourceField.get(task);
            return source.relative(direction).equals(position);
        } catch (IllegalAccessException | IndexOutOfBoundsException error) {
            throw new IllegalStateException("无法读取多邻居更新状态", error);
        }
    }

    private static CollectingNeighborUpdater updater(Level level) {
        try {
            return (CollectingNeighborUpdater)LEVEL_UPDATER.get(level);
        } catch (IllegalAccessException error) {
            throw new IllegalStateException("无法读取世界邻居更新器", error);
        }
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

    private record Sample(int tick, Scenario.Pos pos) {
    }
}
