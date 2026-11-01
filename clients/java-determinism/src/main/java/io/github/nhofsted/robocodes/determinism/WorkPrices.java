/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import io.github.nhofsted.robocodes.determinism.WorkProfile.Group;
import org.objectweb.asm.tree.AbstractInsnNode;
import org.objectweb.asm.tree.MethodInsnNode;

import java.util.EnumMap;
import java.util.Map;

/**
 * What robot code's work costs, in counted units of {@link RobotRandomness#PICOS_PER_UNIT}
 * each (#15, ADR 0014, ADR 0018): a price per kind of instruction ({@link Group}), per call
 * into the JDK (some methods priced on their own), per unit of a sized JDK call
 * ({@link JdkCosts}) and per element of a new array. Part of the deterministic rules.
 *
 * <p>Prices are fixed when a class is instrumented: a block's cost is one constant, so
 * prices cost nothing at run time.
 */
final class WorkPrices {

    /** Units of one instruction in rules 2, which counted every instruction alike at 160 ps. */
    static final long INSTRUCTION = 16;

    private static final Map<Group, Long> GROUPS = new EnumMap<>(Group.class);
    /** A JDK call's work inside the JDK, on top of the call instruction's own price. */
    private static final long JDK_CALL = 10 * INSTRUCTION;
    /** JDK methods with prices of their own ({@code owner.name desc}): the work inside the JDK. */
    private static final Map<String, Long> JDK_METHODS = Map.of();

    static {
        for (Group g : Group.values()) {
            GROUPS.put(g, INSTRUCTION);
        }
    }

    private WorkPrices() {
    }

    /** The price of one instruction; {@code jdk}: a call into the JDK. */
    static long of(AbstractInsnNode insn, boolean jdk) {
        long price = GROUPS.get(WorkProfile.group(insn, jdk));
        if (jdk && insn instanceof MethodInsnNode call) {
            price += JDK_METHODS.getOrDefault(call.owner + "." + call.name + call.desc, JDK_CALL);
        }
        return price;
    }

    /** Units per unit of a sized JDK call that rules 2 priced at {@code instructions} each. */
    static long sized(long instructions) {
        return instructions * INSTRUCTION;
    }

    /** Units per element of a new array ({@code newarray}: primitive; {@code anewarray}). */
    static long arrayElement(boolean primitive) {
        return 0;
    }
}
