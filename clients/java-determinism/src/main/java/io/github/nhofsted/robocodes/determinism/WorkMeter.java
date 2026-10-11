/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.Opcodes;
import org.objectweb.asm.tree.AbstractInsnNode;
import org.objectweb.asm.tree.ClassNode;
import org.objectweb.asm.tree.FieldInsnNode;
import org.objectweb.asm.tree.FieldNode;
import org.objectweb.asm.tree.InsnList;
import org.objectweb.asm.tree.InsnNode;
import org.objectweb.asm.tree.LabelNode;
import org.objectweb.asm.tree.LdcInsnNode;
import org.objectweb.asm.tree.MethodInsnNode;
import org.objectweb.asm.tree.MethodNode;
import org.objectweb.asm.tree.TypeInsnNode;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * Deterministic work metering and a virtual clock for robots (#15, ADR 0014).
 *
 * <p>In robot classes the agent adds, at the start of every straight-line block of
 * bytecode, {@code fuel[0] += n}: the block's instructions, plus a fixed cost per call
 * into the JDK. {@code fuel} is one counter per robot (per robot class loader). Calls to
 * {@code System.nanoTime()} and {@code System.currentTimeMillis()} in robot code read a
 * virtual clock instead, computed from that counter and the robot's turns
 * ({@link RobotRandomness#virtualNanos}). The count is exact for the robot's own code and
 * an estimate for work inside the JDK; it only depends on what the robot executes.
 */
final class WorkMeter {

    /** Counted per call into the JDK, for the work done there. */
    static final int JDK_CALL_COST = 10;

    private static final String FIELD = "robocodes$fuel";
    private static final String HOOK_FUNCTION = "java/util/function/Function";
    private static final String HOOK_CLOCK = "java/util/function/ToLongFunction";

    private WorkMeter() {
    }

    /** Adds work counting and the virtual clock to a robot class. Null: leave unchanged. */
    static byte[] patchRobotClass(byte[] classfileBuffer) {
        ClassReader reader = new ClassReader(classfileBuffer);
        if ((reader.getAccess() & (Opcodes.ACC_INTERFACE | Opcodes.ACC_MODULE)) != 0) {
            return null; // interfaces: default methods stay uncounted (rare in robots)
        }
        ClassNode node = new ClassNode(Opcodes.ASM9);
        reader.accept(node, 0);
        node.fields.add(new FieldNode(Opcodes.ACC_PRIVATE | Opcodes.ACC_STATIC | Opcodes.ACC_TRANSIENT
                | Opcodes.ACC_SYNTHETIC, FIELD, "[J", null, null));
        MethodNode clinit = null;
        for (MethodNode method : node.methods) {
            if (method.name.equals("<clinit>")) {
                clinit = method;
            }
            if (method.instructions.size() > 0) {
                // Counting first: a clock call costs one JDK call, not the hook's code.
                count(node.name, method.instructions);
                replaceClockCalls(node.name, method.instructions);
                IdentityHashes.replaceHashCalls(method.instructions);
                dropUnplacedLocals(method);
                if (RobotRandomness.tracing()) {
                    traceApiCalls(method.instructions);
                }
            }
        }
        if (clinit == null) {
            clinit = new MethodNode(Opcodes.ACC_STATIC, "<clinit>", "()V", null, null);
            clinit.instructions.add(new InsnNode(Opcodes.RETURN));
            node.methods.add(clinit);
        }
        // Before anything else, also before the counting code at the start of <clinit>.
        clinit.instructions.insert(initCounter(node.name));
        // Inserted code is straight-line and stack-neutral: existing frames stay valid.
        ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_MAXS);
        node.accept(writer);
        return writer.toByteArray();
    }

    /**
     * Diagnostics only: after each call into the Robocode API that returns a primitive,
     * {@code hook.accept(boxed result, "Owner.method")}. Not counted as work.
     */
    private static void traceApiCalls(InsnList instructions) {
        for (AbstractInsnNode insn = instructions.getFirst(); insn != null; insn = insn.getNext()) {
            if (!(insn instanceof MethodInsnNode call) || !call.owner.startsWith("robocode/")) {
                continue;
            }
            char ret = call.desc.charAt(call.desc.indexOf(')') + 1);
            String box = switch (ret) {
                case 'D' -> "(D)Ljava/lang/Double;";
                case 'J' -> "(J)Ljava/lang/Long;";
                case 'I' -> "(I)Ljava/lang/Integer;";
                case 'Z' -> "(Z)Ljava/lang/Boolean;";
                default -> null;
            };
            if (box == null) {
                continue;
            }
            String boxed = switch (ret) {
                case 'D' -> "java/lang/Double";
                case 'J' -> "java/lang/Long";
                case 'I' -> "java/lang/Integer";
                default -> "java/lang/Boolean";
            };
            InsnList trace = new InsnList();
            trace.add(new InsnNode(ret == 'D' || ret == 'J' ? Opcodes.DUP2 : Opcodes.DUP));
            trace.add(new MethodInsnNode(Opcodes.INVOKESTATIC, boxed, "valueOf", box, false));
            trace.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties",
                    "()Ljava/util/Properties;", false));
            trace.add(new LdcInsnNode(RobotRandomness.API_TRACE_KEY));
            trace.add(new MethodInsnNode(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                    "(Ljava/lang/Object;)Ljava/lang/Object;", false));
            trace.add(new TypeInsnNode(Opcodes.CHECKCAST, "java/util/function/BiConsumer"));
            trace.add(new InsnNode(Opcodes.SWAP));
            trace.add(new LdcInsnNode(call.owner.substring(call.owner.lastIndexOf('/') + 1) + "." + call.name));
            trace.add(new MethodInsnNode(Opcodes.INVOKEINTERFACE, "java/util/function/BiConsumer", "accept",
                    "(Ljava/lang/Object;Ljava/lang/Object;)V", true));
            AbstractInsnNode last = trace.getLast();
            instructions.insert(insn, trace);
            insn = last;
        }
    }

    /**
     * Drops local variable debug entries whose range doesn't start and end at an
     * instruction. Old compilers (javac 1.1) wrote ends in the middle of an instruction; the
     * JVM accepts that, but ASM can't place the label and would write a negative length,
     * which the JVM rejects ("Invalid length in LocalVariableTable"). Debug information only.
     */
    private static void dropUnplacedLocals(MethodNode method) {
        if (method.localVariables == null || method.localVariables.isEmpty()) {
            return;
        }
        Set<LabelNode> placed = new HashSet<>();
        for (AbstractInsnNode insn = method.instructions.getFirst(); insn != null; insn = insn.getNext()) {
            if (insn instanceof LabelNode label) {
                placed.add(label);
            }
        }
        method.localVariables.removeIf(local -> !placed.contains(local.start) || !placed.contains(local.end));
    }

    /** {@code fuel = hook.apply(Class.forName("C").getClassLoader())}. */
    private static InsnList initCounter(String className) {
        InsnList code = new InsnList();
        code.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties",
                "()Ljava/util/Properties;", false));
        code.add(new LdcInsnNode(RobotRandomness.FUEL_KEY));
        code.add(new MethodInsnNode(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                "(Ljava/lang/Object;)Ljava/lang/Object;", false));
        code.add(new TypeInsnNode(Opcodes.CHECKCAST, HOOK_FUNCTION));
        // Class.forName(String) uses the caller's class loader; works in every class file version.
        code.add(new LdcInsnNode(className.replace('/', '.')));
        code.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/Class", "forName",
                "(Ljava/lang/String;)Ljava/lang/Class;", false));
        code.add(new MethodInsnNode(Opcodes.INVOKEVIRTUAL, "java/lang/Class", "getClassLoader",
                "()Ljava/lang/ClassLoader;", false));
        code.add(new MethodInsnNode(Opcodes.INVOKEINTERFACE, HOOK_FUNCTION, "apply",
                "(Ljava/lang/Object;)Ljava/lang/Object;", true));
        code.add(new TypeInsnNode(Opcodes.CHECKCAST, "[J"));
        code.add(new FieldInsnNode(Opcodes.PUTSTATIC, className, FIELD, "[J"));
        return code;
    }

    /** {@code System.nanoTime()} and {@code currentTimeMillis()} read the virtual clock. */
    private static void replaceClockCalls(String className, InsnList instructions) {
        for (AbstractInsnNode insn = instructions.getFirst(); insn != null; insn = insn.getNext()) {
            if (insn.getOpcode() != Opcodes.INVOKESTATIC) {
                continue;
            }
            MethodInsnNode call = (MethodInsnNode) insn;
            if (!call.owner.equals("java/lang/System") || !call.desc.equals("()J")) {
                continue;
            }
            String key = switch (call.name) {
                case "nanoTime" -> RobotRandomness.CLOCK_NANOS_KEY;
                case "currentTimeMillis" -> RobotRandomness.CLOCK_MILLIS_KEY;
                default -> null;
            };
            if (key == null) {
                continue;
            }
            InsnList clock = new InsnList();
            clock.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties",
                    "()Ljava/util/Properties;", false));
            clock.add(new LdcInsnNode(key));
            clock.add(new MethodInsnNode(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                    "(Ljava/lang/Object;)Ljava/lang/Object;", false));
            clock.add(new TypeInsnNode(Opcodes.CHECKCAST, HOOK_CLOCK));
            clock.add(new FieldInsnNode(Opcodes.GETSTATIC, className, FIELD, "[J"));
            clock.add(new MethodInsnNode(Opcodes.INVOKEINTERFACE, HOOK_CLOCK, "applyAsLong",
                    "(Ljava/lang/Object;)J", true));
            AbstractInsnNode last = clock.getLast();
            instructions.insert(insn, clock);
            instructions.remove(insn);
            insn = last;
        }
    }

    /**
     * Splits the code into straight-line blocks (a block ends at a label, which any jump
     * target or handler is, and after a jump, switch, return or throw) and adds
     * {@code fuel[0] += cost} at the start of each block with instructions.
     */
    private static void count(String className, InsnList instructions) {
        List<AbstractInsnNode> starts = new ArrayList<>();
        List<Long> costs = new ArrayList<>();
        AbstractInsnNode start = null;
        long cost = 0;
        for (AbstractInsnNode insn = instructions.getFirst(); insn != null; insn = insn.getNext()) {
            int type = insn.getType();
            if (type == AbstractInsnNode.LABEL) {
                if (start != null) {
                    starts.add(start);
                    costs.add(cost);
                }
                start = null;
                cost = 0;
                continue;
            }
            if (insn.getOpcode() < 0) {
                continue; // line numbers and frames
            }
            if (start == null) {
                start = insn;
            }
            cost += 1;
            if (insn instanceof MethodInsnNode call && isJdk(call.owner)) {
                cost += JDK_CALL_COST;
            }
            if (endsBlock(insn)) {
                starts.add(start);
                costs.add(cost);
                start = null;
                cost = 0;
            }
        }
        if (start != null) {
            starts.add(start);
            costs.add(cost);
        }
        for (int i = 0; i < starts.size(); i++) {
            InsnList add = new InsnList();
            add.add(new FieldInsnNode(Opcodes.GETSTATIC, className, FIELD, "[J"));
            add.add(new InsnNode(Opcodes.ICONST_0));
            add.add(new InsnNode(Opcodes.DUP2));
            add.add(new InsnNode(Opcodes.LALOAD));
            add.add(new LdcInsnNode(costs.get(i)));
            add.add(new InsnNode(Opcodes.LADD));
            add.add(new InsnNode(Opcodes.LASTORE));
            AbstractInsnNode first = starts.get(i);
            if (first.getOpcode() == Opcodes.NEW) {
                // Stack map frames refer to a NEW by the label right before it
                // ("uninitialized"); code in between would break that. Stack-neutral, so
                // right after the NEW is just as good.
                instructions.insert(first, add);
            } else {
                instructions.insertBefore(first, add);
            }
        }
    }

    private static boolean endsBlock(AbstractInsnNode insn) {
        int type = insn.getType();
        if (type == AbstractInsnNode.JUMP_INSN || type == AbstractInsnNode.TABLESWITCH_INSN
                || type == AbstractInsnNode.LOOKUPSWITCH_INSN) {
            return true;
        }
        int op = insn.getOpcode();
        return (op >= Opcodes.IRETURN && op <= Opcodes.RETURN) || op == Opcodes.ATHROW;
    }

    private static boolean isJdk(String owner) {
        return owner.startsWith("java/") || owner.startsWith("javax/") || owner.startsWith("kotlin/");
    }
}
