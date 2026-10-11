/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.Label;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;

import java.lang.instrument.ClassFileTransformer;
import java.lang.instrument.Instrumentation;
import java.security.ProtectionDomain;
import java.util.Random;
import java.util.concurrent.ThreadLocalRandom;
import java.util.function.Consumer;
import java.util.function.Function;
import java.util.function.ToIntFunction;
import java.util.function.ToLongFunction;

/**
 * Java agent that routes robot randomness through {@link RobotRandomness}.
 *
 * <p>It rewrites three JDK methods when the JVM starts:
 * <ul>
 *   <li>{@code Math.random()} and {@code StrictMath.random()} first ask the
 *       {@code DoubleSupplier} hook and use its value unless it is NaN;</li>
 *   <li>{@code new Random()} first asks the {@code Supplier<Long>} hook for a
 *       seed and uses it unless it is null.</li>
 * </ul>
 * It also prepends classic Robocode's {@code robocode.util.Utils.getRandom()} (when that
 * class loads) so robot threads get their own generator (ADR 0010), and gives robots
 * deterministic identity hash codes ({@link IdentityHashes}, ADR 0013).
 * The hooks live in the system properties because code in {@code java.base}
 * cannot link against classes outside it. Without hooks the JDK behaves as usual.
 */
public final class DeterminismAgent {

    /** Hook in classic's {@code RobotThreadManager.waitForStop()} (a {@code Consumer<Thread>}). */
    static final String STOP_KEY = "robocodes.determinism.awaitStop";

    /** How long a robot's thread may take to finish on its own at the end of a round. */
    static final long STOP_WAIT_MILLIS = 60_000;

    private DeterminismAgent() {
    }

    /**
     * Classic gives a robot thread that is still running at the end of a round 3 seconds
     * of wall-clock time, then interrupts it, forces it to stop and punishes the robot.
     * Whether a robot that saves data in {@code onRoundEnded} finishes in time then
     * depends on the machine's load. Here it may finish on its own first.
     */
    static void awaitRobotThread(Object thread) {
        if (thread instanceof Thread t && t.isAlive()) {
            try {
                t.join(STOP_WAIT_MILLIS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        }
    }

    public static void premain(String args, Instrumentation instrumentation) throws Exception {
        // ThreadLocalRandom calls Random() once while initializing. Do that now, so it can
        // never consume a robot's seed depending on which thread happens to touch it first.
        ThreadLocalRandom.current();
        // Installed before Enum.hashCode() uses it; answers like the JVM until configured.
        System.getProperties().put(RobotRandomness.IDENTITY_HASH_KEY,
                (ToIntFunction<Object>) RobotRandomness::identityHash);
        // Work metering and the virtual clock for robot classes (ADR 0014).
        System.getProperties().put(RobotRandomness.FUEL_KEY, (Function<Object, Object>) RobotRandomness::fuel);
        System.getProperties().put(RobotRandomness.CLOCK_NANOS_KEY,
                (ToLongFunction<Object>) RobotRandomness::virtualNanos);
        System.getProperties().put(RobotRandomness.CLOCK_MILLIS_KEY,
                (ToLongFunction<Object>) RobotRandomness::virtualMillis);
        System.getProperties().put(RobotRandomness.TURN_KEY, (Runnable) RobotRandomness::turn);
        System.getProperties().put(STOP_KEY, (Consumer<Object>) DeterminismAgent::awaitRobotThread);
        instrumentation.addTransformer(new Transformer(), true);
        instrumentation.retransformClasses(Math.class, StrictMath.class, Random.class, Enum.class);
        RobotRandomness.markAgentInstalled();
    }

    /**
     * Prepends to {@code Utils.getRandom()}: {@code Object h = hook; if (h instanceof Supplier)
     * { Object r = h.get(); if (r != null) return (Random) r; }}.
     */
    private static byte[] patchUtils(byte[] classfileBuffer) {
        try {
            ClassReader reader = new ClassReader(classfileBuffer);
            ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS) {
                @Override
                protected String getCommonSuperClass(String type1, String type2) {
                    return "java/lang/Object";
                }
            };
            int[] patched = new int[1];
            reader.accept(new ClassVisitor(Opcodes.ASM9, writer) {
                @Override
                public MethodVisitor visitMethod(int access, String name, String descriptor, String signature,
                        String[] exceptions) {
                    MethodVisitor mv = super.visitMethod(access, name, descriptor, signature, exceptions);
                    if (!name.equals("getRandom") || !descriptor.equals("()Ljava/util/Random;")) {
                        return mv;
                    }
                    patched[0]++;
                    return new MethodVisitor(Opcodes.ASM9, mv) {
                        @Override
                        public void visitCode() {
                            super.visitCode();
                            Label original = new Label();
                            mv.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties",
                                    "()Ljava/util/Properties;", false);
                            mv.visitLdcInsn(RobotRandomness.UTILS_RANDOM_KEY);
                            mv.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                                    "(Ljava/lang/Object;)Ljava/lang/Object;", false);
                            mv.visitVarInsn(Opcodes.ASTORE, 0);
                            mv.visitVarInsn(Opcodes.ALOAD, 0);
                            mv.visitTypeInsn(Opcodes.INSTANCEOF, "java/util/function/Supplier");
                            mv.visitJumpInsn(Opcodes.IFEQ, original);
                            mv.visitVarInsn(Opcodes.ALOAD, 0);
                            mv.visitTypeInsn(Opcodes.CHECKCAST, "java/util/function/Supplier");
                            mv.visitMethodInsn(Opcodes.INVOKEINTERFACE, "java/util/function/Supplier", "get",
                                    "()Ljava/lang/Object;", true);
                            mv.visitVarInsn(Opcodes.ASTORE, 0);
                            mv.visitVarInsn(Opcodes.ALOAD, 0);
                            mv.visitJumpInsn(Opcodes.IFNULL, original);
                            mv.visitVarInsn(Opcodes.ALOAD, 0);
                            mv.visitTypeInsn(Opcodes.CHECKCAST, "java/util/Random");
                            mv.visitInsn(Opcodes.ARETURN);
                            mv.visitLabel(original);
                        }
                    };
                }
            }, 0);
            if (patched[0] != 1) {
                throw new IllegalStateException("expected Utils.getRandom(), found " + patched[0]);
            }
            return writer.toByteArray();
        } catch (Throwable t) {
            System.err.println("DeterminismAgent: could not patch Utils: " + t);
            Runtime.getRuntime().halt(3);
            return null;
        }
    }

    /** A robot class: identity hash codes (ADR 0013), then work metering (ADR 0014). */
    private static byte[] patchRobotClass(ClassLoader loader, String className, byte[] classfileBuffer) {
        byte[] bytes = classfileBuffer;
        try {
            byte[] hashed = IdentityHashes.patchRobotClass(loader, bytes);
            if (hashed != null) {
                bytes = hashed;
            }
        } catch (Throwable t) {
            System.err.println("robocode-s: could not give " + className + " identity hash codes: " + t);
        }
        try {
            byte[] metered = WorkMeter.patchRobotClass(bytes);
            if (metered != null) {
                bytes = metered;
            }
        } catch (Throwable t) {
            // E.g. a method that grows beyond 64 KiB: the class runs uncounted, and a clock
            // call in it reads real time.
            System.err.println("robocode-s: could not meter " + className + ": " + t);
        }
        return bytes == classfileBuffer ? null : bytes;
    }

    /**
     * Classic's robot side: every {@code executeImpl()} and {@code waitForBattleEndImpl()}
     * of a robot thread is a turn for the robot's virtual clock.
     */
    private static byte[] patchTurns(byte[] classfileBuffer) {
        try {
            ClassReader reader = new ClassReader(classfileBuffer);
            ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_MAXS);
            int[] patched = new int[1];
            reader.accept(new ClassVisitor(Opcodes.ASM9, writer) {
                @Override
                public MethodVisitor visitMethod(int access, String name, String descriptor, String signature,
                        String[] exceptions) {
                    MethodVisitor mv = super.visitMethod(access, name, descriptor, signature, exceptions);
                    if (!descriptor.equals("()V")
                            || !(name.equals("executeImpl") || name.equals("waitForBattleEndImpl"))) {
                        return mv;
                    }
                    patched[0]++;
                    return new MethodVisitor(Opcodes.ASM9, mv) {
                        @Override
                        public void visitCode() {
                            super.visitCode();
                            mv.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties",
                                    "()Ljava/util/Properties;", false);
                            mv.visitLdcInsn(RobotRandomness.TURN_KEY);
                            mv.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                                    "(Ljava/lang/Object;)Ljava/lang/Object;", false);
                            mv.visitTypeInsn(Opcodes.CHECKCAST, "java/lang/Runnable");
                            mv.visitMethodInsn(Opcodes.INVOKEINTERFACE, "java/lang/Runnable", "run", "()V", true);
                        }
                    };
                }
            }, 0);
            if (patched[0] != 2) {
                throw new IllegalStateException("expected executeImpl and waitForBattleEndImpl, found " + patched[0]);
            }
            return writer.toByteArray();
        } catch (Throwable t) {
            System.err.println("DeterminismAgent: could not patch BasicRobotProxy: " + t);
            return null;
        }
    }

    /** {@code RobotThreadManager.waitForStop()} first awaits the robot's thread ({@link #awaitRobotThread}). */
    private static byte[] patchStop(byte[] classfileBuffer) {
        try {
            ClassReader reader = new ClassReader(classfileBuffer);
            ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_MAXS);
            int[] patched = new int[1];
            reader.accept(new ClassVisitor(Opcodes.ASM9, writer) {
                @Override
                public MethodVisitor visitMethod(int access, String name, String descriptor, String signature,
                        String[] exceptions) {
                    MethodVisitor mv = super.visitMethod(access, name, descriptor, signature, exceptions);
                    if (!name.equals("waitForStop") || !descriptor.equals("()Z")) {
                        return mv;
                    }
                    patched[0]++;
                    return new MethodVisitor(Opcodes.ASM9, mv) {
                        @Override
                        public void visitCode() {
                            super.visitCode();
                            mv.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties",
                                    "()Ljava/util/Properties;", false);
                            mv.visitLdcInsn(STOP_KEY);
                            mv.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                                    "(Ljava/lang/Object;)Ljava/lang/Object;", false);
                            mv.visitTypeInsn(Opcodes.CHECKCAST, "java/util/function/Consumer");
                            mv.visitVarInsn(Opcodes.ALOAD, 0);
                            mv.visitFieldInsn(Opcodes.GETFIELD, "net/sf/robocode/host/security/RobotThreadManager",
                                    "runThread", "Ljava/lang/Thread;");
                            mv.visitMethodInsn(Opcodes.INVOKEINTERFACE, "java/util/function/Consumer", "accept",
                                    "(Ljava/lang/Object;)V", true);
                        }
                    };
                }
            }, 0);
            if (patched[0] != 1) {
                throw new IllegalStateException("expected RobotThreadManager.waitForStop(), found " + patched[0]);
            }
            return writer.toByteArray();
        } catch (Throwable t) {
            System.err.println("DeterminismAgent: could not patch RobotThreadManager: " + t);
            return null;
        }
    }

    private static final class Transformer implements ClassFileTransformer {
        @Override
        public byte[] transform(Module module, ClassLoader loader, String className, Class<?> classBeingRedefined,
                ProtectionDomain protectionDomain, byte[] classfileBuffer) {
            if ("robocode/util/Utils".equals(className)) {
                return patchUtils(classfileBuffer);
            }
            if ("java/lang/Enum".equals(className)) {
                try {
                    return IdentityHashes.patchEnum(classfileBuffer);
                } catch (Throwable t) {
                    System.err.println("DeterminismAgent: could not patch " + className + ": " + t);
                    return null;
                }
            }
            if ("net/sf/robocode/host/proxies/BasicRobotProxy".equals(className)) {
                return patchTurns(classfileBuffer);
            }
            if ("net/sf/robocode/host/security/RobotThreadManager".equals(className)) {
                return patchStop(classfileBuffer);
            }
            if (IdentityHashes.isRobotClassLoader(loader)) {
                return patchRobotClass(loader, className, classfileBuffer);
            }
            if (!"java/lang/Math".equals(className) && !"java/lang/StrictMath".equals(className)
                    && !"java/util/Random".equals(className)) {
                return null;
            }
            try {
                ClassReader reader = new ClassReader(classfileBuffer);
                ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS) {
                    @Override
                    protected String getCommonSuperClass(String type1, String type2) {
                        return "java/lang/Object"; // avoid class loading inside java.base transforms
                    }
                };
                reader.accept(new Rewriter(writer, className), 0);
                return writer.toByteArray();
            } catch (Throwable t) {
                t.printStackTrace();
                return null;
            }
        }
    }

    private static final class Rewriter extends ClassVisitor {
        private final String className;

        Rewriter(ClassVisitor next, String className) {
            super(Opcodes.ASM9, next);
            this.className = className;
        }

        @Override
        public MethodVisitor visitMethod(int access, String name, String descriptor, String signature,
                String[] exceptions) {
            MethodVisitor mv = super.visitMethod(access, name, descriptor, signature, exceptions);
            if (className.equals("java/util/Random") && name.equals("<init>") && descriptor.equals("()V")) {
                emitSeededConstructor(mv);
                return null; // original body is replaced; its code is reproduced below
            }
            if (!className.equals("java/util/Random") && name.equals("random") && descriptor.equals("()D")) {
                return new MethodVisitor(Opcodes.ASM9, mv) {
                    @Override
                    public void visitCode() {
                        super.visitCode();
                        emitMathRandomPrologue(mv);
                    }
                };
            }
            return mv;
        }
    }

    /** Pushes the hook object stored under {@code key} in the system properties. */
    private static void loadHook(MethodVisitor mv, String key) {
        mv.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties", "()Ljava/util/Properties;", false);
        mv.visitLdcInsn(key);
        mv.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                "(Ljava/lang/Object;)Ljava/lang/Object;", false);
    }

    /**
     * {@code Object h = hook; if (h instanceof DoubleSupplier) { double d = h.getAsDouble();
     * if (d == d) return d; }} followed by the original body.
     */
    private static void emitMathRandomPrologue(MethodVisitor mv) {
        Label original = new Label();
        loadHook(mv, RobotRandomness.MATH_RANDOM_KEY);
        mv.visitVarInsn(Opcodes.ASTORE, 0);
        mv.visitVarInsn(Opcodes.ALOAD, 0);
        mv.visitTypeInsn(Opcodes.INSTANCEOF, "java/util/function/DoubleSupplier");
        mv.visitJumpInsn(Opcodes.IFEQ, original);
        mv.visitVarInsn(Opcodes.ALOAD, 0);
        mv.visitTypeInsn(Opcodes.CHECKCAST, "java/util/function/DoubleSupplier");
        mv.visitMethodInsn(Opcodes.INVOKEINTERFACE, "java/util/function/DoubleSupplier", "getAsDouble", "()D", true);
        mv.visitVarInsn(Opcodes.DSTORE, 1);
        mv.visitVarInsn(Opcodes.DLOAD, 1);
        mv.visitVarInsn(Opcodes.DLOAD, 1);
        mv.visitInsn(Opcodes.DCMPL);
        mv.visitJumpInsn(Opcodes.IFNE, original); // NaN: not a robot thread
        mv.visitVarInsn(Opcodes.DLOAD, 1);
        mv.visitInsn(Opcodes.DRETURN);
        mv.visitLabel(original);
    }

    /**
     * Replaces {@code Random()} with: {@code Object h = hook; if (h instanceof Supplier) {
     * Long s = h.get(); if (s != null) { this(s); return; } } this(seedUniquifier() ^ System.nanoTime());}
     * The fallback is the JDK's own implementation.
     */
    private static void emitSeededConstructor(MethodVisitor mv) {
        Label original = new Label();
        mv.visitCode();
        loadHook(mv, RobotRandomness.RANDOM_SEED_KEY);
        mv.visitVarInsn(Opcodes.ASTORE, 1);
        mv.visitVarInsn(Opcodes.ALOAD, 1);
        mv.visitTypeInsn(Opcodes.INSTANCEOF, "java/util/function/Supplier");
        mv.visitJumpInsn(Opcodes.IFEQ, original);
        mv.visitVarInsn(Opcodes.ALOAD, 1);
        mv.visitTypeInsn(Opcodes.CHECKCAST, "java/util/function/Supplier");
        mv.visitMethodInsn(Opcodes.INVOKEINTERFACE, "java/util/function/Supplier", "get", "()Ljava/lang/Object;", true);
        mv.visitVarInsn(Opcodes.ASTORE, 2);
        mv.visitVarInsn(Opcodes.ALOAD, 2);
        mv.visitJumpInsn(Opcodes.IFNULL, original);
        mv.visitVarInsn(Opcodes.ALOAD, 0);
        mv.visitVarInsn(Opcodes.ALOAD, 2);
        mv.visitTypeInsn(Opcodes.CHECKCAST, "java/lang/Long");
        mv.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/lang/Long", "longValue", "()J", false);
        mv.visitMethodInsn(Opcodes.INVOKESPECIAL, "java/util/Random", "<init>", "(J)V", false);
        mv.visitInsn(Opcodes.RETURN);
        mv.visitLabel(original);
        mv.visitVarInsn(Opcodes.ALOAD, 0);
        mv.visitMethodInsn(Opcodes.INVOKESTATIC, "java/util/Random", "seedUniquifier", "()J", false);
        mv.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/System", "nanoTime", "()J", false);
        mv.visitInsn(Opcodes.LXOR);
        mv.visitMethodInsn(Opcodes.INVOKESPECIAL, "java/util/Random", "<init>", "(J)V", false);
        mv.visitInsn(Opcodes.RETURN);
        mv.visitMaxs(0, 0);
        mv.visitEnd();
    }
}
