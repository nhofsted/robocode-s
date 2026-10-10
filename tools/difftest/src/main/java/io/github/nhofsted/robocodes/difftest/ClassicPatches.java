/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.difftest;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;

import java.lang.instrument.ClassFileTransformer;
import java.lang.instrument.Instrumentation;
import java.security.ProtectionDomain;
import java.util.concurrent.atomic.AtomicBoolean;


/**
 * Java agent with timing-only fixes for classic Robocode 1.11.1, for use by the oracle.
 *
 * <p>{@code RobotPeer.startRound} starts the robot's thread and then waits for the
 * robot to report that it is asleep, without first checking whether it already
 * did. If the robot is quicker than the battle thread, the notification is lost
 * and the battle waits for the full timeout: up to 10 seconds normally, but
 * 10 minutes with {@code -Ddebug=true}. This patch makes that wait conditional,
 * as {@code RobotPeer.waitSleeping} already is. It changes timing only, never the
 * outcome of a battle.
 *
 * <p>(The per-robot {@code Utils.getRandom()} of ADR 0010 is done by the determinism
 * agent, which the robocode-s Java robot host uses too.)
 *
 * <p>Finally, {@code RobotPeer.performLoadCommands}, {@code publishStatus} and
 * {@code addEvent} call {@link OracleHooks} on entry, so the oracle can record the
 * robots' commands, the remaining values and the events per turn. The hooks only
 * observe.
 */
public final class ClassicPatches {

    private static volatile boolean installed;

    private ClassicPatches() {
    }

    public static void premain(String args, Instrumentation instrumentation) {
        instrumentation.addTransformer(new Transformer());
        installed = true;
    }

    /** Whether the agent is active. */
    public static boolean isInstalled() {
        return installed;
    }

    /**
     * Replacement for {@code isSleeping.wait(millis, nanos)} in {@code RobotPeer.startRound}.
     * The caller holds the monitor, so the check and the wait are atomic with respect to
     * the robot thread, which sets the flag and notifies under the same monitor.
     */
    public static void waitUnlessAsleep(Object isSleeping, long millis, int nanos) throws InterruptedException {
        if (!((AtomicBoolean) isSleeping).get()) {
            isSleeping.wait(millis, nanos);
        }
    }

    private static final class Transformer implements ClassFileTransformer {
        @Override
        public byte[] transform(Module module, ClassLoader loader, String className, Class<?> classBeingRedefined,
                ProtectionDomain protectionDomain, byte[] classfileBuffer) {
            if (!"net/sf/robocode/battle/peer/RobotPeer".equals(className)) {
                return null;
            }
            try {
                ClassReader reader = new ClassReader(classfileBuffer);
                // The hooks add stack use at method entry; no branches, so frames stay valid.
                ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_MAXS);
                int[] replaced = new int[1];
                int[] hooked = new int[1];
                reader.accept(new ClassVisitor(Opcodes.ASM9, writer) {
                    @Override
                    public MethodVisitor visitMethod(int access, String name, String descriptor, String signature,
                            String[] exceptions) {
                        MethodVisitor mv = super.visitMethod(access, name, descriptor, signature, exceptions);
                        String hook = switch (name + descriptor) {
                            case "performLoadCommands()V" -> "commandsLoaded";
                            case "publishStatus(J)V" -> "statusPublished";
                            case "addEvent(Lrobocode/Event;)V" -> "eventAdded";
                            default -> null;
                        };
                        if (hook != null) {
                            hooked[0]++;
                            boolean withEvent = hook.equals("eventAdded");
                            return new MethodVisitor(Opcodes.ASM9, mv) {
                                @Override
                                public void visitCode() {
                                    super.visitCode();
                                    mv.visitVarInsn(Opcodes.ALOAD, 0);
                                    if (withEvent) {
                                        mv.visitVarInsn(Opcodes.ALOAD, 1);
                                    }
                                    mv.visitMethodInsn(Opcodes.INVOKESTATIC,
                                            "io/github/nhofsted/robocodes/difftest/OracleHooks", hook,
                                            withEvent ? "(Ljava/lang/Object;Ljava/lang/Object;)V" : "(Ljava/lang/Object;)V",
                                            false);
                                }
                            };
                        }
                        if (!name.equals("startRound")) {
                            return mv;
                        }
                        return new MethodVisitor(Opcodes.ASM9, mv) {
                            @Override
                            public void visitCode() {
                                super.visitCode();
                                mv.visitVarInsn(Opcodes.ALOAD, 0);
                                mv.visitMethodInsn(Opcodes.INVOKESTATIC,
                                        "io/github/nhofsted/robocodes/difftest/OracleHooks", "robotStarted",
                                        "(Ljava/lang/Object;)V", false);
                            }

                            @Override
                            public void visitMethodInsn(int opcode, String owner, String method, String desc,
                                    boolean isInterface) {
                                if (opcode == Opcodes.INVOKEVIRTUAL && owner.equals("java/lang/Object")
                                        && method.equals("wait") && desc.equals("(JI)V")) {
                                    replaced[0]++;
                                    super.visitMethodInsn(Opcodes.INVOKESTATIC,
                                            "io/github/nhofsted/robocodes/difftest/ClassicPatches",
                                            "waitUnlessAsleep", "(Ljava/lang/Object;JI)V", false);
                                } else {
                                    super.visitMethodInsn(opcode, owner, method, desc, isInterface);
                                }
                            }
                        };
                    }
                }, 0);
                if (hooked[0] != 3) {
                    throw new IllegalStateException("expected 3 hooked RobotPeer methods, found " + hooked[0]);
                }
                if (replaced[0] != 1) {
                    throw new IllegalStateException("expected one wait() in RobotPeer.startRound, found " + replaced[0]);
                }
                return writer.toByteArray();
            } catch (Throwable t) {
                // A failed transform is otherwise silently ignored; make it impossible to miss.
                System.err.println("ClassicPatches: could not patch RobotPeer: " + t);
                Runtime.getRuntime().halt(3);
                return null;
            }
        }
    }
}
