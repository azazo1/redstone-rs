package redstone.oracle;

import java.util.List;
import net.minecraft.core.BlockPos;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;

record FrameGeometry(Scenario.Pos offset, Scenario.Pos size) {
    private static final int PADDING = 16;
    static final String ENV = "REDSTONE_ORACLE_FRAME";

    static FrameGeometry from(
        CompoundTag structure,
        List<CompoundTag> pasteStructures,
        Scenario scenario
    ) {
        if (pasteStructures.size() != scenario.source.pastes().size()) {
            throw new IllegalArgumentException("paste 结构数量与场景不匹配");
        }
        Bounds bounds = new Bounds();
        includeStructure(
            bounds,
            structure,
            Scenario.Pos.ZERO,
            scenario.source.mirrorValue(),
            scenario.source.rotationValue()
        );
        for (int index = 0; index < pasteStructures.size(); index++) {
            Scenario.Paste paste = scenario.source.pastes().get(index);
            Scenario.Pos relativeOrigin = new Scenario.Pos(
                Math.subtractExact(paste.origin().x(), scenario.source.origin().x()),
                Math.subtractExact(paste.origin().y(), scenario.source.origin().y()),
                Math.subtractExact(paste.origin().z(), scenario.source.origin().z())
            );
            includeStructure(
                bounds,
                pasteStructures.get(index),
                relativeOrigin,
                paste.mirrorValue(),
                paste.rotationValue()
            );
        }
        Scenario.Pos offset = new Scenario.Pos(
            Math.subtractExact(PADDING, bounds.minX),
            Math.subtractExact(PADDING, bounds.minY),
            Math.subtractExact(PADDING, bounds.minZ)
        );
        Scenario.Pos frameSize = new Scenario.Pos(
            Math.addExact(Math.subtractExact(bounds.maxX, bounds.minX), PADDING * 2 + 1),
            Math.addExact(Math.subtractExact(bounds.maxY, bounds.minY), PADDING * 2 + 1),
            Math.addExact(Math.subtractExact(bounds.maxZ, bounds.minZ), PADDING * 2 + 1)
        );
        return new FrameGeometry(offset, frameSize);
    }

    private static void includeStructure(
        Bounds bounds,
        CompoundTag structure,
        Scenario.Pos origin,
        net.minecraft.world.level.block.Mirror mirror,
        net.minecraft.world.level.block.Rotation rotation
    ) {
        ListTag sizeTag = structure.getListOrEmpty("size");
        int sizeX = sizeTag.getIntOr(0, 0);
        int sizeY = sizeTag.getIntOr(1, 0);
        int sizeZ = sizeTag.getIntOr(2, 0);
        if (sizeX <= 0 || sizeY <= 0 || sizeZ <= 0) {
            throw new IllegalArgumentException("structure size 必须全部大于 0");
        }
        int[] xs = {0, sizeX - 1};
        int[] ys = {0, sizeY - 1};
        int[] zs = {0, sizeZ - 1};
        for (int x : xs) {
            for (int y : ys) {
                for (int z : zs) {
                    BlockPos transformed = StructureTemplate.transform(
                        new BlockPos(x, y, z),
                        mirror,
                        rotation,
                        BlockPos.ZERO
                    ).offset(origin.x(), origin.y(), origin.z());
                    bounds.include(transformed);
                }
            }
        }
    }

    static FrameGeometry fromEnvironment() {
        String value = System.getenv(ENV);
        if (value == null) {
            throw new IllegalStateException("GameTest 子进程缺少 frame geometry");
        }
        String[] parts = value.split(",", -1);
        if (parts.length != 6) {
            throw new IllegalArgumentException("无效 frame geometry: " + value);
        }
        return new FrameGeometry(
            new Scenario.Pos(parse(parts[0]), parse(parts[1]), parse(parts[2])),
            new Scenario.Pos(parse(parts[3]), parse(parts[4]), parse(parts[5]))
        );
    }

    String encode() {
        return offset.x()
            + ","
            + offset.y()
            + ","
            + offset.z()
            + ","
            + size.x()
            + ","
            + size.y()
            + ","
            + size.z();
    }

    private static int parse(String value) {
        return Integer.parseInt(value);
    }

    private static final class Bounds {
        private int minX = Integer.MAX_VALUE;
        private int minY = Integer.MAX_VALUE;
        private int minZ = Integer.MAX_VALUE;
        private int maxX = Integer.MIN_VALUE;
        private int maxY = Integer.MIN_VALUE;
        private int maxZ = Integer.MIN_VALUE;

        void include(BlockPos pos) {
            minX = Math.min(minX, pos.getX());
            minY = Math.min(minY, pos.getY());
            minZ = Math.min(minZ, pos.getZ());
            maxX = Math.max(maxX, pos.getX());
            maxY = Math.max(maxY, pos.getY());
            maxZ = Math.max(maxZ, pos.getZ());
        }
    }
}
