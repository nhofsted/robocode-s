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
import org.objectweb.asm.Type;

import java.lang.instrument.ClassFileTransformer;
import java.lang.instrument.Instrumentation;
import java.security.ProtectionDomain;
import java.util.ArrayList;
import java.util.List;
import java.util.Random;
import java.util.concurrent.ThreadLocalRandom;
import java.util.function.Consumer;
import java.util.function.Function;
import java.util.function.LongSupplier;
import java.util.function.Predicate;
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

    /**
     * Hook in classic's {@code RobotThreadManager.waitForStop()} (a {@code Predicate<Thread>}:
     * true when the robot's threads are stopped already, ADR 0025).
     */
    static final String STOP_KEY = "robocodes.determinism.awaitStop";
    /**
     * Hooks in {@code java.lang.Thread}'s {@code run()}, {@code exit()} and {@code interrupt()}
     * (each a {@code Consumer<Object[]>}; {@code start} uses {@link RobotRandomness#THREAD_START_KEY}).
     */
    static final String THREAD_RUN_KEY = "robocodes.determinism.threadRun";
    static final String THREAD_EXIT_KEY = "robocodes.determinism.threadExit";
    static final String THREAD_INTERRUPT_KEY = "robocodes.determinism.threadInterrupt";
    /** Hooks in {@code Thread.sleep} and {@code Thread.join(long)} (each a {@code Predicate<Object[]>}: whether it did the call). */
    static final String THREAD_SLEEP_KEY = "robocodes.determinism.threadSleep";
    static final String THREAD_JOIN_KEY = "robocodes.determinism.threadJoin";
    /** Hook in {@code Object.wait(long)} (a {@code Predicate<Object[]>}: this, the timeout; whether it waited). */
    static final String WAIT_KEY = "robocodes.determinism.wait";
    /** Hooks in {@code LockSupport} (each a {@code Predicate<Object[]>}: the arguments; whether it did the call). */
    static final String PARK_KEY = "robocodes.determinism.park";
    static final String PARK_NANOS_KEY = "robocodes.determinism.parkNanos";
    static final String PARK_UNTIL_KEY = "robocodes.determinism.parkUntil";
    static final String UNPARK_KEY = "robocodes.determinism.unpark";
    /** Hooks for the JDK's {@code Timer} and {@code java.util.concurrent}: their clock (each a {@code LongSupplier}) and notifies ({@code Consumer<Object>}). */
    static final String JDK_NANOS_KEY = "robocodes.determinism.jdkNanos";
    static final String JDK_MILLIS_KEY = "robocodes.determinism.jdkMillis";
    static final String JDK_NOTIFY_KEY = "robocodes.determinism.jdkNotify";
    static final String JDK_NOTIFY_ALL_KEY = "robocodes.determinism.jdkNotifyAll";

    /** How long a robot's thread may take to finish on its own at the end of a round. */
    static final long STOP_WAIT_MILLIS = 60_000;

    private DeterminismAgent() {
    }

    /**
     * Classic gives a robot thread that is still running at the end of a round 3 seconds
     * of wall-clock time, then interrupts it, forces it to stop and punishes the robot.
     * Whether a robot that saves data in {@code onRoundEnded} finishes in time then
     * depends on the machine's load. Here it may finish on its own first. Then the robot's
     * other threads, if robot code started any, end the round deterministically
     * ({@link RobotThreads#endRound}).
     *
     * @return whether classic's wait for the robot's other threads is to be skipped
     */
    static boolean awaitRobotThread(Object thread) {
        if (thread instanceof Thread t && t.isAlive()) {
            try {
                t.join(STOP_WAIT_MILLIS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        }
        return thread instanceof Thread t && RobotThreads.endRound(t);
    }

    /** Robot code's thread calls ({@link ThreadCalls}): the call's name and arguments. */
    static void threadCall(Object[] call) {
        if (!"yield".equals(call[0])) {
            throw new IllegalArgumentException("unknown thread call " + call[0]);
        }
        RobotThreads.yieldNow();
    }

    /** {@code Thread.sleep(millis[, nanos])}: whether the robot's scheduler did it. */
    private static boolean sleep(Object[] args) {
        try {
            return RobotThreads.sleep((Long) args[0], args.length > 1 ? (Integer) args[1] : 0);
        } catch (InterruptedException e) {
            throw DeterminismAgent.<RuntimeException>sneaky(e);
        }
    }

    /** {@code o.wait(timeout)}: whether the robot's scheduler did it. */
    private static boolean waitOn(Object[] args) {
        try {
            return RobotThreads.waiting(args[0], (Long) args[1]);
        } catch (InterruptedException e) {
            throw DeterminismAgent.<RuntimeException>sneaky(e);
        }
    }

    /** {@code thread.join(millis)}: whether the robot's scheduler did it. */
    private static boolean join(Object[] args) {
        try {
            return RobotThreads.join((Thread) args[0], (Long) args[1]);
        } catch (InterruptedException e) {
            throw DeterminismAgent.<RuntimeException>sneaky(e);
        }
    }

    @SuppressWarnings("unchecked")
    private static <T extends Throwable> T sneaky(Throwable t) throws T {
        throw (T) t;
    }

    public static void premain(String args, Instrumentation instrumentation) throws Exception {
        // ThreadLocalRandom calls Random() once while initializing. Do that now, so it can
        // never consume a robot's seed depending on which thread happens to touch it first.
        ThreadLocalRandom.current();
        // Installed before Enum.hashCode() uses it; answers like the JVM until configured.
        System.getProperties().put(RobotRandomness.IDENTITY_HASH_KEY,
                (ToIntFunction<Object>) RobotRandomness::identityHash);
        System.getProperties().put(RobotRandomness.HASH_CODE_KEY, (ToIntFunction<Object>) RobotRandomness::hashCode);
        System.getProperties().put(RobotRandomness.TABLE_HASH_KEY, (ToIntFunction<Object>) RobotRandomness::tableHash);
        // Work metering and the virtual clock for robot classes (ADR 0014).
        System.getProperties().put(RobotRandomness.FUEL_KEY, (Function<Object, Object>) RobotRandomness::fuel);
        System.getProperties().put(RobotRandomness.CLOCK_NANOS_KEY,
                (ToLongFunction<Object>) RobotRandomness::virtualNanos);
        System.getProperties().put(RobotRandomness.CLOCK_MILLIS_KEY,
                (ToLongFunction<Object>) RobotRandomness::virtualMillis);
        System.getProperties().put(RobotRandomness.TURN_KEY, (Runnable) RobotRandomness::turn);
        System.getProperties().put(RobotRandomness.API_TRACE_KEY,
                (java.util.function.BiConsumer<Object, Object>) RobotRandomness::traceApi);
        System.getProperties().put(STOP_KEY, (Predicate<Object>) DeterminismAgent::awaitRobotThread);
        // Deterministic threads for robots (ADR 0025).
        System.getProperties().put(ThreadCalls.CALL_KEY, (Consumer<Object[]>) DeterminismAgent::threadCall);
        System.getProperties().put(ThreadCalls.LIMIT_KEY, (Consumer<Object>) RobotThreads::atLimit);
        System.getProperties().put(ThreadCalls.ENTER_KEY, (Runnable) RobotThreads::entering);
        // Threads robot code starts with start() are scheduled (ADR 0025); the others are
        // reported as not reproducible (ADR 0024): start(ThreadContainer) is how the JDK's
        // executors start theirs, and Timer's thread is started from JDK code.
        System.getProperties().put(RobotRandomness.THREAD_START_KEY, (Consumer<Object[]>) a -> {
            if (!RobotThreads.starting(a[0])) {
                RobotRandomness.threadStart(a[0]);
            }
        });
        System.getProperties().put(THREAD_RUN_KEY, (Consumer<Object[]>) a -> RobotThreads.entering());
        System.getProperties().put(THREAD_EXIT_KEY, (Consumer<Object[]>) a -> RobotThreads.exiting(a[0]));
        System.getProperties().put(THREAD_INTERRUPT_KEY, (Consumer<Object[]>) a -> RobotThreads.interrupting(a[0]));
        System.getProperties().put(THREAD_SLEEP_KEY, (Predicate<Object[]>) DeterminismAgent::sleep);
        System.getProperties().put(THREAD_JOIN_KEY, (Predicate<Object[]>) DeterminismAgent::join);
        System.getProperties().put(WAIT_KEY, (Predicate<Object[]>) DeterminismAgent::waitOn);
        System.getProperties().put(PARK_KEY, (Predicate<Object[]>) a -> RobotThreads.parking(-1, false));
        System.getProperties().put(PARK_NANOS_KEY, (Predicate<Object[]>) a -> {
            long nanos = (Long) a[a.length - 1];
            return nanos > 0 && RobotThreads.parking(nanos, false);
        });
        System.getProperties().put(PARK_UNTIL_KEY,
                (Predicate<Object[]>) a -> RobotThreads.parking((Long) a[a.length - 1], true));
        System.getProperties().put(UNPARK_KEY, (Predicate<Object[]>) a -> RobotThreads.unparking((Thread) a[0]));
        System.getProperties().put(JDK_NANOS_KEY, (LongSupplier) RobotThreads::nanoTime);
        System.getProperties().put(JDK_MILLIS_KEY, (LongSupplier) RobotThreads::currentTimeMillis);
        System.getProperties().put(JDK_NOTIFY_KEY, (Consumer<Object>) o -> RobotThreads.notifying(o, false));
        System.getProperties().put(JDK_NOTIFY_ALL_KEY, (Consumer<Object>) o -> RobotThreads.notifying(o, true));
        // Skipped turns by counted work in classic (#15).
        System.getProperties().put(RobotRandomness.PEER_ROUND_KEY, (Consumer<Object>) RobotRandomness::peerRound);
        System.getProperties().put(RobotRandomness.PEER_EXECUTE_KEY, (Consumer<Object[]>) RobotRandomness::peerExecute);
        System.getProperties().put(RobotRandomness.PEER_ASLEEP_KEY, (Predicate<Object>) RobotRandomness::peerAsleep);
        System.getProperties().put(RobotRandomness.PEER_LOAD_KEY, (Consumer<Object[]>) RobotRandomness::peerLoad);
        instrumentation.addTransformer(new Transformer(), true);
        List<Class<?>> retransform = new ArrayList<>(List.of(Math.class, StrictMath.class, Random.class,
                Enum.class, Thread.class, Object.class, java.util.concurrent.locks.LockSupport.class,
                java.util.HashMap.class, java.util.WeakHashMap.class, java.util.Hashtable.class,
                java.util.concurrent.ConcurrentHashMap.class, java.util.Objects.class));
        // The JDK's Timer and java.util.concurrent classes already loaded: their clock and notifies.
        for (Class<?> c : instrumentation.getAllLoadedClasses()) {
            if (instrumentation.isModifiableClass(c) && !retransform.contains(c)
                    && isJdkConcurrency(c.getName().replace('.', '/'))) {
                retransform.add(c);
            }
        }
        instrumentation.retransformClasses(retransform.toArray(new Class<?>[0]));
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
            byte[] metered = WorkMeter.patchRobotClass(bytes, loader);
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

    /**
     * {@code RobotThreadManager.waitForStop()} first awaits the robot's thread
     * ({@link #awaitRobotThread}), and returns true right away if that also stopped the
     * robot's other threads.
     */
    private static byte[] patchStop(byte[] classfileBuffer) {
        String manager = "net/sf/robocode/host/security/RobotThreadManager";
        try {
            ClassReader reader = new ClassReader(classfileBuffer);
            ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_MAXS);
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
                            mv.visitTypeInsn(Opcodes.CHECKCAST, "java/util/function/Predicate");
                            mv.visitVarInsn(Opcodes.ALOAD, 0);
                            mv.visitFieldInsn(Opcodes.GETFIELD, manager, "runThread", "Ljava/lang/Thread;");
                            mv.visitMethodInsn(Opcodes.INVOKEINTERFACE, "java/util/function/Predicate", "test",
                                    "(Ljava/lang/Object;)Z", true);
                            Label classic = new Label();
                            mv.visitJumpInsn(Opcodes.IFEQ, classic);
                            mv.visitInsn(Opcodes.ICONST_1);
                            mv.visitInsn(Opcodes.IRETURN);
                            mv.visitLabel(classic);
                            // Frames are expanded (EXPAND_FRAMES): the method's initial frame.
                            mv.visitFrame(Opcodes.F_NEW, 1, new Object[] {manager}, 0, new Object[0]);
                        }
                    };
                }
            }, ClassReader.EXPAND_FRAMES);
            if (patched[0] != 1) {
                throw new IllegalStateException("expected RobotThreadManager.waitForStop(), found " + patched[0]);
            }
            return writer.toByteArray();
        } catch (Throwable t) {
            System.err.println("DeterminismAgent: could not patch RobotThreadManager: " + t);
            return null;
        }
    }

    /** A hook in a JDK method: the method, its key, whether it may take over the call, whether it is static. */
    private record ThreadHook(String name, String descriptor, String key, boolean predicate, boolean isStatic) {
    }

    private static final List<ThreadHook> THREAD_HOOKS = List.of(
            new ThreadHook("start", "()V", RobotRandomness.THREAD_START_KEY, false, false),
            new ThreadHook("start", "(Ljdk/internal/vm/ThreadContainer;)V", RobotRandomness.THREAD_START_KEY, false,
                    false),
            new ThreadHook("run", "()V", THREAD_RUN_KEY, false, false),
            new ThreadHook("exit", "()V", THREAD_EXIT_KEY, false, false),
            new ThreadHook("interrupt", "()V", THREAD_INTERRUPT_KEY, false, false),
            new ThreadHook("sleep", "(J)V", THREAD_SLEEP_KEY, true, true),
            new ThreadHook("sleep", "(JI)V", THREAD_SLEEP_KEY, true, true),
            new ThreadHook("join", "(J)V", THREAD_JOIN_KEY, true, false));
    /** {@code Object.wait(long)}: {@code wait()} and {@code wait(long, int)} call it (ADR 0025). */
    private static final List<ThreadHook> OBJECT_HOOKS = List.of(
            new ThreadHook("wait", "(J)V", WAIT_KEY, true, false));
    /** {@code LockSupport}: what every {@code java.util.concurrent} lock, condition and queue blocks in (ADR 0025). */
    private static final List<ThreadHook> LOCK_SUPPORT_HOOKS = List.of(
            new ThreadHook("park", "(Ljava/lang/Object;)V", PARK_KEY, true, true),
            new ThreadHook("park", "()V", PARK_KEY, true, true),
            new ThreadHook("parkNanos", "(Ljava/lang/Object;J)V", PARK_NANOS_KEY, true, true),
            new ThreadHook("parkNanos", "(J)V", PARK_NANOS_KEY, true, true),
            new ThreadHook("parkUntil", "(Ljava/lang/Object;J)V", PARK_UNTIL_KEY, true, true),
            new ThreadHook("parkUntil", "(J)V", PARK_UNTIL_KEY, true, true),
            new ThreadHook("unpark", "(Ljava/lang/Thread;)V", UNPARK_KEY, true, true));
    /** {@code run()} of a class outside robot code that extends {@code Thread} (a {@code Timer}'s, Kotlin's {@code thread {}}). */
    private static final List<ThreadHook> THREAD_SUBCLASS_HOOKS = List.of(
            new ThreadHook("run", "()V", THREAD_RUN_KEY, false, false));

    /** The JDK's classes whose clock and notifies the robot's scheduler takes over on its threads (ADR 0025). */
    static boolean isJdkConcurrency(String className) {
        return className.startsWith("java/util/concurrent/") || className.equals("java/util/Timer")
                || className.startsWith("java/util/Timer$") || className.equals("java/util/TimerThread")
                || className.equals("java/util/TaskQueue");
    }

    /**
     * The JDK's {@code Timer} and {@code java.util.concurrent}: {@code System.nanoTime()} and
     * {@code currentTimeMillis()} read their hooks (on a scheduled robot thread the robot's
     * virtual clock, in classic's time, so their timed waits fit), and {@code notify()} and
     * {@code notifyAll()} first tell the scheduler ({@code DUP; hook.accept}). Straight-line.
     */
    static byte[] patchJdkConcurrency(byte[] classfileBuffer) {
        ClassReader reader = new ClassReader(classfileBuffer);
        org.objectweb.asm.tree.ClassNode node = new org.objectweb.asm.tree.ClassNode(Opcodes.ASM9);
        reader.accept(node, 0);
        boolean changed = false;
        for (org.objectweb.asm.tree.MethodNode method : node.methods) {
            for (org.objectweb.asm.tree.AbstractInsnNode insn = method.instructions.getFirst(); insn != null;
                    insn = insn.getNext()) {
                if (!(insn instanceof org.objectweb.asm.tree.MethodInsnNode call)) {
                    continue;
                }
                String key = null;
                boolean clock = false;
                if (call.owner.equals("java/lang/System") && call.desc.equals("()J")
                        && (call.name.equals("nanoTime") || call.name.equals("currentTimeMillis"))) {
                    key = call.name.equals("nanoTime") ? JDK_NANOS_KEY : JDK_MILLIS_KEY;
                    clock = true;
                } else if (call.getOpcode() == Opcodes.INVOKEVIRTUAL && call.desc.equals("()V")
                        && (call.name.equals("notify") || call.name.equals("notifyAll"))) {
                    key = call.name.equals("notify") ? JDK_NOTIFY_KEY : JDK_NOTIFY_ALL_KEY;
                }
                if (key == null) {
                    continue;
                }
                org.objectweb.asm.tree.InsnList code = new org.objectweb.asm.tree.InsnList();
                if (!clock) {
                    code.add(new org.objectweb.asm.tree.InsnNode(Opcodes.DUP));
                }
                code.add(new org.objectweb.asm.tree.MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/System",
                        "getProperties", "()Ljava/util/Properties;", false));
                code.add(new org.objectweb.asm.tree.LdcInsnNode(key));
                code.add(new org.objectweb.asm.tree.MethodInsnNode(Opcodes.INVOKEVIRTUAL, "java/util/Properties",
                        "get", "(Ljava/lang/Object;)Ljava/lang/Object;", false));
                if (clock) {
                    code.add(new org.objectweb.asm.tree.TypeInsnNode(Opcodes.CHECKCAST,
                            "java/util/function/LongSupplier"));
                    code.add(new org.objectweb.asm.tree.MethodInsnNode(Opcodes.INVOKEINTERFACE,
                            "java/util/function/LongSupplier", "getAsLong", "()J", true));
                    method.instructions.insertBefore(insn, code);
                    org.objectweb.asm.tree.AbstractInsnNode next = insn.getPrevious();
                    method.instructions.remove(insn);
                    insn = next;
                } else {
                    code.add(new org.objectweb.asm.tree.TypeInsnNode(Opcodes.CHECKCAST, "java/util/function/Consumer"));
                    code.add(new org.objectweb.asm.tree.InsnNode(Opcodes.SWAP));
                    code.add(new org.objectweb.asm.tree.MethodInsnNode(Opcodes.INVOKEINTERFACE,
                            "java/util/function/Consumer", "accept", "(Ljava/lang/Object;)V", true));
                    method.instructions.insertBefore(insn, code);
                }
                changed = true;
            }
        }
        if (!changed) {
            return null;
        }
        ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_MAXS);
        node.accept(writer);
        return writer.toByteArray();
    }

    /**
     * {@code java.lang.Thread}: these methods first call their hook, if it is installed, with
     * {@code new Object[] {this (unless static), args...}}: {@code start()} and
     * {@code start(ThreadContainer)} (reported, ADR 0024), {@code run()},
     * {@code exit()} (which the JVM calls when a thread ends) and {@code interrupt()} a
     * {@code Consumer}; {@code sleep(long)}, {@code sleep(long, int)} and {@code join(long)}
     * (which {@code join()} and {@code join(long, int)} call) a {@code Predicate} that returns
     * true when it did the call itself. The robot's scheduler (ADR 0025) learns of new robot
     * threads, makes them wait for the CPU, hands it on when they end, and takes over robot
     * code's sleeps and joins. For other threads nothing changes, not even their stack traces.
     */
    private static byte[] patchThread(byte[] classfileBuffer) {
        return patchHooks(classfileBuffer, "java/lang/Thread", THREAD_HOOKS, true);
    }

    /** Prologue hooks in a class's methods ({@link #emitThreadHook}); {@code all}: every hook must match. */
    private static byte[] patchHooks(byte[] classfileBuffer, String className, List<ThreadHook> hooks, boolean all) {
        try {
            ClassReader reader = new ClassReader(classfileBuffer);
            ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_MAXS);
            int[] patched = new int[1];
            reader.accept(new ClassVisitor(Opcodes.ASM9, writer) {
                @Override
                public MethodVisitor visitMethod(int access, String name, String descriptor, String signature,
                        String[] exceptions) {
                    MethodVisitor mv = super.visitMethod(access, name, descriptor, signature, exceptions);
                    boolean isStatic = (access & Opcodes.ACC_STATIC) != 0;
                    ThreadHook hook = hooks.stream()
                            .filter(h -> h.name().equals(name) && h.descriptor().equals(descriptor)
                                    && isStatic == h.isStatic())
                            .findFirst().orElse(null);
                    if (hook == null || (access & (Opcodes.ACC_ABSTRACT | Opcodes.ACC_NATIVE)) != 0) {
                        return mv;
                    }
                    patched[0]++;
                    return new MethodVisitor(Opcodes.ASM9, mv) {
                        @Override
                        public void visitCode() {
                            super.visitCode();
                            emitThreadHook(mv, hook, isStatic, className);
                        }
                    };
                }
            }, ClassReader.EXPAND_FRAMES);
            if (all ? patched[0] != hooks.size() : patched[0] == 0) {
                if (!all) {
                    return null;
                }
                throw new IllegalStateException("expected " + hooks + ", found " + patched[0]);
            }
            return writer.toByteArray();
        } catch (Throwable t) {
            System.err.println("DeterminismAgent: could not patch " + className + ": " + t);
            return null;
        }
    }

    /**
     * {@code Object h = hook; if (h instanceof F) { if (h.test/accept(new Object[] {...})) return; }}
     * followed by the original body. Frames are expanded (EXPAND_FRAMES).
     */
    private static void emitThreadHook(MethodVisitor mv, ThreadHook hook, boolean isStatic, String className) {
        String type = hook.predicate() ? "java/util/function/Predicate" : "java/util/function/Consumer";
        Type[] args = Type.getArgumentTypes(hook.descriptor());
        List<Object> locals = new ArrayList<>();
        if (!isStatic) {
            locals.add(className);
        }
        for (Type a : args) {
            locals.add(a.getSort() == Type.LONG ? Opcodes.LONG
                    : a.getSort() == Type.OBJECT ? a.getInternalName() : Opcodes.INTEGER);
        }
        Object[] frame = locals.toArray();
        Label call = new Label();
        Label original = new Label();
        loadHook(mv, hook.key());
        mv.visitInsn(Opcodes.DUP);
        mv.visitTypeInsn(Opcodes.INSTANCEOF, type);
        mv.visitJumpInsn(Opcodes.IFNE, call);
        mv.visitInsn(Opcodes.POP);
        mv.visitJumpInsn(Opcodes.GOTO, original);
        mv.visitLabel(call);
        mv.visitFrame(Opcodes.F_NEW, frame.length, frame, 1, new Object[] {"java/lang/Object"});
        mv.visitTypeInsn(Opcodes.CHECKCAST, type);
        int values = (isStatic ? 0 : 1) + args.length;
        mv.visitLdcInsn(values);
        mv.visitTypeInsn(Opcodes.ANEWARRAY, "java/lang/Object");
        int slot = 0;
        int index = 0;
        if (!isStatic) {
            mv.visitInsn(Opcodes.DUP);
            mv.visitLdcInsn(index++);
            mv.visitVarInsn(Opcodes.ALOAD, slot++);
            mv.visitInsn(Opcodes.AASTORE);
        }
        for (Type a : args) {
            mv.visitInsn(Opcodes.DUP);
            mv.visitLdcInsn(index++);
            mv.visitVarInsn(a.getOpcode(Opcodes.ILOAD), slot);
            if (a.getSort() == Type.LONG) {
                mv.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/Long", "valueOf", "(J)Ljava/lang/Long;", false);
            } else if (a.getSort() == Type.INT) {
                mv.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/Integer", "valueOf", "(I)Ljava/lang/Integer;",
                        false);
            }
            mv.visitInsn(Opcodes.AASTORE);
            slot += a.getSize();
        }
        if (hook.predicate()) {
            mv.visitMethodInsn(Opcodes.INVOKEINTERFACE, type, "test", "(Ljava/lang/Object;)Z", true);
            mv.visitJumpInsn(Opcodes.IFEQ, original);
            mv.visitInsn(Opcodes.RETURN);
        } else {
            mv.visitMethodInsn(Opcodes.INVOKEINTERFACE, type, "accept", "(Ljava/lang/Object;)V", true);
        }
        mv.visitLabel(original);
        mv.visitFrame(Opcodes.F_NEW, frame.length, frame, 0, new Object[0]);
    }

    /**
     * Classic's battle side skips turns of robots by their counted work instead of wall time
     * (#15), as robocode-s's server does in deterministic mode. Classic's own skipping stays:
     * it waits for every robot (the oracle runs with {@code -Ddebug=true}), and a robot that
     * did more work than the budget stays asleep for the turns it would have been busy
     * ({@link RobotRandomness#lateTurns}). Its {@code checkSkippedTurn} then sees it answer
     * late, with classic's events, kept commands and disabling.
     * <ul>
     *   <li>{@code startRound(long, int)} first calls {@code peerRound(this)};</li>
     *   <li>{@code executeImpl(ExecCommands)} first calls {@code peerExecute({this, currentExecutionTime, commands})};</li>
     *   <li>{@code waitWakeup()} returns right away if {@code peerAsleep(this)}. Only
     *       {@code Battle.wakeupRobots} calls it;</li>
     *   <li>{@code performLoadCommands()} first calls {@code peerLoad({this, commands})}:
     *       a late robot's new commands are loaded only after its late turns.</li>
     * </ul>
     */
    private static byte[] patchPeer(byte[] classfileBuffer) {
        String peer = "net/sf/robocode/battle/peer/RobotPeer";
        try {
            ClassReader reader = new ClassReader(classfileBuffer);
            ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_MAXS);
            int[] patched = new int[1];
            reader.accept(new ClassVisitor(Opcodes.ASM9, writer) {
                @Override
                public MethodVisitor visitMethod(int access, String name, String descriptor, String signature,
                        String[] exceptions) {
                    MethodVisitor mv = super.visitMethod(access, name, descriptor, signature, exceptions);
                    boolean round = name.equals("startRound") && descriptor.equals("(JI)V");
                    boolean execute = name.equals("executeImpl")
                            && descriptor.startsWith("(Lnet/sf/robocode/peer/ExecCommands;)");
                    boolean wake = name.equals("waitWakeup") && descriptor.equals("()V");
                    boolean load = name.equals("performLoadCommands") && descriptor.equals("()V");
                    if (!round && !execute && !wake && !load) {
                        return mv;
                    }
                    patched[0]++;
                    return new MethodVisitor(Opcodes.ASM9, mv) {
                        @Override
                        public void visitCode() {
                            super.visitCode();
                            loadHook(mv, round ? RobotRandomness.PEER_ROUND_KEY
                                    : execute ? RobotRandomness.PEER_EXECUTE_KEY
                                    : load ? RobotRandomness.PEER_LOAD_KEY : RobotRandomness.PEER_ASLEEP_KEY);
                            if (load || execute) {
                                // {this, (execute: Integer.valueOf(currentExecutionTime),) commands}
                                mv.visitTypeInsn(Opcodes.CHECKCAST, "java/util/function/Consumer");
                                mv.visitInsn(execute ? Opcodes.ICONST_3 : Opcodes.ICONST_2);
                                mv.visitTypeInsn(Opcodes.ANEWARRAY, "java/lang/Object");
                                mv.visitInsn(Opcodes.DUP);
                                mv.visitInsn(Opcodes.ICONST_0);
                                mv.visitVarInsn(Opcodes.ALOAD, 0);
                                mv.visitInsn(Opcodes.AASTORE);
                                if (execute) {
                                    mv.visitInsn(Opcodes.DUP);
                                    mv.visitInsn(Opcodes.ICONST_1);
                                    mv.visitVarInsn(Opcodes.ALOAD, 0);
                                    mv.visitFieldInsn(Opcodes.GETFIELD, peer, "currentExecutionTime", "I");
                                    mv.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/Integer", "valueOf",
                                            "(I)Ljava/lang/Integer;", false);
                                    mv.visitInsn(Opcodes.AASTORE);
                                }
                                mv.visitInsn(Opcodes.DUP);
                                mv.visitInsn(execute ? Opcodes.ICONST_2 : Opcodes.ICONST_1);
                                mv.visitVarInsn(Opcodes.ALOAD, 0);
                                mv.visitFieldInsn(Opcodes.GETFIELD, peer, "commands",
                                        "Ljava/util/concurrent/atomic/AtomicReference;");
                                mv.visitInsn(Opcodes.AASTORE);
                                mv.visitMethodInsn(Opcodes.INVOKEINTERFACE, "java/util/function/Consumer", "accept",
                                        "(Ljava/lang/Object;)V", true);
                            } else if (round) {
                                mv.visitTypeInsn(Opcodes.CHECKCAST, "java/util/function/Consumer");
                                mv.visitVarInsn(Opcodes.ALOAD, 0);
                                mv.visitMethodInsn(Opcodes.INVOKEINTERFACE, "java/util/function/Consumer", "accept",
                                        "(Ljava/lang/Object;)V", true);
                            } else {
                                Label original = new Label();
                                mv.visitTypeInsn(Opcodes.CHECKCAST, "java/util/function/Predicate");
                                mv.visitVarInsn(Opcodes.ALOAD, 0);
                                mv.visitMethodInsn(Opcodes.INVOKEINTERFACE, "java/util/function/Predicate", "test",
                                        "(Ljava/lang/Object;)Z", true);
                                mv.visitJumpInsn(Opcodes.IFEQ, original);
                                mv.visitInsn(Opcodes.RETURN);
                                mv.visitLabel(original);
                                // Frames are expanded (EXPAND_FRAMES): the method's initial frame.
                                mv.visitFrame(Opcodes.F_NEW, 1, new Object[] {peer}, 0, new Object[0]);
                            }
                        }
                    };
                }
            }, ClassReader.EXPAND_FRAMES);
            if (patched[0] != 4) {
                throw new IllegalStateException("expected startRound, executeImpl, waitWakeup and performLoadCommands, "
                        + "found " + patched[0]);
            }
            return writer.toByteArray();
        } catch (Throwable t) {
            System.err.println("DeterminismAgent: could not patch RobotPeer: " + t);
            return null;
        }
    }

    /**
     * Classic's {@code Battle.wakeupSerial} and {@code wakeupParallel} wait for every robot
     * whose thread runs, not only for living ones ({@code isRunning()} instead of
     * {@code isAlive()}), as robocode-s's server waits for a robot's final {@code Execute}.
     * A robot that just died still runs its handlers and then ends or waits for the round's
     * end; without this, its code ran on unwatched: a robot looping there crashed at its
     * work cutoff (ADR 0018) at a random time, and its drained energy changed what its
     * bullets still gave it (rrh.Iwillfindyou).
     */
    private static byte[] patchBattle(byte[] classfileBuffer) {
        try {
            ClassReader reader = new ClassReader(classfileBuffer);
            ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_MAXS);
            int[] patched = new int[1];
            reader.accept(new ClassVisitor(Opcodes.ASM9, writer) {
                @Override
                public MethodVisitor visitMethod(int access, String name, String descriptor, String signature,
                        String[] exceptions) {
                    MethodVisitor mv = super.visitMethod(access, name, descriptor, signature, exceptions);
                    if (!name.equals("wakeupSerial") && !name.equals("wakeupParallel")) {
                        return mv;
                    }
                    return new MethodVisitor(Opcodes.ASM9, mv) {
                        @Override
                        public void visitMethodInsn(int opcode, String owner, String method, String desc,
                                boolean isInterface) {
                            if (owner.equals("net/sf/robocode/battle/peer/RobotPeer") && method.equals("isAlive")
                                    && desc.equals("()Z")) {
                                patched[0]++;
                                method = "isRunning";
                            }
                            super.visitMethodInsn(opcode, owner, method, desc, isInterface);
                        }
                    };
                }
            }, 0);
            if (patched[0] != 2) {
                throw new IllegalStateException("expected isAlive() in wakeupSerial and wakeupParallel, found "
                        + patched[0]);
            }
            return writer.toByteArray();
        } catch (Throwable t) {
            System.err.println("DeterminismAgent: could not patch Battle: " + t);
            return null;
        }
    }

    private static boolean extendsThread(byte[] classfileBuffer) {
        try {
            return "java/lang/Thread".equals(new ClassReader(classfileBuffer).getSuperName());
        } catch (RuntimeException e) {
            return false;
        }
    }

    private static final class Transformer implements ClassFileTransformer {
        @Override
        public byte[] transform(Module module, ClassLoader loader, String className, Class<?> classBeingRedefined,
                ProtectionDomain protectionDomain, byte[] classfileBuffer) {
            if ("robocode/util/Utils".equals(className)) {
                return patchUtils(classfileBuffer);
            }
            if (className != null && isJdkConcurrency(className)) {
                byte[] bytes = classfileBuffer;
                try {
                    if (IdentityHashes.HASHING_CLASSES.contains(className)) {
                        bytes = IdentityHashes.patchHashCalls(bytes);
                    }
                    byte[] clocked = patchJdkConcurrency(bytes);
                    if (clocked != null) {
                        bytes = clocked;
                    }
                    if (className.equals("java/util/concurrent/locks/LockSupport")) {
                        bytes = patchHooks(bytes, className, LOCK_SUPPORT_HOOKS, true);
                    } else if (className.equals("java/util/TimerThread")
                            || className.equals("java/util/concurrent/ForkJoinWorkerThread")) {
                        byte[] hooked = patchHooks(bytes, className, THREAD_SUBCLASS_HOOKS, false);
                        if (hooked != null) {
                            bytes = hooked;
                        }
                    }
                } catch (Throwable t) {
                    System.err.println("DeterminismAgent: could not patch " + className + ": " + t);
                    return null;
                }
                return bytes == classfileBuffer ? null : bytes;
            }
            if (IdentityHashes.HASHING_CLASSES.contains(className)) {
                try {
                    return IdentityHashes.patchHashCalls(classfileBuffer);
                } catch (Throwable t) {
                    System.err.println("DeterminismAgent: could not patch " + className + ": " + t);
                    return null;
                }
            }
            if ("java/lang/Object".equals(className)) {
                return patchHooks(classfileBuffer, className, OBJECT_HOOKS, true);
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
            if ("java/lang/Thread".equals(className)) {
                return patchThread(classfileBuffer);
            }
            if ("net/sf/robocode/battle/peer/RobotPeer".equals(className)) {
                return patchPeer(classfileBuffer);
            }
            if ("net/sf/robocode/battle/Battle".equals(className)) {
                return patchBattle(classfileBuffer);
            }
            if (IdentityHashes.isRobotClassLoader(loader)) {
                return patchRobotClass(loader, className, classfileBuffer);
            }
            if (className != null && !className.startsWith("java/lang/") && extendsThread(classfileBuffer)) {
                // Kotlin's thread {}, a library's thread class: it waits for the CPU too when a robot's.
                return patchHooks(classfileBuffer, className, THREAD_SUBCLASS_HOOKS, false);
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
