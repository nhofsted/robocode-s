/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import org.objectweb.asm.Opcodes;
import org.objectweb.asm.tree.AbstractInsnNode;
import org.objectweb.asm.tree.FieldInsnNode;
import org.objectweb.asm.tree.InsnList;
import org.objectweb.asm.tree.InsnNode;
import org.objectweb.asm.tree.LdcInsnNode;
import org.objectweb.asm.tree.MethodInsnNode;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Calibration only (#15): what a robot's counted work consists of. With
 * {@code -Drobocodes.calibrate} and the work metered, each block also adds how many
 * instructions of each {@link Group} it has, and its JDK calls by method, to slots of the
 * robot's counter after the count itself. The calibration then fits a price per group and
 * per JDK method against the robots' real CPU time, offline, from one run. The extra code is
 * the agent's, not the robot's: it is never counted, and it only runs in the metered run,
 * whose CPU time the calibration doesn't use.
 */
final class WorkProfile {

    /** What kind of work an instruction is. */
    enum Group {
        CONST, LOAD, STORE, STACK, INT, INT_MUL_DIV, FLOAT, FLOAT_DIV, CONVERT, COMPARE, BRANCH, SWITCH,
        ARRAY_LOAD, ARRAY_STORE, ARRAY_LENGTH, FIELD_GET, FIELD_PUT, STATIC_GET, STATIC_PUT, INVOKE,
        INVOKE_INTERFACE, ROBOT_API, JDK_CALL, RETURN, NEW, NEW_ARRAY, TYPE_CHECK, MONITOR, THROW, OTHER
    }

    /** Whether blocks add their profile (calibration with metering). */
    static final boolean ON = RobotRandomness.CALIBRATING && RobotRandomness.METER_WORK;

    /** Counter slots before the profile: the count and its limit. */
    private static final int BASE = 2;
    /** Slot of the blocks' static count, the per-instruction and per-call cost without sized JDK calls. */
    private static final int STATIC = BASE + Group.values().length;
    private static final int JDK_BASE = STATIC + 1;
    /** JDK methods with a slot of their own; later ones share the last. */
    private static final int JDK_METHODS = 4096;
    /** Counter length with the profile. */
    static final int SLOTS = JDK_BASE + JDK_METHODS;

    /**
     * Named slots by key, in the order first seen (the same for every robot of this JVM):
     * {@code jdk:<method>} calls, {@code sized:<method>} units of sized JDK calls (elements, or
     * n·log2 n for sorts), {@code alloc:primitive} and {@code alloc:reference} array elements.
     */
    private static final Map<String, Integer> NAMED_SLOTS = new ConcurrentHashMap<>();
    private static final List<String> NAMES = new ArrayList<>();

    private WorkProfile() {
    }

    static Group group(AbstractInsnNode insn, boolean jdk) {
        int op = insn.getOpcode();
        if (op <= Opcodes.LDC) {
            return op == Opcodes.NOP ? Group.OTHER : Group.CONST;
        }
        if (op <= Opcodes.ALOAD) {
            return Group.LOAD;
        }
        if (op <= Opcodes.SALOAD) {
            return Group.ARRAY_LOAD;
        }
        if (op <= Opcodes.ASTORE) {
            return Group.STORE;
        }
        if (op <= Opcodes.SASTORE) {
            return Group.ARRAY_STORE;
        }
        if (op <= Opcodes.SWAP) {
            return Group.STACK;
        }
        return switch (op) {
            case Opcodes.IADD, Opcodes.LADD, Opcodes.ISUB, Opcodes.LSUB, Opcodes.INEG, Opcodes.LNEG, Opcodes.ISHL,
                    Opcodes.LSHL, Opcodes.ISHR, Opcodes.LSHR, Opcodes.IUSHR, Opcodes.LUSHR, Opcodes.IAND, Opcodes.LAND,
                    Opcodes.IOR, Opcodes.LOR, Opcodes.IXOR, Opcodes.LXOR, Opcodes.IINC -> Group.INT;
            case Opcodes.IMUL, Opcodes.LMUL, Opcodes.IDIV, Opcodes.LDIV, Opcodes.IREM, Opcodes.LREM -> Group.INT_MUL_DIV;
            case Opcodes.FADD, Opcodes.DADD, Opcodes.FSUB, Opcodes.DSUB, Opcodes.FMUL, Opcodes.DMUL, Opcodes.FNEG,
                    Opcodes.DNEG -> Group.FLOAT;
            case Opcodes.FDIV, Opcodes.DDIV, Opcodes.FREM, Opcodes.DREM -> Group.FLOAT_DIV;
            case Opcodes.I2L, Opcodes.I2F, Opcodes.I2D, Opcodes.L2I, Opcodes.L2F, Opcodes.L2D, Opcodes.F2I, Opcodes.F2L,
                    Opcodes.F2D, Opcodes.D2I, Opcodes.D2L, Opcodes.D2F, Opcodes.I2B, Opcodes.I2C, Opcodes.I2S ->
                    Group.CONVERT;
            case Opcodes.LCMP, Opcodes.FCMPL, Opcodes.FCMPG, Opcodes.DCMPL, Opcodes.DCMPG -> Group.COMPARE;
            case Opcodes.TABLESWITCH, Opcodes.LOOKUPSWITCH -> Group.SWITCH;
            case Opcodes.IRETURN, Opcodes.LRETURN, Opcodes.FRETURN, Opcodes.DRETURN, Opcodes.ARETURN, Opcodes.RETURN ->
                    Group.RETURN;
            case Opcodes.GETFIELD -> Group.FIELD_GET;
            case Opcodes.PUTFIELD -> Group.FIELD_PUT;
            case Opcodes.GETSTATIC -> Group.STATIC_GET;
            case Opcodes.PUTSTATIC -> Group.STATIC_PUT;
            case Opcodes.INVOKEVIRTUAL, Opcodes.INVOKESPECIAL, Opcodes.INVOKESTATIC, Opcodes.INVOKEINTERFACE,
                    Opcodes.INVOKEDYNAMIC -> {
                if (jdk) {
                    yield Group.JDK_CALL;
                }
                if (insn instanceof MethodInsnNode call && call.owner.startsWith("robocode/")) {
                    yield Group.ROBOT_API;
                }
                yield op == Opcodes.INVOKEINTERFACE ? Group.INVOKE_INTERFACE : Group.INVOKE;
            }
            case Opcodes.NEW -> Group.NEW;
            case Opcodes.NEWARRAY, Opcodes.ANEWARRAY, Opcodes.MULTIANEWARRAY -> Group.NEW_ARRAY;
            case Opcodes.ARRAYLENGTH -> Group.ARRAY_LENGTH;
            case Opcodes.CHECKCAST, Opcodes.INSTANCEOF -> Group.TYPE_CHECK;
            case Opcodes.MONITORENTER, Opcodes.MONITOREXIT -> Group.MONITOR;
            case Opcodes.ATHROW -> Group.THROW;
            // ASM reads goto_w and jsr_w as GOTO and JSR.
            default -> op >= Opcodes.IFEQ && op <= Opcodes.RET || op == Opcodes.IFNULL || op == Opcodes.IFNONNULL
                    ? Group.BRANCH : Group.OTHER;
        };
    }

    /** The slot of a JDK method's calls ({@code owner.name desc}). */
    static int jdkSlot(MethodInsnNode call) {
        return slot("jdk:" + call.owner + "." + call.name + call.desc);
    }

    /** The slot of a key ({@link #NAMED_SLOTS}). */
    static int slot(String key) {
        return JDK_BASE + NAMED_SLOTS.computeIfAbsent(key, n -> {
            synchronized (NAMES) {
                int slot = Math.min(NAMES.size(), JDK_METHODS - 1);
                if (NAMES.size() < JDK_METHODS) {
                    NAMES.add(n);
                }
                return slot;
            }
        });
    }

    /** One block's profile, as it is built instruction by instruction. */
    static final class Block {
        private final long[] groups = new long[Group.values().length];
        private final Map<Integer, Long> jdk = new java.util.TreeMap<>();
        private long cost;

        void add(AbstractInsnNode insn, boolean jdkCall, long insnCost) {
            groups[group(insn, jdkCall).ordinal()]++;
            if (jdkCall && insn instanceof MethodInsnNode call) {
                jdk.merge(jdkSlot(call), 1L, Long::sum);
            }
            cost += insnCost;
        }

        /** {@code fuel[slot] += n} for each of the block's slots. */
        InsnList code(String className, String field) {
            InsnList add = new InsnList();
            for (int g = 0; g < groups.length; g++) {
                if (groups[g] != 0) {
                    increment(add, className, field, BASE + g, groups[g]);
                }
            }
            increment(add, className, field, STATIC, cost);
            jdk.forEach((slot, n) -> increment(add, className, field, slot, n));
            return add;
        }
    }

    private static void increment(InsnList add, String className, String field, int slot, long n) {
        add.add(new FieldInsnNode(Opcodes.GETSTATIC, className, field, "[J"));
        add.add(new LdcInsnNode(slot));
        add.add(new InsnNode(Opcodes.DUP2));
        add.add(new InsnNode(Opcodes.LALOAD));
        add.add(new LdcInsnNode(n));
        add.add(new InsnNode(Opcodes.LADD));
        add.add(new InsnNode(Opcodes.LASTORE));
    }

    /**
     * A robot's profile for the calibration file: {@code group=n} for every group with work,
     * {@code static=n}, then the named slots with counts ({@code jdk:<method>=n},
     * {@code sized:<method>=n}, {@code alloc:<kind>=n}), comma-separated.
     */
    static String describe(long[] fuel) {
        if (fuel == null || fuel.length < SLOTS) {
            return "";
        }
        StringBuilder out = new StringBuilder();
        Group[] groups = Group.values();
        for (int g = 0; g < groups.length; g++) {
            if (fuel[BASE + g] != 0) {
                out.append(groups[g].name().toLowerCase()).append('=').append(fuel[BASE + g]).append(',');
            }
        }
        out.append("static=").append(fuel[STATIC]);
        synchronized (NAMES) {
            for (int i = 0; i < NAMES.size(); i++) {
                if (fuel[JDK_BASE + i] != 0) {
                    out.append(',').append(NAMES.get(i)).append('=').append(fuel[JDK_BASE + i]);
                }
            }
        }
        return out.toString();
    }
}
