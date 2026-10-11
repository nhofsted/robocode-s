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
import org.objectweb.asm.commons.SerialVersionUIDAdder;
import org.objectweb.asm.tree.AbstractInsnNode;
import org.objectweb.asm.tree.InsnList;
import org.objectweb.asm.tree.InsnNode;
import org.objectweb.asm.tree.LdcInsnNode;
import org.objectweb.asm.tree.MethodInsnNode;
import org.objectweb.asm.tree.TypeInsnNode;

import java.util.Set;

/**
 * Deterministic identity hash codes for robots (ADR 0013).
 *
 * <p>Robots iterate {@code HashMap}s and {@code HashSet}s of their own objects, in an order
 * that follows the objects' identity hash codes. The JVM's are not reproducible, so:
 * <ul>
 *   <li>The top robot class of each hierarchy that would inherit {@code Object.hashCode()}
 *       gets its own: on first use it takes the next value of the robot's identity hash
 *       stream ({@link RobotRandomness#identityHashCode}) and keeps it in a field.</li>
 *   <li>{@code Enum.hashCode()} (final) asks the same hook instead of the JVM.</li>
 *   <li>Arrays can't get a field: the JDK's hash collections ({@link #HASHING_CLASSES}) and
 *       robot code ask another hook for an array's hash code, which keeps it in a table
 *       ({@link RobotRandomness#tableHash}). So does robot code's
 *       {@code System.identityHashCode(o)} and {@code super.hashCode()} on {@code Object}.</li>
 * </ul>
 * Values only depend on the battle seed, the robot's name and the order in which the robot
 * hashes its objects: the same in classic (one JVM) and robocode-s (one JVM per robot).
 * Off robot threads, and without the hook, the JVM's own identity hash is used.
 */
final class IdentityHashes {

    /** Classic's class loader for robot classes (one per robot). */
    static final String ROBOT_CLASS_LOADER = "net.sf.robocode.host.security.RobotClassLoader";

    private static final String FIELD = "robocodes$identityHash";
    private static final String HOOK = "java/util/function/ToIntFunction";

    /**
     * JDK classes whose {@code Object.hashCode()} calls ask the hook for arrays: the hash
     * collections a robot's arrays can be keys of, and {@code Objects.hashCode}.
     */
    static final Set<String> HASHING_CLASSES = Set.of("java/util/HashMap", "java/util/WeakHashMap",
            "java/util/Hashtable", "java/util/concurrent/ConcurrentHashMap", "java/util/Objects");

    private IdentityHashes() {
    }

    /** Whether a class is being defined by classic's robot class loader. */
    static boolean isRobotClassLoader(ClassLoader loader) {
        return loader != null && loader.getClass().getName().equals(ROBOT_CLASS_LOADER);
    }

    /**
     * Pushes the hook's hash for the object below the top of the stack, or the JVM's when
     * the hook is missing. Stack: {@code [..., o]} becomes {@code [..., hash]}. Leaves
     * {@code [..., o, o, hook]} at {@code fallback} for the caller's frame.
     */
    private static void emitHash(MethodVisitor mv, String key, Label fallback, Label end) {
        mv.visitInsn(Opcodes.DUP);
        mv.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties", "()Ljava/util/Properties;",
                false);
        mv.visitLdcInsn(key);
        mv.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                "(Ljava/lang/Object;)Ljava/lang/Object;", false);
        mv.visitInsn(Opcodes.DUP);
        mv.visitTypeInsn(Opcodes.INSTANCEOF, HOOK);
        mv.visitJumpInsn(Opcodes.IFEQ, fallback);
        mv.visitTypeInsn(Opcodes.CHECKCAST, HOOK);
        mv.visitInsn(Opcodes.SWAP);
        mv.visitMethodInsn(Opcodes.INVOKEINTERFACE, HOOK, "applyAsInt", "(Ljava/lang/Object;)I", true);
        mv.visitInsn(Opcodes.SWAP);
        mv.visitInsn(Opcodes.POP);
        mv.visitJumpInsn(Opcodes.GOTO, end);
    }

    /** At {@code fallback}: {@code [..., o, o, hook]} becomes {@code [..., hash]}. */
    private static void emitFallback(MethodVisitor mv) {
        mv.visitInsn(Opcodes.POP);
        mv.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/System", "identityHashCode", "(Ljava/lang/Object;)I",
                false);
        mv.visitInsn(Opcodes.SWAP);
        mv.visitInsn(Opcodes.POP);
    }

    /** {@code Enum.hashCode()}: {@code System.identityHashCode(this)} becomes the hook. */
    static byte[] patchEnum(byte[] classfileBuffer) {
        ClassReader reader = new ClassReader(classfileBuffer);
        ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS) {
            @Override
            protected String getCommonSuperClass(String type1, String type2) {
                return "java/lang/Object"; // no class loading inside java.base transforms
            }
        };
        int[] patched = new int[1];
        reader.accept(new ClassVisitor(Opcodes.ASM9, writer) {
            @Override
            public MethodVisitor visitMethod(int access, String name, String descriptor, String signature,
                    String[] exceptions) {
                MethodVisitor mv = super.visitMethod(access, name, descriptor, signature, exceptions);
                if (!name.equals("hashCode") || !descriptor.equals("()I")) {
                    return mv;
                }
                return new MethodVisitor(Opcodes.ASM9, mv) {
                    @Override
                    public void visitMethodInsn(int opcode, String owner, String method, String desc,
                            boolean isInterface) {
                        if (opcode == Opcodes.INVOKESTATIC && owner.equals("java/lang/System")
                                && method.equals("identityHashCode")) {
                            patched[0]++;
                            Label fallback = new Label();
                            Label end = new Label();
                            emitHash(mv, RobotRandomness.IDENTITY_HASH_KEY, fallback, end);
                            mv.visitLabel(fallback);
                            emitFallback(mv);
                            mv.visitLabel(end);
                        } else {
                            super.visitMethodInsn(opcode, owner, method, desc, isInterface);
                        }
                    }
                };
            }
        }, 0);
        if (patched[0] != 1) {
            throw new IllegalStateException("expected one identityHashCode call in Enum.hashCode(), found " + patched[0]);
        }
        return writer.toByteArray();
    }

    /**
     * A JDK class of {@link #HASHING_CLASSES}: every {@code o.hashCode()} with {@code o} typed
     * {@code Object} becomes
     * <pre>
     * o.getClass().isArray() ? hook(o) : o.hashCode()
     * </pre>
     * with the JVM's identity hash when the hook is missing. Other objects take the same path
     * as before, at the cost of one {@code isArray()} check.
     */
    static byte[] patchHashCalls(byte[] classfileBuffer) {
        ClassReader reader = new ClassReader(classfileBuffer);
        ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS) {
            @Override
            protected String getCommonSuperClass(String type1, String type2) {
                return "java/lang/Object"; // no class loading inside java.base transforms
            }
        };
        int[] patched = new int[1];
        reader.accept(new ClassVisitor(Opcodes.ASM9, writer) {
            @Override
            public MethodVisitor visitMethod(int access, String name, String descriptor, String signature,
                    String[] exceptions) {
                return new MethodVisitor(Opcodes.ASM9, super.visitMethod(access, name, descriptor, signature,
                        exceptions)) {
                    @Override
                    public void visitMethodInsn(int opcode, String owner, String method, String desc,
                            boolean isInterface) {
                        if (opcode != Opcodes.INVOKEVIRTUAL || !owner.equals("java/lang/Object")
                                || !method.equals("hashCode") || !desc.equals("()I")) {
                            super.visitMethodInsn(opcode, owner, method, desc, isInterface);
                            return;
                        }
                        patched[0]++;
                        Label plain = new Label();
                        Label fallback = new Label();
                        Label end = new Label();
                        mv.visitInsn(Opcodes.DUP);
                        mv.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/lang/Object", "getClass",
                                "()Ljava/lang/Class;", false);
                        mv.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/lang/Class", "isArray", "()Z", false);
                        mv.visitJumpInsn(Opcodes.IFEQ, plain);
                        emitHash(mv, RobotRandomness.HASH_CODE_KEY, fallback, end);
                        mv.visitLabel(fallback);
                        emitFallback(mv);
                        mv.visitJumpInsn(Opcodes.GOTO, end);
                        mv.visitLabel(plain);
                        mv.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/lang/Object", "hashCode", "()I", false);
                        mv.visitLabel(end);
                    }
                };
            }
        }, 0);
        if (patched[0] == 0) {
            throw new IllegalStateException("no Object.hashCode() call found in " + reader.getClassName());
        }
        return writer.toByteArray();
    }

    /**
     * In a robot class's code: calls that can reach the JVM's identity hash ask a hook
     * instead. {@code o.hashCode()} on an array or {@code Object} goes to
     * {@link RobotRandomness#hashCode}; {@code System.identityHashCode(o)} and
     * {@code super.hashCode()} on {@code Object} to {@link RobotRandomness#tableHash}.
     * Straight-line and stack-neutral, like the work counting: frames stay valid.
     */
    static void replaceHashCalls(InsnList instructions) {
        for (AbstractInsnNode insn = instructions.getFirst(); insn != null; insn = insn.getNext()) {
            if (!(insn instanceof MethodInsnNode call)) {
                continue;
            }
            String key = hookFor(call);
            if (key == null) {
                continue;
            }
            // [..., o] becomes [..., hook, o] and then [..., hash].
            InsnList hook = new InsnList();
            hook.add(new MethodInsnNode(Opcodes.INVOKESTATIC, "java/lang/System", "getProperties",
                    "()Ljava/util/Properties;", false));
            hook.add(new LdcInsnNode(key));
            hook.add(new MethodInsnNode(Opcodes.INVOKEVIRTUAL, "java/util/Properties", "get",
                    "(Ljava/lang/Object;)Ljava/lang/Object;", false));
            hook.add(new TypeInsnNode(Opcodes.CHECKCAST, HOOK));
            hook.add(new InsnNode(Opcodes.SWAP));
            hook.add(new MethodInsnNode(Opcodes.INVOKEINTERFACE, HOOK, "applyAsInt", "(Ljava/lang/Object;)I", true));
            AbstractInsnNode last = hook.getLast();
            instructions.insert(insn, hook);
            instructions.remove(insn);
            insn = last;
        }
    }

    private static String hookFor(MethodInsnNode call) {
        if (call.getOpcode() == Opcodes.INVOKESTATIC && call.owner.equals("java/lang/System")
                && call.name.equals("identityHashCode") && call.desc.equals("(Ljava/lang/Object;)I")) {
            return RobotRandomness.TABLE_HASH_KEY;
        }
        if (!call.name.equals("hashCode") || !call.desc.equals("()I")) {
            return null;
        }
        if (call.getOpcode() == Opcodes.INVOKEVIRTUAL
                && (call.owner.equals("java/lang/Object") || call.owner.startsWith("["))) {
            return RobotRandomness.HASH_CODE_KEY;
        }
        if (call.getOpcode() == Opcodes.INVOKESPECIAL && call.owner.equals("java/lang/Object")) {
            return RobotRandomness.TABLE_HASH_KEY;
        }
        return null;
    }

    /**
     * A robot class: adds the deterministic {@code hashCode()} if it is the top robot class
     * of a hierarchy that would inherit {@code Object.hashCode()}. Returns null to leave
     * the class unchanged.
     */
    static byte[] patchRobotClass(ClassLoader loader, byte[] classfileBuffer) {
        ClassReader reader = new ClassReader(classfileBuffer);
        Scan scan = new Scan();
        reader.accept(scan, ClassReader.SKIP_DEBUG | ClassReader.SKIP_FRAMES);
        int skip = Opcodes.ACC_INTERFACE | Opcodes.ACC_ANNOTATION | Opcodes.ACC_ENUM | Opcodes.ACC_MODULE;
        String superName = reader.getSuperName();
        if ((reader.getAccess() & skip) != 0 || scan.declaresHashCode || superName == null
                || superName.equals("java/lang/Record") || !inheritsObjectHashCode(loader, superName)) {
            return null;
        }
        ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_MAXS);
        // Adding hashCode() would change the default serialVersionUID of a Serializable
        // class; the adder first fixes it to the original class's value.
        reader.accept(new SerialVersionUIDAdder(Opcodes.ASM9, new Injector(writer)) {
        }, 0);
        return writer.toByteArray();
    }

    /**
     * Whether a robot class whose superclass is {@code superName} inherits
     * {@code Object.hashCode()} from outside the robot. A superclass the parent loader
     * can't find is a robot class itself: its own top class gets the hash code.
     */
    private static boolean inheritsObjectHashCode(ClassLoader loader, String superName) {
        try {
            Class<?> superClass = Class.forName(superName.replace('/', '.'), false, loader.getParent());
            return superClass.getMethod("hashCode").getDeclaringClass() == Object.class;
        } catch (ClassNotFoundException | NoSuchMethodException | LinkageError e) {
            return false;
        }
    }

    /** What the agent needs to know about a robot class before changing it. */
    private static final class Scan extends ClassVisitor {
        boolean declaresHashCode;

        Scan() {
            super(Opcodes.ASM9);
        }

        @Override
        public MethodVisitor visitMethod(int access, String name, String descriptor, String signature,
                String[] exceptions) {
            if (name.equals("hashCode") && descriptor.equals("()I")) {
                declaresHashCode = true;
            }
            return null;
        }
    }

    /**
     * Adds {@code private transient int robocodes$identityHash} and
     * <pre>
     * public int hashCode() {
     *     int h = robocodes$identityHash;
     *     if (h == 0) { h = robocodes$identityHash = hook(this); }
     *     return h;
     * }
     * </pre>
     */
    private static final class Injector extends ClassVisitor {
        private String name;
        private int version;

        Injector(ClassVisitor next) {
            super(Opcodes.ASM9, next);
        }

        @Override
        public void visit(int version, int access, String name, String signature, String superName,
                String[] interfaces) {
            this.name = name;
            this.version = version & 0xFFFF;
            super.visit(version, access, name, signature, superName, interfaces);
        }

        @Override
        public void visitEnd() {
            cv.visitField(Opcodes.ACC_PRIVATE | Opcodes.ACC_TRANSIENT | Opcodes.ACC_SYNTHETIC, FIELD, "I", null,
                    null).visitEnd();
            MethodVisitor mv = cv.visitMethod(Opcodes.ACC_PUBLIC, "hashCode", "()I", null, null);
            // Older class files (before Java 6) have no stack map frames.
            boolean frames = version >= Opcodes.V1_6;
            Object[] locals = {name, Opcodes.INTEGER};
            mv.visitCode();
            mv.visitVarInsn(Opcodes.ALOAD, 0);
            mv.visitFieldInsn(Opcodes.GETFIELD, name, FIELD, "I");
            mv.visitVarInsn(Opcodes.ISTORE, 1);
            mv.visitVarInsn(Opcodes.ILOAD, 1);
            Label done = new Label();
            mv.visitJumpInsn(Opcodes.IFNE, done);
            mv.visitVarInsn(Opcodes.ALOAD, 0);
            mv.visitVarInsn(Opcodes.ALOAD, 0);
            Label fallback = new Label();
            Label end = new Label();
            emitHash(mv, RobotRandomness.IDENTITY_HASH_KEY, fallback, end);
            mv.visitLabel(fallback);
            if (frames) {
                mv.visitFrame(Opcodes.F_FULL, 2, locals, 4, new Object[] {name, name, name, "java/lang/Object"});
            }
            emitFallback(mv);
            mv.visitLabel(end);
            if (frames) {
                mv.visitFrame(Opcodes.F_FULL, 2, locals, 2, new Object[] {name, Opcodes.INTEGER});
            }
            mv.visitInsn(Opcodes.DUP_X1);
            mv.visitFieldInsn(Opcodes.PUTFIELD, name, FIELD, "I");
            mv.visitVarInsn(Opcodes.ISTORE, 1);
            mv.visitLabel(done);
            if (frames) {
                mv.visitFrame(Opcodes.F_FULL, 2, locals, 0, new Object[0]);
            }
            mv.visitVarInsn(Opcodes.ILOAD, 1);
            mv.visitInsn(Opcodes.IRETURN);
            mv.visitMaxs(0, 0);
            mv.visitEnd();
            super.visitEnd();
        }
    }
}
