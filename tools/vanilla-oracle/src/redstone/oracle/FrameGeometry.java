package redstone.oracle;

import net.minecraft.core.BlockPos;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;

record FrameGeometry(Scenario.Pos offset, Scenario.Pos size) {
    private static final int PADDING = 16;
    static final String ENV = "REDSTONE_ORACLE_FRAME";

    static FrameGeometry from(CompoundTag structure, Scenario scenario) {
        ListTag sizeTag = structure.getListOrEmpty("size");
        int sizeX = sizeTag.getIntOr(0, 0);
        int sizeY = sizeTag.getIntOr(1, 0);
        int sizeZ = sizeTag.getIntOr(2, 0);
        if (sizeX <= 0 || sizeY <= 0 || sizeZ <= 0) {
            throw new IllegalArgumentException("structure size 必须全部大于 0");
        }
        int minX = Integer.MAX_VALUE;
        int minY = Integer.MAX_VALUE;
        int minZ = Integer.MAX_VALUE;
        int maxX = Integer.MIN_VALUE;
        int maxY = Integer.MIN_VALUE;
        int maxZ = Integer.MIN_VALUE;
        int[] xs = {0, sizeX - 1};
        int[] ys = {0, sizeY - 1};
        int[] zs = {0, sizeZ - 1};
        for (int x : xs) {
            for (int y : ys) {
                for (int z : zs) {
                    BlockPos transformed = StructureTemplate.transform(
                        new BlockPos(x, y, z),
                        scenario.source.mirrorValue(),
                        scenario.source.rotationValue(),
                        BlockPos.ZERO
                    );
                    minX = Math.min(minX, transformed.getX());
                    minY = Math.min(minY, transformed.getY());
                    minZ = Math.min(minZ, transformed.getZ());
                    maxX = Math.max(maxX, transformed.getX());
                    maxY = Math.max(maxY, transformed.getY());
                    maxZ = Math.max(maxZ, transformed.getZ());
                }
            }
        }
        Scenario.Pos offset = new Scenario.Pos(PADDING - minX, PADDING - minY, PADDING - minZ);
        Scenario.Pos frameSize = new Scenario.Pos(
            Math.addExact(Math.subtractExact(maxX, minX), PADDING * 2 + 1),
            Math.addExact(Math.subtractExact(maxY, minY), PADDING * 2 + 1),
            Math.addExact(Math.subtractExact(maxZ, minZ), PADDING * 2 + 1)
        );
        return new FrameGeometry(offset, frameSize);
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
}
