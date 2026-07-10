package redstone.oracle;

import java.lang.instrument.ClassFileTransformer;
import java.security.ProtectionDomain;
import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;

final class OracleTransformer implements ClassFileTransformer {
    private static final String NEIGHBOR_TARGET = "net/minecraft/world/level/redstone/NeighborUpdater";
    private static final String GAME_TEST_SERVER_TARGET = "net/minecraft/gametest/framework/GameTestServer";
    private static final String LEVEL_TARGET = "net/minecraft/world/level/Level";
    private static final String LEVEL_TICKS_TARGET = "net/minecraft/world/ticks/LevelTicks";
    private static final String SERVER_LEVEL_TARGET = "net/minecraft/server/level/ServerLevel";
    private static final String METHOD = "executeUpdate";
    private static final String DESCRIPTOR = "(Lnet/minecraft/world/level/Level;"
        + "Lnet/minecraft/world/level/block/state/BlockState;"
        + "Lnet/minecraft/core/BlockPos;"
        + "Lnet/minecraft/world/level/block/Block;"
        + "Lnet/minecraft/world/level/redstone/Orientation;Z)V";
    private static final String HOOK_DESCRIPTOR = "(Lnet/minecraft/world/level/Level;"
        + "Lnet/minecraft/core/BlockPos;"
        + "Lnet/minecraft/world/level/block/Block;"
        + "Lnet/minecraft/world/level/redstone/Orientation;Z)V";

    @Override
    public byte[] transform(
        Module module,
        ClassLoader loader,
        String className,
        Class<?> classBeingRedefined,
        ProtectionDomain protectionDomain,
        byte[] classfileBuffer
    ) {
        if (!NEIGHBOR_TARGET.equals(className)
            && !GAME_TEST_SERVER_TARGET.equals(className)
            && !LEVEL_TARGET.equals(className)
            && !LEVEL_TICKS_TARGET.equals(className)
            && !SERVER_LEVEL_TARGET.equals(className)) {
            return null;
        }
        ClassReader reader = new ClassReader(classfileBuffer);
        ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_MAXS);
        ClassVisitor visitor = new ClassVisitor(Opcodes.ASM9, writer) {
            @Override
            public MethodVisitor visitMethod(
                int access,
                String name,
                String descriptor,
                String signature,
                String[] exceptions
            ) {
                MethodVisitor delegate = super.visitMethod(
                    access,
                    name,
                    descriptor,
                    signature,
                    exceptions
                );
                if (NEIGHBOR_TARGET.equals(className)
                    && METHOD.equals(name)
                    && DESCRIPTOR.equals(descriptor)) {
                    return neighborVisitor(delegate);
                }
                if (GAME_TEST_SERVER_TARGET.equals(className)
                    && name.equals("startTests")
                    && descriptor.equals("(Lnet/minecraft/server/level/ServerLevel;)V")) {
                    return gameTestStartVisitor(delegate);
                }
                if (LEVEL_TARGET.equals(className)
                    && name.equals("setBlock")
                    && descriptor.equals(
                        "(Lnet/minecraft/core/BlockPos;"
                            + "Lnet/minecraft/world/level/block/state/BlockState;II)Z"
                    )) {
                    return blockStateWriteVisitor(delegate);
                }
                if (LEVEL_TICKS_TARGET.equals(className)
                    && name.equals("schedule")
                    && descriptor.equals("(Lnet/minecraft/world/ticks/ScheduledTick;)V")) {
                    return scheduledTickQueueVisitor(delegate);
                }
                if (SERVER_LEVEL_TARGET.equals(className)
                    && name.equals("tickBlock")
                    && descriptor.equals(
                        "(Lnet/minecraft/core/BlockPos;Lnet/minecraft/world/level/block/Block;)V"
                    )) {
                    return scheduledBlockTickVisitor(delegate);
                }
                if (SERVER_LEVEL_TARGET.equals(className)
                    && name.equals("blockEvent")
                    && descriptor.equals(
                        "(Lnet/minecraft/core/BlockPos;"
                            + "Lnet/minecraft/world/level/block/Block;II)V"
                    )) {
                    return blockEventQueueVisitor(delegate);
                }
                if (SERVER_LEVEL_TARGET.equals(className)
                    && name.equals("doBlockEvent")
                    && descriptor.equals("(Lnet/minecraft/world/level/BlockEventData;)Z")) {
                    return blockEventExecutionVisitor(delegate);
                }
                return delegate;
            }

            private MethodVisitor neighborVisitor(MethodVisitor delegate) {
                return new MethodVisitor(Opcodes.ASM9, delegate) {
                    @Override
                    public void visitCode() {
                        super.visitCode();
                        super.visitVarInsn(Opcodes.ALOAD, 0);
                        super.visitVarInsn(Opcodes.ALOAD, 2);
                        super.visitVarInsn(Opcodes.ALOAD, 3);
                        super.visitVarInsn(Opcodes.ALOAD, 4);
                        super.visitVarInsn(Opcodes.ILOAD, 5);
                        super.visitMethodInsn(
                            Opcodes.INVOKESTATIC,
                            "redstone/oracle/OracleHooks",
                            "onNeighborUpdate",
                            HOOK_DESCRIPTOR,
                            false
                        );
                    }
                };
            }

            private MethodVisitor gameTestStartVisitor(MethodVisitor delegate) {
                return new MethodVisitor(Opcodes.ASM9, delegate) {
                    private boolean adjusted;

                    @Override
                    public void visitVarInsn(int opcode, int variable) {
                        super.visitVarInsn(opcode, variable);
                        if (!adjusted && opcode == Opcodes.ASTORE && variable == 3) {
                            adjusted = true;
                            super.visitVarInsn(Opcodes.ALOAD, 3);
                            super.visitMethodInsn(
                                Opcodes.INVOKESTATIC,
                                "redstone/oracle/OracleHooks",
                                "adjustGameTestStart",
                                "(Lnet/minecraft/core/BlockPos;)Lnet/minecraft/core/BlockPos;",
                                false
                            );
                            super.visitVarInsn(Opcodes.ASTORE, 3);
                        }
                    }
                };
            }

            private MethodVisitor scheduledTickQueueVisitor(MethodVisitor delegate) {
                return new MethodVisitor(Opcodes.ASM9, delegate) {
                    @Override
                    public void visitCode() {
                        super.visitCode();
                        super.visitVarInsn(Opcodes.ALOAD, 1);
                        super.visitMethodInsn(
                            Opcodes.INVOKESTATIC,
                            "redstone/oracle/OracleHooks",
                            "onScheduledTickQueued",
                            "(Lnet/minecraft/world/ticks/ScheduledTick;)V",
                            false
                        );
                    }
                };
            }

            private MethodVisitor blockStateWriteVisitor(MethodVisitor delegate) {
                return new MethodVisitor(Opcodes.ASM9, delegate) {
                    @Override
                    public void visitCode() {
                        super.visitCode();
                        super.visitVarInsn(Opcodes.ALOAD, 0);
                        super.visitVarInsn(Opcodes.ALOAD, 1);
                        super.visitVarInsn(Opcodes.ALOAD, 2);
                        super.visitMethodInsn(
                            Opcodes.INVOKESTATIC,
                            "redstone/oracle/OracleHooks",
                            "onBlockStateChangeRequested",
                            "(Lnet/minecraft/world/level/Level;"
                                + "Lnet/minecraft/core/BlockPos;"
                                + "Lnet/minecraft/world/level/block/state/BlockState;)V",
                            false
                        );
                    }
                };
            }

            private MethodVisitor scheduledBlockTickVisitor(MethodVisitor delegate) {
                return new MethodVisitor(Opcodes.ASM9, delegate) {
                    @Override
                    public void visitCode() {
                        super.visitCode();
                        super.visitVarInsn(Opcodes.ALOAD, 0);
                        super.visitVarInsn(Opcodes.ALOAD, 1);
                        super.visitVarInsn(Opcodes.ALOAD, 2);
                        super.visitMethodInsn(
                            Opcodes.INVOKESTATIC,
                            "redstone/oracle/OracleHooks",
                            "onScheduledBlockTick",
                            "(Lnet/minecraft/server/level/ServerLevel;"
                                + "Lnet/minecraft/core/BlockPos;"
                                + "Lnet/minecraft/world/level/block/Block;)V",
                            false
                        );
                    }
                };
            }

            private MethodVisitor blockEventQueueVisitor(MethodVisitor delegate) {
                return new MethodVisitor(Opcodes.ASM9, delegate) {
                    @Override
                    public void visitMethodInsn(
                        int opcode,
                        String owner,
                        String name,
                        String descriptor,
                        boolean isInterface
                    ) {
                        super.visitMethodInsn(opcode, owner, name, descriptor, isInterface);
                        if (opcode == Opcodes.INVOKEVIRTUAL
                            && owner.equals(
                                "it/unimi/dsi/fastutil/objects/ObjectLinkedOpenHashSet"
                            )
                            && name.equals("add")
                            && descriptor.equals("(Ljava/lang/Object;)Z")) {
                            super.visitInsn(Opcodes.DUP);
                            super.visitVarInsn(Opcodes.ALOAD, 0);
                            super.visitVarInsn(Opcodes.ALOAD, 1);
                            super.visitVarInsn(Opcodes.ALOAD, 2);
                            super.visitVarInsn(Opcodes.ILOAD, 3);
                            super.visitVarInsn(Opcodes.ILOAD, 4);
                            super.visitMethodInsn(
                                Opcodes.INVOKESTATIC,
                                "redstone/oracle/OracleHooks",
                                "onBlockEventQueued",
                                "(ZLnet/minecraft/server/level/ServerLevel;"
                                    + "Lnet/minecraft/core/BlockPos;"
                                    + "Lnet/minecraft/world/level/block/Block;II)V",
                                false
                            );
                        }
                    }
                };
            }

            private MethodVisitor blockEventExecutionVisitor(MethodVisitor delegate) {
                return new MethodVisitor(Opcodes.ASM9, delegate) {
                    @Override
                    public void visitCode() {
                        super.visitCode();
                        super.visitVarInsn(Opcodes.ALOAD, 0);
                        super.visitVarInsn(Opcodes.ALOAD, 1);
                        super.visitMethodInsn(
                            Opcodes.INVOKESTATIC,
                            "redstone/oracle/OracleHooks",
                            "onBlockEventExecuted",
                            "(Lnet/minecraft/server/level/ServerLevel;"
                                + "Lnet/minecraft/world/level/BlockEventData;)V",
                            false
                        );
                    }
                };
            }
        };
        reader.accept(visitor, 0);
        return writer.toByteArray();
    }
}
