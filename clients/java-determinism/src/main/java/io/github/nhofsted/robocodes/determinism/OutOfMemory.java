/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.Opcodes;
import org.objectweb.asm.tree.AbstractInsnNode;
import org.objectweb.asm.tree.ClassNode;
import org.objectweb.asm.tree.InsnList;
import org.objectweb.asm.tree.InsnNode;
import org.objectweb.asm.tree.LabelNode;
import org.objectweb.asm.tree.LdcInsnNode;
import org.objectweb.asm.tree.MethodInsnNode;
import org.objectweb.asm.tree.MethodNode;
import org.objectweb.asm.tree.TryCatchBlockNode;
import org.objectweb.asm.tree.TypeInsnNode;

import java.util.HashSet;
import java.util.Set;

/**
 * Notices an {@link OutOfMemoryError} in the robot's JVM (#178). When it hits depends on how
 * many bytes the JVM holds, and that includes the machine's paths (the robot's home, its jars):
 * the battle can differ between machines and between runs. The robot host reports it with
 * the robot's results, and such a battle is treated like one with the hard limit (ADR 0025).
 *
 * <p>The JVM throws a heap's {@code OutOfMemoryError} without calling a constructor, so it
 * is noticed where it is caught instead: every exception handler in robot classes and in
 * classic's robot side ({@code robocode/}, {@code net/sf/robocode/host/}) first passes what
 * it caught to {@link #caught} ({@code DUP; hook.accept}), and so does
 * {@code Thread.dispatchUncaughtException} (an uncaught one ends a thread of the robot's
 * own). Classic's robot thread catches every {@code Throwable} of the robot's code. The added
 * code is stack-neutral and straight-line, it allocates nothing, and in robot classes it is
 * added after the work metering: it changes neither the count nor anything else the robot
 * sees.
 */
public final class OutOfMemory {

    /** The handlers' hook, a {@code Consumer<Object>}: what the handler caught. */
    static final String CAUGHT_KEY = "robocodes.determinism.caught";

    private static volatile boolean happened;

    private OutOfMemory() {
    }

    /** Whether any thread of this JVM caught (or died of) an {@code OutOfMemoryError}. */
    public static boolean happened() {
        return happened;
    }

    /** Tests only: as if none happened. */
    static void reset() {
        happened = false;
    }

    /** A handler caught {@code thrown}. */
    static void caught(Object thrown) {
        if (thrown instanceof OutOfMemoryError) {
            happened = true;
        }
    }

    /** Classes whose handlers report what they catch, apart from robot classes. */
    static boolean isClassicRobotSide(String className) {
        return className != null && (className.startsWith("robocode/")
                || className.startsWith("net/sf/robocode/host/"));
    }

    /** Adds {@code DUP; hook.accept} at the start of every exception handler. Null: no handlers. */
    static byte[] patchHandlers(byte[] classfileBuffer) {
        ClassReader reader = new ClassReader(classfileBuffer);
        ClassNode node = new ClassNode(Opcodes.ASM9);
        reader.accept(node, 0);
        boolean changed = false;
        for (MethodNode method : node.methods) {
            Set<LabelNode> handlers = new HashSet<>();
            for (TryCatchBlockNode block : method.tryCatchBlocks) {
                if (!handlers.add(block.handler)) {
                    continue;
                }
                AbstractInsnNode first = block.handler;
                while (first != null && first.getOpcode() < 0) {
                    first = first.getNext();
                }
                if (first == null) {
                    continue;
                }
                // The handler's stack is the caught throwable alone; so it is after this.
                InsnList code = new InsnList();
                code.add(new InsnNode(Opcodes.DUP));
                code.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties",
                        "()Ljava/util/Properties;", false));
                code.add(new LdcInsnNode(CAUGHT_KEY));
                code.add(new MethodInsnNode(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                        "(Ljava/lang/Object;)Ljava/lang/Object;", false));
                code.add(new TypeInsnNode(Opcodes.CHECKCAST, "java/util/function/Consumer"));
                code.add(new InsnNode(Opcodes.SWAP));
                code.add(new MethodInsnNode(Opcodes.INVOKEINTERFACE, "java/util/function/Consumer", "accept",
                        "(Ljava/lang/Object;)V", true));
                method.instructions.insertBefore(first, code);
                changed = true;
            }
        }
        if (!changed) {
            return null;
        }
        // Straight-line and stack-neutral: the existing frames stay valid.
        ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_MAXS);
        node.accept(writer);
        return writer.toByteArray();
    }
}
