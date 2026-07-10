package redstone.oracle;

import net.minecraft.core.BlockPos;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.BlockEventData;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.redstone.Orientation;

public final class OracleHooks {
    private static volatile OracleTraceRecorder traceRecorder;
    private static volatile BlockPos testStart;

    private OracleHooks() {
    }

    static void setTraceRecorder(OracleTraceRecorder recorder) {
        traceRecorder = recorder;
    }

    static void clearTraceRecorder(OracleTraceRecorder recorder) {
        if (traceRecorder == recorder) {
            traceRecorder = null;
        }
    }

    static void configureTestStart(Scenario scenario, FrameGeometry frame) {
        testStart = scenario.oracleMicroTrace
            ? new BlockPos(
                Math.subtractExact(scenario.source.origin().x(), frame.offset().x()),
                Math.subtractExact(
                    Math.subtractExact(scenario.source.origin().y(), frame.offset().y()),
                    1
                ),
                Math.subtractExact(
                    Math.subtractExact(scenario.source.origin().z(), frame.offset().z()),
                    1
                )
            )
            : null;
    }

    public static BlockPos adjustGameTestStart(BlockPos generated) {
        BlockPos configured = testStart;
        return configured == null ? generated : configured;
    }

    public static void onNeighborUpdate(
        Level level,
        BlockPos pos,
        Block sourceBlock,
        Orientation orientation,
        boolean movedByPiston
    ) {
        OracleTraceRecorder recorder = traceRecorder;
        if (recorder != null) {
            recorder.recordNeighbor(level, pos, sourceBlock, orientation, movedByPiston);
        }
    }

    public static void onScheduledTickQueued(net.minecraft.world.ticks.ScheduledTick<?> scheduled) {
        OracleTraceRecorder recorder = traceRecorder;
        if (recorder != null) {
            recorder.recordScheduledTickQueued(scheduled);
        }
    }

    public static void onBlockStateChangeRequested(
        Level level,
        BlockPos pos,
        BlockState newState
    ) {
        OracleTraceRecorder recorder = traceRecorder;
        if (recorder != null) {
            BlockState oldState = level.getBlockState(pos);
            if (oldState != newState) {
                recorder.recordBlockChange(level, pos, oldState, newState);
            }
        }
    }

    public static void onScheduledBlockTick(
        ServerLevel level,
        BlockPos pos,
        Block block
    ) {
        OracleTraceRecorder recorder = traceRecorder;
        if (recorder != null) {
            recorder.recordScheduledBlockTick(level, pos, block);
        }
    }

    public static void onBlockEventQueued(
        boolean accepted,
        ServerLevel level,
        BlockPos pos,
        Block block,
        int paramA,
        int paramB
    ) {
        OracleTraceRecorder recorder = traceRecorder;
        if (accepted && recorder != null) {
            recorder.recordBlockEventQueued(level, pos, block, paramA, paramB);
        }
    }

    public static void onBlockEventExecuted(ServerLevel level, BlockEventData event) {
        OracleTraceRecorder recorder = traceRecorder;
        if (recorder != null) {
            recorder.recordBlockEventExecuted(level, event);
        }
    }
}
