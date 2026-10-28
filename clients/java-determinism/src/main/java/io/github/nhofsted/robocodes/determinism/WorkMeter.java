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
import org.objectweb.asm.tree.FrameNode;
import org.objectweb.asm.tree.InsnList;
import org.objectweb.asm.tree.InsnNode;
import org.objectweb.asm.tree.IntInsnNode;
import org.objectweb.asm.tree.JumpInsnNode;
import org.objectweb.asm.tree.LabelNode;
import org.objectweb.asm.tree.LdcInsnNode;
import org.objectweb.asm.tree.LookupSwitchInsnNode;
import org.objectweb.asm.tree.MethodInsnNode;
import org.objectweb.asm.tree.MethodNode;
import org.objectweb.asm.tree.TableSwitchInsnNode;
import org.objectweb.asm.tree.TryCatchBlockNode;
import org.objectweb.asm.tree.TypeInsnNode;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * Deterministic work metering and a virtual clock for robots (#15, ADR 0014).
 *
 * <p>In robot classes the agent adds, at the start of every straight-line block of
 * bytecode, {@code fuel[1] += n}: the block's instructions, plus a fixed cost per call
 * into the JDK. {@code fuel} is one counter per robot (per robot class loader), kept as
 * {@code {limit, count - limit}}: at a loop's start, the block throws once the count
 * reaches the limit (an endless loop crashes at a deterministic point, ADR 0018). Calls to
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

    /**
     * Adds work counting and the virtual clock to a robot class, and routes its thread calls
     * through the robot's scheduler (ADR 0025). Null: leave unchanged.
     */
    static byte[] patchRobotClass(byte[] classfileBuffer, ClassLoader loader) {
        ClassReader reader = new ClassReader(classfileBuffer);
        if ((reader.getAccess() & (Opcodes.ACC_INTERFACE | Opcodes.ACC_MODULE)) != 0) {
            return null; // interfaces: default methods stay uncounted (rare in robots)
        }
        ClassNode node = new ClassNode(Opcodes.ASM9);
        // Expanded frames: a check's branch reuses the frame of the block it starts (countAndCheck).
        reader.accept(node, ClassReader.EXPAND_FRAMES);
        boolean meter = RobotRandomness.METER_WORK;
        if (meter) {
            node.fields.add(new FieldNode(Opcodes.ACC_PRIVATE | Opcodes.ACC_STATIC | Opcodes.ACC_TRANSIENT
                    | Opcodes.ACC_SYNTHETIC, FIELD, "[J", null, null));
        }
        MethodNode clinit = null;
        for (MethodNode method : node.methods) {
            if (method.name.equals("<clinit>")) {
                clinit = method;
            }
            if (method.instructions.size() > 0) {
                // Counting first: a clock call costs one JDK call, not the hook's code.
                if (meter) {
                    count(node.name, method, (node.version & 0xFFFF) >= Opcodes.V1_6);
                    replaceClockCalls(node.name, method.instructions);
                }
                IdentityHashes.replaceHashCalls(method.instructions);
                dropUnplacedLocals(method);
                if (RobotRandomness.tracing()) {
                    traceApiCalls(method.instructions);
                }
            }
        }
        if (!meter) {
            // Calibration only (-Drobocodes.meterWork=false): deterministic hashes, no counting.
            ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_MAXS);
            node.accept(writer);
            return writer.toByteArray();
        }
        // After the counting: the helpers it adds aren't counted themselves.
        JdkCosts.charge(node, FIELD);
        ThreadCalls.patch(node, loader, FIELD);
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
    /** Robot API methods traced whatever class robot code calls them on (its own robot class). */
    private static final Set<String> TRACED_ROBOT_METHODS = Set.of("getTime", "getGunHeat", "getEnergy",
            "setFireBullet", "setFire", "fireBullet", "fire");

    private static void traceApiCalls(InsnList instructions) {
        for (AbstractInsnNode insn = instructions.getFirst(); insn != null; insn = insn.getNext()) {
            if (!(insn instanceof MethodInsnNode call) || !(call.owner.startsWith("robocode/")
                    || TRACED_ROBOT_METHODS.contains(call.name))) {
                continue;
            }
            char ret = call.desc.charAt(call.desc.indexOf(')') + 1);
            boolean object = ret == 'L' || ret == '[';
            if (object) {
                ret = 'Z';
            }
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
            if (object) {
                // An object result: traced as whether it is null.
                trace.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/util/Objects", "isNull",
                        "(Ljava/lang/Object;)Z", false));
            }
            trace.add(new MethodInsnNode(Opcodes.INVOKESTATIC, boxed, "valueOf", box, false));
            trace.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties",
                    "()Ljava/util/Properties;", false));
            trace.add(new LdcInsnNode(RobotRandomness.API_TRACE_KEY));
            trace.add(new MethodInsnNode(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                    "(Ljava/lang/Object;)Ljava/lang/Object;", false));
            trace.add(new TypeInsnNode(Opcodes.CHECKCAST, "java/util/function/BiConsumer"));
            trace.add(new InsnNode(Opcodes.SWAP));
            trace.add(new LdcInsnNode(call.owner.substring(call.owner.lastIndexOf('/') + 1) + "." + call.name
                    + (object ? " isNull" : "")));
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

    /**
     * Robot code's ways to ask for the time read the virtual clock: {@code System.nanoTime()}
     * and {@code currentTimeMillis()}, and the JDK calls that read the latter themselves:
     * {@code new Date()}, {@code Calendar.getInstance()} and {@code Instant.now()}.
     */
    private static void replaceClockCalls(String className, InsnList instructions) {
        for (AbstractInsnNode insn = instructions.getFirst(); insn != null; insn = insn.getNext()) {
            if (!(insn instanceof MethodInsnNode call)) {
                continue;
            }
            String method = call.owner + "." + call.name + call.desc;
            InsnList replacement = new InsnList();
            switch (method) {
                case "java/lang/System.nanoTime()J" ->
                        replacement.add(virtualClock(className, RobotRandomness.CLOCK_NANOS_KEY));
                case "java/lang/System.currentTimeMillis()J" ->
                        replacement.add(virtualClock(className, RobotRandomness.CLOCK_MILLIS_KEY));
                case "java/util/Date.<init>()V" -> {
                    // new Date() becomes new Date(millis).
                    replacement.add(virtualClock(className, RobotRandomness.CLOCK_MILLIS_KEY));
                    replacement.add(new MethodInsnNode(Opcodes.INVOKESPECIAL, "java/util/Date", "<init>", "(J)V",
                            false));
                }
                case "java/util/Calendar.getInstance()Ljava/util/Calendar;" -> {
                    replacement.add(new MethodInsnNode(Opcodes.INVOKESTATIC, call.owner, call.name, call.desc,
                            false));
                    replacement.add(new InsnNode(Opcodes.DUP));
                    replacement.add(virtualClock(className, RobotRandomness.CLOCK_MILLIS_KEY));
                    replacement.add(new MethodInsnNode(Opcodes.INVOKEVIRTUAL, "java/util/Calendar",
                            "setTimeInMillis", "(J)V", false));
                }
                case "java/time/Instant.now()Ljava/time/Instant;" -> {
                    replacement.add(virtualClock(className, RobotRandomness.CLOCK_MILLIS_KEY));
                    replacement.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/time/Instant", "ofEpochMilli",
                            "(J)Ljava/time/Instant;", false));
                }
                default -> {
                    continue;
                }
            }
            AbstractInsnNode last = replacement.getLast();
            instructions.insert(insn, replacement);
            instructions.remove(insn);
            insn = last;
        }
    }

    /** Pushes the hook's virtual time ({@code key}: nanos or millis) for this robot. */
    private static InsnList virtualClock(String className, String key) {
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
        return clock;
    }

    /**
     * Splits the code into straight-line blocks (a block ends at a label, which any jump
     * target or handler is, and after a jump, switch, return or throw) and adds
     * {@code fuel[1] += cost} at the start of each block with instructions.
     *
     * <p>Where code can repeat, the block also checks the limit ({@link #countAndCheck}).
     * Straight-line, so the frames stay valid. Code that runs on and on
     * passes a loop's start (a backward jump's target) again and again, so it throws a little
     * after the limit, at the same instruction everywhere; endless recursion overflows the
     * stack instead. The other blocks only count: a check in every block doubled the CPU time
     * of tight loops, one at every method's start still cost a fifth. Handlers check too, so
     * robot code can't catch it for long.
     *
     * <p>Except a handler that its own range covers: javac's {@code finally} and
     * {@code synchronized} handlers retry their first instructions. A check there would
     * throw into itself forever.
     */
    private static void count(String className, MethodNode method, boolean frames) {
        InsnList instructions = method.instructions;
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
        Set<AbstractInsnNode> checked = repeatable(method);
        checked.removeAll(selfCoveredHandlers(method));
        for (int i = 0; i < starts.size(); i++) {
            InsnList add = checked.contains(starts.get(i))
                    ? countAndCheck(className, costs.get(i), starts.get(i), frames)
                    : count(className, costs.get(i));
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

    /** {@code fuel[1] += cost}: the count, relative to the limit. */
    private static InsnList count(String className, long cost) {
        InsnList add = new InsnList();
        add.add(new FieldInsnNode(Opcodes.GETSTATIC, className, FIELD, "[J"));
        add.add(new InsnNode(Opcodes.ICONST_1));
        add.add(new InsnNode(Opcodes.DUP2));
        add.add(new InsnNode(Opcodes.LALOAD));
        add.add(new LdcInsnNode(cost));
        add.add(new InsnNode(Opcodes.LADD));
        add.add(new InsnNode(Opcodes.LASTORE));
        return add;
    }

    /**
     * The same, then {@code robocodes$limit(fuel)}, which asks the robot's scheduler once the
     * count reaches the limit (a quantum's end for a robot with threads, ADR 0025; at the
     * cutoff it changes nothing), then {@code fuel[1 + (int) (~fuel[1] >>> 63)]}: index 2,
     * out of bounds, once the count still reaches the limit. A load: storing at a computed
     * index instead made tight loops slower still.
     */
    private static InsnList countAndCheck(String className, long cost, AbstractInsnNode first, boolean frames) {
        InsnList add = count(className, cost);
        FrameNode frame = frames ? frameBefore(first) : null;
        if (first.getOpcode() == Opcodes.NEW || (frames && frame == null)) {
            // No frame to branch to (or an uninitialized object on the stack): call the helper.
            add.add(new FieldInsnNode(Opcodes.GETSTATIC, className, FIELD, "[J"));
            add.add(new MethodInsnNode(Opcodes.INVOKESTATIC, className, ThreadCalls.LIMIT_METHOD, "([J)V", false));
        } else {
            // if (fuel[1] >= 0) robocodes$limit(fuel): inline, so the common case is a load and
            // a compare; a call there cost BeepBoop half again its CPU time.
            LabelNode below = new LabelNode();
            add.add(new FieldInsnNode(Opcodes.GETSTATIC, className, FIELD, "[J"));
            add.add(new InsnNode(Opcodes.ICONST_1));
            add.add(new InsnNode(Opcodes.LALOAD));
            add.add(new InsnNode(Opcodes.LCONST_0));
            add.add(new InsnNode(Opcodes.LCMP));
            add.add(new JumpInsnNode(Opcodes.IFLT, below));
            add.add(new FieldInsnNode(Opcodes.GETSTATIC, className, FIELD, "[J"));
            add.add(new MethodInsnNode(Opcodes.INVOKESTATIC, className, ThreadCalls.LIMIT_METHOD, "([J)V", false));
            add.add(below);
            if (frame != null) {
                // The block's own frame: the code before is stack- and locals-neutral.
                add.add(new FrameNode(Opcodes.F_NEW, frame.local.size(), frame.local.toArray(),
                        frame.stack.size(), frame.stack.toArray()));
            }
        }
        add.add(new FieldInsnNode(Opcodes.GETSTATIC, className, FIELD, "[J"));
        add.add(new FieldInsnNode(Opcodes.GETSTATIC, className, FIELD, "[J"));
        add.add(new InsnNode(Opcodes.ICONST_1));
        add.add(new InsnNode(Opcodes.LALOAD));
        add.add(new LdcInsnNode(-1L));
        add.add(new InsnNode(Opcodes.LXOR));
        add.add(new IntInsnNode(Opcodes.BIPUSH, 63));
        add.add(new InsnNode(Opcodes.LUSHR));
        add.add(new InsnNode(Opcodes.L2I));
        add.add(new InsnNode(Opcodes.ICONST_1));
        add.add(new InsnNode(Opcodes.IADD));
        add.add(new InsnNode(Opcodes.LALOAD));
        add.add(new InsnNode(Opcodes.POP2));
        return add;
    }

    /** The (expanded) frame right before a block's first instruction, if any. */
    private static FrameNode frameBefore(AbstractInsnNode first) {
        for (AbstractInsnNode insn = first.getPrevious(); insn != null && insn.getOpcode() < 0;
                insn = insn.getPrevious()) {
            if (insn instanceof FrameNode frame) {
                return frame.type == Opcodes.F_NEW ? frame : null;
            }
        }
        return null;
    }

    /** The first instructions of loops (backward jumps' targets) and handlers. */
    private static Set<AbstractInsnNode> repeatable(MethodNode method) {
        Set<AbstractInsnNode> starts = new HashSet<>();
        InsnList instructions = method.instructions;
        for (AbstractInsnNode insn = instructions.getFirst(); insn != null; insn = insn.getNext()) {
            List<LabelNode> targets = new ArrayList<>();
            if (insn instanceof JumpInsnNode jump) {
                targets.add(jump.label);
            } else if (insn instanceof TableSwitchInsnNode table) {
                targets.add(table.dflt);
                targets.addAll(table.labels);
            } else if (insn instanceof LookupSwitchInsnNode lookup) {
                targets.add(lookup.dflt);
                targets.addAll(lookup.labels);
            }
            int at = instructions.indexOf(insn);
            for (LabelNode target : targets) {
                if (instructions.indexOf(target) <= at) {
                    starts.add(firstInstruction(target));
                }
            }
        }
        for (TryCatchBlockNode block : method.tryCatchBlocks) {
            starts.add(firstInstruction(block.handler));
        }
        starts.remove(null);
        return starts;
    }

    /** The first real instruction at or after {@code insn} (skipping labels, lines and frames). */
    private static AbstractInsnNode firstInstruction(AbstractInsnNode insn) {
        while (insn != null && insn.getOpcode() < 0) {
            insn = insn.getNext();
        }
        return insn;
    }

    /** The first instructions of handlers whose own range covers them. */
    private static Set<AbstractInsnNode> selfCoveredHandlers(MethodNode method) {
        Set<AbstractInsnNode> covered = new HashSet<>();
        InsnList instructions = method.instructions;
        for (TryCatchBlockNode block : method.tryCatchBlocks) {
            AbstractInsnNode first = firstInstruction(block.handler);
            if (first == null) {
                continue;
            }
            int at = instructions.indexOf(first);
            if (instructions.indexOf(block.start) <= at && at < instructions.indexOf(block.end)) {
                covered.add(first);
            }
        }
        return covered;
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
