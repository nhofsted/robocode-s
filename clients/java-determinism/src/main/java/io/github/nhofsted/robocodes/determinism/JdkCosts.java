/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import org.objectweb.asm.Opcodes;
import org.objectweb.asm.Type;
import org.objectweb.asm.tree.AbstractInsnNode;
import org.objectweb.asm.tree.ClassNode;
import org.objectweb.asm.tree.FieldInsnNode;
import org.objectweb.asm.tree.InsnList;
import org.objectweb.asm.tree.InsnNode;
import org.objectweb.asm.tree.IntInsnNode;
import org.objectweb.asm.tree.LdcInsnNode;
import org.objectweb.asm.tree.MethodInsnNode;
import org.objectweb.asm.tree.MethodNode;
import org.objectweb.asm.tree.VarInsnNode;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * The work of JDK calls that grows with their arguments (#15, ADR 0018). Every JDK call
 * counts its price ({@link WorkPrices}); a {@code System.arraycopy} of a million elements
 * or a sort of ten thousand would then cost as little as {@code Math.abs}, which budgets
 * would make unfair and robots could game. These calls also count per element (per
 * n·log2 n for sorts), in instructions of rules 2 ({@link WorkPrices#sized}), as measured on
 * the calibration machine at 160 ps per instruction: copies and fills about 1 instruction per
 * element, copies that allocate about 4, primitive sorts about 20 and object sorts about 50
 * per n·log2 n, linear list searches about 3.
 *
 * <p>The robot class calls a synthetic helper of its own instead, which adds the cost to
 * the robot's counter and then makes the original call (same dispatch), so robot classes
 * need no access to the agent. Rarely used calls (streams, regular expressions, I/O) and
 * string operations keep the flat cost.
 */
final class JdkCosts {

    private static final String PREFIX = "robocodes$jdk$";

    /** How a call's size is found among the helper's parameters (the receiver first). */
    private sealed interface Size {
    }

    /** An {@code int} parameter. */
    private record IntArg(int index) implements Size {
    }

    /** {@code to - from}, two {@code int} parameters. */
    private record Range(int from, int to) implements Size {
    }

    /** The length of an array parameter. */
    private record ArrayLength(int index) implements Size {
    }

    /** The {@code size()} of a collection parameter. */
    private record CollectionSize(int index) implements Size {
    }

    /** Instructions per unit; a unit is an element, or n·log2 n for sorts. */
    private record Cost(Size size, long perUnit, boolean nLogN) {
    }

    private static final Set<String> LISTS = Set.of("java/util/List", "java/util/ArrayList", "java/util/Vector",
            "java/util/LinkedList", "java/util/AbstractList", "java/util/Stack");

    private JdkCosts() {
    }

    /** The cost of a call (the receiver counts as the first parameter), or null for the flat cost. */
    static Cost cost(int opcode, String owner, String name, String desc) {
        if (opcode == Opcodes.INVOKESPECIAL) {
            return null; // super calls and constructors stay where they are
        }
        Type[] args = Type.getArgumentTypes(desc);
        boolean isStatic = opcode == Opcodes.INVOKESTATIC;
        if (owner.startsWith("[") && name.equals("clone") && args.length == 0) {
            return new Cost(new ArrayLength(0), 4, false);
        }
        switch (owner) {
            case "java/lang/System" -> {
                if (name.equals("arraycopy")) {
                    return new Cost(new IntArg(4), 1, false);
                }
            }
            case "java/util/Arrays" -> {
                if (args.length == 0 || args[0].getSort() != Type.ARRAY) {
                    return null;
                }
                boolean range = args.length >= 3 && args[1].getSort() == Type.INT && args[2].getSort() == Type.INT;
                Size size = range ? new Range(1, 2) : new ArrayLength(0);
                boolean objects = args[0].getElementType().getSort() == Type.OBJECT;
                switch (name) {
                    case "fill" -> {
                        return new Cost(size, 1, false);
                    }
                    case "copyOf" -> {
                        return new Cost(new IntArg(1), 4, false);
                    }
                    case "copyOfRange" -> {
                        return new Cost(new Range(1, 2), 4, false);
                    }
                    case "sort" -> {
                        return new Cost(size, objects ? 50 : 20, true);
                    }
                    case "hashCode" -> {
                        return new Cost(new ArrayLength(0), 1, false);
                    }
                    default -> {
                        return null;
                    }
                }
            }
            case "java/util/Collections" -> {
                switch (name) {
                    case "sort" -> {
                        return new Cost(new CollectionSize(0), 50, true);
                    }
                    case "reverse", "shuffle" -> {
                        return new Cost(new CollectionSize(0), 2, false);
                    }
                    case "min", "max" -> {
                        return new Cost(new CollectionSize(0), 3, false);
                    }
                    default -> {
                        return null;
                    }
                }
            }
            default -> {
            }
        }
        if (isStatic || !LISTS.contains(owner)) {
            return null;
        }
        return switch (name + desc) {
            case "sort(Ljava/util/Comparator;)V" -> new Cost(new CollectionSize(0), 50, true);
            case "contains(Ljava/lang/Object;)Z", "indexOf(Ljava/lang/Object;)I", "lastIndexOf(Ljava/lang/Object;)I",
                    "remove(Ljava/lang/Object;)Z", "removeAll(Ljava/util/Collection;)Z",
                    "retainAll(Ljava/util/Collection;)Z" -> new Cost(new CollectionSize(0), 3, false);
            case "addAll(Ljava/util/Collection;)Z" -> new Cost(new CollectionSize(1), 2, false);
            case "addAll(ILjava/util/Collection;)Z" -> new Cost(new CollectionSize(2), 2, false);
            case "toArray()[Ljava/lang/Object;", "toArray([Ljava/lang/Object;)[Ljava/lang/Object;" ->
                    new Cost(new CollectionSize(0), 2, false);
            default -> null;
        };
    }

    /**
     * Routes the class's sized JDK calls through helpers that count their size; adds the
     * helpers to the class. Call after the counting, so a call still counts as one JDK call.
     * New arrays pass their length through a helper that counts it too.
     */
    static void charge(ClassNode node, String fuelField) {
        Map<String, MethodNode> helpers = new LinkedHashMap<>();
        boolean itf = (node.access & Opcodes.ACC_INTERFACE) != 0;
        for (MethodNode method : List.copyOf(node.methods)) {
            for (AbstractInsnNode insn = method.instructions.getFirst(); insn != null; insn = insn.getNext()) {
                int op = insn.getOpcode();
                if ((op == Opcodes.NEWARRAY || op == Opcodes.ANEWARRAY)
                        && (WorkPrices.arrayElement(op == Opcodes.NEWARRAY) != 0 || WorkProfile.ON)) {
                    String kind = op == Opcodes.NEWARRAY ? "primitive" : "reference";
                    MethodNode helper = helpers.computeIfAbsent("alloc " + kind,
                            k -> arrayHelper(node.name, fuelField, PREFIX + helpers.size(), kind));
                    method.instructions.insertBefore(insn, new MethodInsnNode(Opcodes.INVOKESTATIC, node.name,
                            helper.name, helper.desc, itf));
                    continue;
                }
                if (!(insn instanceof MethodInsnNode call)) {
                    continue;
                }
                Cost cost = cost(call.getOpcode(), call.owner, call.name, call.desc);
                if (cost == null) {
                    continue;
                }
                String key = call.getOpcode() + " " + call.owner + "." + call.name + call.desc;
                MethodNode helper = helpers.computeIfAbsent(key,
                        k -> helper(node.name, fuelField, PREFIX + helpers.size(), call, cost));
                MethodInsnNode replacement = new MethodInsnNode(Opcodes.INVOKESTATIC, node.name, helper.name,
                        helper.desc, (node.access & Opcodes.ACC_INTERFACE) != 0);
                method.instructions.set(call, replacement);
                insn = replacement;
            }
        }
        node.methods.addAll(helpers.values());
    }

    /**
     * {@code fuel[1] += perUnit * max(0, units); return original(args)}: straight-line, so it
     * needs no frames.
     */
    private static MethodNode helper(String className, String fuelField, String name, MethodInsnNode call, Cost cost) {
        boolean isStatic = call.getOpcode() == Opcodes.INVOKESTATIC;
        Type[] callArgs = Type.getArgumentTypes(call.desc);
        Type[] params = new Type[callArgs.length + (isStatic ? 0 : 1)];
        if (!isStatic) {
            params[0] = Type.getObjectType(call.owner);
        }
        System.arraycopy(callArgs, 0, params, isStatic ? 0 : 1, callArgs.length);
        Type result = Type.getReturnType(call.desc);
        String desc = Type.getMethodDescriptor(result, params);
        MethodNode helper = new MethodNode(Opcodes.ACC_PRIVATE | Opcodes.ACC_STATIC | Opcodes.ACC_SYNTHETIC, name, desc,
                null, null);
        int[] slots = new int[params.length];
        for (int i = 0, slot = 0; i < params.length; i++) {
            slots[i] = slot;
            slot += params[i].getSize();
        }
        InsnList code = helper.instructions;
        code.add(new FieldInsnNode(Opcodes.GETSTATIC, className, fuelField, "[J"));
        code.add(new InsnNode(Opcodes.ICONST_1));
        code.add(new InsnNode(Opcodes.DUP2));
        code.add(new InsnNode(Opcodes.LALOAD));
        // The size, as an int.
        switch (cost.size()) {
            case IntArg a -> code.add(new VarInsnNode(Opcodes.ILOAD, slots[a.index()]));
            case Range r -> {
                code.add(new VarInsnNode(Opcodes.ILOAD, slots[r.to()]));
                code.add(new VarInsnNode(Opcodes.ILOAD, slots[r.from()]));
                code.add(new InsnNode(Opcodes.ISUB));
            }
            case ArrayLength a -> {
                code.add(new VarInsnNode(Opcodes.ALOAD, slots[a.index()]));
                code.add(new InsnNode(Opcodes.ARRAYLENGTH));
            }
            case CollectionSize c -> {
                code.add(new VarInsnNode(Opcodes.ALOAD, slots[c.index()]));
                code.add(new MethodInsnNode(Opcodes.INVOKEINTERFACE, "java/util/Collection", "size", "()I", true));
            }
        }
        if (cost.nLogN()) {
            // n * (32 - numberOfLeadingZeros(n)): n times its bit length.
            code.add(new InsnNode(Opcodes.DUP));
            code.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/Integer", "numberOfLeadingZeros", "(I)I",
                    false));
            code.add(new IntInsnNode(Opcodes.BIPUSH, 32));
            code.add(new InsnNode(Opcodes.SWAP));
            code.add(new InsnNode(Opcodes.ISUB));
            code.add(new InsnNode(Opcodes.I2L));
            code.add(new InsnNode(Opcodes.DUP2_X1));
            code.add(new InsnNode(Opcodes.POP2));
            code.add(new InsnNode(Opcodes.I2L));
            code.add(new InsnNode(Opcodes.LMUL));
        } else {
            code.add(new InsnNode(Opcodes.I2L));
        }
        // A negative size throws in the call; it must not lower the count.
        code.add(new InsnNode(Opcodes.LCONST_0));
        code.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/Math", "max", "(JJ)J", false));
        if (WorkProfile.ON) {
            // Calibration: the units also go to the call's profile slot.
            int units = params.length == 0 ? 0 : slots[params.length - 1] + params[params.length - 1].getSize();
            code.add(new InsnNode(Opcodes.DUP2));
            code.add(new VarInsnNode(Opcodes.LSTORE, units));
            code.add(new LdcInsnNode(WorkPrices.sized(cost.perUnit())));
            code.add(new InsnNode(Opcodes.LMUL));
            code.add(new InsnNode(Opcodes.LADD));
            code.add(new InsnNode(Opcodes.LASTORE));
            code.add(new FieldInsnNode(Opcodes.GETSTATIC, className, fuelField, "[J"));
            code.add(new LdcInsnNode(WorkProfile.slot("sized:" + call.owner + "." + call.name + call.desc)));
            code.add(new InsnNode(Opcodes.DUP2));
            code.add(new InsnNode(Opcodes.LALOAD));
            code.add(new VarInsnNode(Opcodes.LLOAD, units));
            code.add(new InsnNode(Opcodes.LADD));
            code.add(new InsnNode(Opcodes.LASTORE));
        } else {
            code.add(new LdcInsnNode(WorkPrices.sized(cost.perUnit())));
            code.add(new InsnNode(Opcodes.LMUL));
            code.add(new InsnNode(Opcodes.LADD));
            code.add(new InsnNode(Opcodes.LASTORE));
        }
        for (int i = 0; i < params.length; i++) {
            code.add(new VarInsnNode(params[i].getOpcode(Opcodes.ILOAD), slots[i]));
        }
        code.add(new MethodInsnNode(call.getOpcode(), call.owner, call.name, call.desc, call.itf));
        code.add(new InsnNode(result.getOpcode(Opcodes.IRETURN)));
        return helper;
    }

    /**
     * {@code (int n) -> { fuel[1] += price * max(0, n); return n; }} ({@link WorkPrices#arrayElement}), before a
     * new array of length {@code n}; in calibration also {@code fuel[alloc:<kind>] += max(0, n)}.
     * Straight-line, so it needs no frames.
     */
    private static MethodNode arrayHelper(String className, String fuelField, String name, String kind) {
        MethodNode helper = new MethodNode(Opcodes.ACC_PRIVATE | Opcodes.ACC_STATIC | Opcodes.ACC_SYNTHETIC, name,
                "(I)I", null, null);
        InsnList code = helper.instructions;
        long price = WorkPrices.arrayElement(kind.equals("primitive"));
        if (price != 0) {
            add(code, className, fuelField, 1, price);
        }
        if (WorkProfile.ON) {
            add(code, className, fuelField, WorkProfile.slot("alloc:" + kind), 1);
        }
        code.add(new VarInsnNode(Opcodes.ILOAD, 0));
        code.add(new InsnNode(Opcodes.IRETURN));
        return helper;
    }

    /** {@code fuel[slot] += perElement * max(0, n)}, n the helper's int parameter. */
    private static void add(InsnList code, String className, String fuelField, int slot, long perElement) {
        code.add(new FieldInsnNode(Opcodes.GETSTATIC, className, fuelField, "[J"));
        code.add(new LdcInsnNode(slot));
        code.add(new InsnNode(Opcodes.DUP2));
        code.add(new InsnNode(Opcodes.LALOAD));
        code.add(new VarInsnNode(Opcodes.ILOAD, 0));
        code.add(new InsnNode(Opcodes.I2L));
        code.add(new InsnNode(Opcodes.LCONST_0));
        code.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/Math", "max", "(JJ)J", false));
        if (perElement != 1) {
            code.add(new LdcInsnNode(perElement));
            code.add(new InsnNode(Opcodes.LMUL));
        }
        code.add(new InsnNode(Opcodes.LADD));
        code.add(new InsnNode(Opcodes.LASTORE));
    }
}
