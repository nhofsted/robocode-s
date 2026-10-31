/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.Opcodes;
import org.objectweb.asm.Type;
import org.objectweb.asm.tree.AbstractInsnNode;
import org.objectweb.asm.tree.ClassNode;
import org.objectweb.asm.tree.FrameNode;
import org.objectweb.asm.tree.InsnList;
import org.objectweb.asm.tree.InsnNode;
import org.objectweb.asm.tree.IntInsnNode;
import org.objectweb.asm.tree.JumpInsnNode;
import org.objectweb.asm.tree.LabelNode;
import org.objectweb.asm.tree.LdcInsnNode;
import org.objectweb.asm.tree.MethodInsnNode;
import org.objectweb.asm.tree.MethodNode;
import org.objectweb.asm.tree.TryCatchBlockNode;
import org.objectweb.asm.tree.TypeInsnNode;
import org.objectweb.asm.tree.VarInsnNode;

import java.io.IOException;
import java.io.InputStream;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.WeakHashMap;

/**
 * Robot code's thread calls go through the robot's deterministic scheduler
 * ({@link RobotThreads}, ADR 0025): {@code Thread.yield} calls a synthetic helper of the
 * robot class that hands it to the scheduler, robot classes that extend {@code Thread} wait for the CPU when their
 * {@code run()} starts, and where robot code checks its work limit (ADR 0018) it first
 * asks the scheduler once the limit is reached ({@code robocodes$limit}). Applied after the
 * counting: the count stays the same.
 */
final class ThreadCalls {

    /** Hook for robot code's thread calls (a {@code Consumer<Object[]>}: the call's name, its arguments). */
    static final String CALL_KEY = "robocodes.determinism.threadCall";
    /** Hook for robot code that reached its work limit (a {@code Consumer<Object>}: the counter). */
    static final String LIMIT_KEY = "robocodes.determinism.atLimit";
    /** Hook for a robot thread's start of {@code run()} (a {@code Runnable}). */
    static final String ENTER_KEY = "robocodes.determinism.threadEnter";
    /** The helper robot classes call where they check their work limit. */
    static final String LIMIT_METHOD = "robocodes$limit";

    private static final String PREFIX = "robocodes$thread$";
    private static final Map<ClassLoader, Map<String, Boolean>> THREAD_CLASSES = new WeakHashMap<>();

    private ThreadCalls() {
    }

    /** The calls robot code makes that the scheduler takes over, by name and descriptor. */
    private record Call(int opcode, String name, String desc, String kind) {
    }

    /**
     * {@code Thread.yield()} is native, so robot code's calls are rewritten. {@code sleep},
     * {@code join} and {@code interrupt} are hooked in {@code java.lang.Thread} itself
     * ({@code DeterminismAgent.patchThread}): a helper here would show in the stack traces
     * of robots without threads.
     */
    private static final List<Call> CALLS = List.of(
            new Call(Opcodes.INVOKESTATIC, "yield", "()V", "yield"));

    /** Rewrites the class's thread calls and adds the helpers. */
    static void patch(ClassNode node, ClassLoader loader, String fuelField) {
        if (usesUncovered(node)) {
            RobotThreads.uncovered(loader);
        }
        boolean thread = isThread(loader, node.superName);
        Map<String, MethodNode> helpers = new LinkedHashMap<>();
        for (MethodNode method : List.copyOf(node.methods)) {
            if (method.instructions.size() == 0) {
                continue;
            }
            for (AbstractInsnNode insn = method.instructions.getFirst(); insn != null; insn = insn.getNext()) {
                if (!(insn instanceof MethodInsnNode call)) {
                    continue;
                }
                Call match = match(call);
                if (match == null || !isThread(loader, call.owner)) {
                    continue;
                }
                MethodNode helper = helpers.computeIfAbsent(match.kind(), k -> helper(match));
                MethodInsnNode replacement = new MethodInsnNode(Opcodes.INVOKESTATIC, node.name, helper.name,
                        helper.desc, false);
                method.instructions.set(call, replacement);
                insn = replacement;
            }
            if (thread && method.name.equals("run") && method.desc.equals("()V")
                    && (method.access & Opcodes.ACC_STATIC) == 0) {
                // A thread of the robot's own class: it waits for the CPU before its code runs.
                method.instructions.insert(enter());
            }
        }
        boolean frames = (node.version & 0xFFFF) >= Opcodes.V1_6;
        for (MethodNode method : List.copyOf(node.methods)) {
            if (method.instructions.size() > 0) {
                syncCalls(method.instructions);
                if ((method.access & Opcodes.ACC_SYNCHRONIZED) != 0) {
                    synchronizedMethod(node, method, frames);
                }
            }
        }
        node.methods.addAll(helpers.values());
        node.methods.add(limitHelper(fuelField, frames));
    }

    private static final String SYNC = "io/github/nhofsted/robocodes/determinism/RobotSync";

    /**
     * Monitors and notifies go through the robot's scheduler ({@link RobotSync}):
     * {@code DUP; enter; monitorenter}, {@code DUP; monitorexit; exit}, {@code DUP; notify;
     * o.notify()}. Stack-neutral and straight-line: the frames stay valid.
     */
    private static void syncCalls(InsnList instructions) {
        for (AbstractInsnNode insn = instructions.getFirst(); insn != null; insn = insn.getNext()) {
            int op = insn.getOpcode();
            if (op == Opcodes.MONITORENTER) {
                InsnList before = new InsnList();
                before.add(new InsnNode(Opcodes.DUP));
                before.add(syncCall("enter"));
                instructions.insertBefore(insn, before);
            } else if (op == Opcodes.MONITOREXIT) {
                instructions.insertBefore(insn, new InsnNode(Opcodes.DUP));
                AbstractInsnNode after = syncCall("exit");
                instructions.insert(insn, after);
                insn = after;
            } else if (insn instanceof MethodInsnNode call && op == Opcodes.INVOKEVIRTUAL && call.desc.equals("()V")
                    && (call.name.equals("notify") || call.name.equals("notifyAll"))) {
                InsnList before = new InsnList();
                before.add(new InsnNode(Opcodes.DUP));
                before.add(syncCall(call.name));
                instructions.insertBefore(insn, before);
            }
        }
    }

    private static MethodInsnNode syncCall(String name) {
        return new MethodInsnNode(Opcodes.INVOKESTATIC, SYNC, name, "(Ljava/lang/Object;)V", false);
    }

    /**
     * A {@code synchronized} method takes its monitor explicitly, so the scheduler sees it:
     * {@code enter; monitorenter} first, {@code monitorexit; exit} before every return and in
     * a handler for everything that rethrows, as javac compiles a {@code synchronized} block.
     * In place, so stack traces stay the same. Left alone where it can't be done safely (an
     * instance method that stores into {@code this}'s slot): its monitor is then the JVM's
     * only.
     */
    private static void synchronizedMethod(ClassNode node, MethodNode method, boolean frames) {
        boolean isStatic = (method.access & Opcodes.ACC_STATIC) != 0;
        if ((method.access & (Opcodes.ACC_ABSTRACT | Opcodes.ACC_NATIVE)) != 0) {
            return;
        }
        if (!isStatic) {
            for (AbstractInsnNode insn = method.instructions.getFirst(); insn != null; insn = insn.getNext()) {
                if (insn instanceof VarInsnNode v && v.var == 0 && v.getOpcode() >= Opcodes.ISTORE
                        && v.getOpcode() <= Opcodes.ASTORE) {
                    return;
                }
            }
        }
        boolean ldcClass = (node.version & 0xFFFF) >= Opcodes.V1_5;
        method.access &= ~Opcodes.ACC_SYNCHRONIZED;
        InsnList code = method.instructions;
        for (AbstractInsnNode insn = code.getFirst(); insn != null; insn = insn.getNext()) {
            int op = insn.getOpcode();
            if (op >= Opcodes.IRETURN && op <= Opcodes.RETURN) {
                InsnList exit = new InsnList();
                exit.add(lock(node, isStatic, ldcClass));
                exit.add(new InsnNode(Opcodes.MONITOREXIT));
                exit.add(lock(node, isStatic, ldcClass));
                exit.add(syncCall("exit"));
                code.insertBefore(insn, exit);
            }
        }
        LabelNode start = new LabelNode();
        LabelNode end = new LabelNode();
        LabelNode handler = new LabelNode();
        InsnList enter = new InsnList();
        enter.add(lock(node, isStatic, ldcClass));
        enter.add(new InsnNode(Opcodes.DUP));
        enter.add(syncCall("enter"));
        enter.add(new InsnNode(Opcodes.MONITORENTER));
        enter.add(start);
        code.insert(enter);
        code.add(end);
        code.add(handler);
        if (frames) {
            Object[] locals = isStatic ? new Object[0] : new Object[] {node.name};
            code.add(new FrameNode(Opcodes.F_NEW, locals.length, locals, 1, new Object[] {"java/lang/Throwable"}));
        }
        code.add(lock(node, isStatic, ldcClass));
        code.add(new InsnNode(Opcodes.MONITOREXIT));
        code.add(lock(node, isStatic, ldcClass));
        code.add(syncCall("exit"));
        code.add(new InsnNode(Opcodes.ATHROW));
        method.tryCatchBlocks.add(new TryCatchBlockNode(start, end, handler, null));
    }

    /** Pushes a synchronized method's monitor: {@code this}, or its class. */
    private static InsnList lock(ClassNode node, boolean isStatic, boolean ldcClass) {
        InsnList code = new InsnList();
        if (!isStatic) {
            code.add(new VarInsnNode(Opcodes.ALOAD, 0));
        } else if (ldcClass) {
            code.add(new LdcInsnNode(Type.getObjectType(node.name)));
        } else {
            code.add(new LdcInsnNode(node.name.replace('/', '.')));
            code.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/Class", "forName",
                    "(Ljava/lang/String;)Ljava/lang/Class;", false));
        }
        return code;
    }

    /**
     * Whether the class uses what the scheduler doesn't cover: virtual threads, and
     * {@code Unsafe}'s parking (which bypasses {@code LockSupport}).
     */
    static boolean usesUncovered(ClassNode node) {
        for (MethodNode method : node.methods) {
            for (AbstractInsnNode insn = method.instructions.getFirst(); insn != null; insn = insn.getNext()) {
                if (insn instanceof MethodInsnNode call && uncovered(call)) {
                    return true;
                }
            }
        }
        return false;
    }

    private static boolean uncovered(MethodInsnNode call) {
        return (call.owner.equals("java/lang/Thread") && (call.name.equals("ofVirtual")
                || call.name.equals("startVirtualThread")))
                || call.owner.startsWith("java/lang/Thread$Builder")
                || (call.owner.equals("java/util/concurrent/Executors")
                        && call.name.equals("newVirtualThreadPerTaskExecutor"))
                || ((call.owner.equals("sun/misc/Unsafe") || call.owner.equals("jdk/internal/misc/Unsafe"))
                        && (call.name.equals("park") || call.name.equals("unpark")));
    }

    private static Call match(MethodInsnNode call) {
        for (Call c : CALLS) {
            if (c.opcode() == call.getOpcode() && c.name().equals(call.name) && c.desc().equals(call.desc)) {
                return c;
            }
        }
        return null;
    }

    /** {@code hook.run()} for {@link #ENTER_KEY}. Straight-line and stack-neutral. */
    private static InsnList enter() {
        InsnList code = new InsnList();
        code.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties",
                "()Ljava/util/Properties;", false));
        code.add(new LdcInsnNode(ENTER_KEY));
        code.add(new MethodInsnNode(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                "(Ljava/lang/Object;)Ljava/lang/Object;", false));
        code.add(new TypeInsnNode(Opcodes.CHECKCAST, "java/lang/Runnable"));
        code.add(new MethodInsnNode(Opcodes.INVOKEINTERFACE, "java/lang/Runnable", "run", "()V", true));
        return code;
    }

    /**
     * {@code hook.accept(new Object[] {kind, receiver?, boxed args...})}: the call's
     * parameters, a receiver as {@code Object} (no class loading to verify it). Straight-line.
     */
    private static MethodNode helper(Call call) {
        Type[] args = Type.getArgumentTypes(call.desc());
        boolean virtual = call.opcode() != Opcodes.INVOKESTATIC;
        Type[] params = new Type[args.length + (virtual ? 1 : 0)];
        if (virtual) {
            params[0] = Type.getType(Object.class);
        }
        System.arraycopy(args, 0, params, virtual ? 1 : 0, args.length);
        MethodNode helper = new MethodNode(Opcodes.ACC_PRIVATE | Opcodes.ACC_STATIC | Opcodes.ACC_SYNTHETIC,
                PREFIX + call.kind(), Type.getMethodDescriptor(Type.VOID_TYPE, params), null,
                new String[] {"java/lang/InterruptedException"});
        InsnList code = helper.instructions;
        code.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties",
                "()Ljava/util/Properties;", false));
        code.add(new LdcInsnNode(CALL_KEY));
        code.add(new MethodInsnNode(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                "(Ljava/lang/Object;)Ljava/lang/Object;", false));
        code.add(new TypeInsnNode(Opcodes.CHECKCAST, "java/util/function/Consumer"));
        code.add(new IntInsnNode(Opcodes.BIPUSH, params.length + 1));
        code.add(new TypeInsnNode(Opcodes.ANEWARRAY, "java/lang/Object"));
        code.add(new InsnNode(Opcodes.DUP));
        code.add(new InsnNode(Opcodes.ICONST_0));
        code.add(new LdcInsnNode(call.kind()));
        code.add(new InsnNode(Opcodes.AASTORE));
        for (int i = 0, slot = 0; i < params.length; i++) {
            code.add(new InsnNode(Opcodes.DUP));
            code.add(new IntInsnNode(Opcodes.BIPUSH, i + 1));
            code.add(new VarInsnNode(params[i].getOpcode(Opcodes.ILOAD), slot));
            switch (params[i].getSort()) {
                case Type.LONG -> code.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/Long", "valueOf",
                        "(J)Ljava/lang/Long;", false));
                case Type.INT -> code.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/Integer", "valueOf",
                        "(I)Ljava/lang/Integer;", false));
                default -> {
                }
            }
            code.add(new InsnNode(Opcodes.AASTORE));
            slot += params[i].getSize();
        }
        code.add(new MethodInsnNode(Opcodes.INVOKEINTERFACE, "java/util/function/Consumer", "accept",
                "(Ljava/lang/Object;)V", true));
        code.add(new InsnNode(Opcodes.RETURN));
        return helper;
    }

    /**
     * {@code static void robocodes$limit(long[] fuel) { if (fuel[1] >= 0) hook.accept(fuel); }}:
     * called where robot code checks its limit, before the check, so the scheduler can hand
     * the CPU on at a quantum's end and raise the limit again. Below the limit it does
     * nothing; at the cutoff the check after it throws, as before.
     */
    private static MethodNode limitHelper(String fuelField, boolean frames) {
        MethodNode helper = new MethodNode(Opcodes.ACC_PRIVATE | Opcodes.ACC_STATIC | Opcodes.ACC_SYNTHETIC,
                LIMIT_METHOD, "([J)V", null, null);
        InsnList code = helper.instructions;
        LabelNode below = new LabelNode();
        code.add(new VarInsnNode(Opcodes.ALOAD, 0));
        code.add(new InsnNode(Opcodes.ICONST_1));
        code.add(new InsnNode(Opcodes.LALOAD));
        code.add(new InsnNode(Opcodes.LCONST_0));
        code.add(new InsnNode(Opcodes.LCMP));
        code.add(new JumpInsnNode(Opcodes.IFLT, below));
        code.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties",
                "()Ljava/util/Properties;", false));
        code.add(new LdcInsnNode(LIMIT_KEY));
        code.add(new MethodInsnNode(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                "(Ljava/lang/Object;)Ljava/lang/Object;", false));
        code.add(new TypeInsnNode(Opcodes.CHECKCAST, "java/util/function/Consumer"));
        code.add(new VarInsnNode(Opcodes.ALOAD, 0));
        code.add(new MethodInsnNode(Opcodes.INVOKEINTERFACE, "java/util/function/Consumer", "accept",
                "(Ljava/lang/Object;)V", true));
        code.add(below);
        if (frames) {
            // Expanded, as the class's other frames are (WorkMeter reads them expanded).
            code.add(new FrameNode(Opcodes.F_NEW, 1, new Object[] {"[J"}, 0, new Object[0]));
        }
        code.add(new InsnNode(Opcodes.RETURN));
        return helper;
    }

    /**
     * Whether the class is {@code java.lang.Thread} or extends it, read from the class files
     * of its loader (no class is loaded). JDK classes other than {@code Thread} count as not.
     */
    static boolean isThread(ClassLoader loader, String name) {
        if (name == null) {
            return false;
        }
        if (name.equals("java/lang/Thread")) {
            return true;
        }
        if (name.startsWith("java/") || name.startsWith("javax/") || name.startsWith("jdk/")
                || name.startsWith("sun/") || name.startsWith("kotlin/") || name.startsWith("robocode/")
                || name.startsWith("[")) {
            return false;
        }
        Map<String, Boolean> known;
        synchronized (THREAD_CLASSES) {
            known = THREAD_CLASSES.computeIfAbsent(loader, l -> new HashMap<>());
        }
        synchronized (known) {
            Boolean cached = known.get(name);
            if (cached != null) {
                return cached;
            }
        }
        String superName = null;
        try (InputStream in = loader == null ? ClassLoader.getSystemResourceAsStream(name + ".class")
                : loader.getResourceAsStream(name + ".class")) {
            if (in != null) {
                superName = new ClassReader(in.readAllBytes()).getSuperName();
            }
        } catch (IOException | RuntimeException e) {
            superName = null;
        }
        boolean result = superName != null && isThread(loader, superName);
        synchronized (known) {
            known.put(name, result);
        }
        return result;
    }
}
