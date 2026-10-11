/*
 * SPDX-License-Identifier: EPL-2.0
 */
package net.sf.robocode.host.security;

import java.io.IOException;
import java.io.InputStream;
import java.util.Set;

/**
 * Stands in for classic's robot class loader in tests: the agent recognizes robot classes
 * by this class name. Defines the given classes itself (child-first) from the test classes.
 */
public final class RobotClassLoader extends ClassLoader {

    private final Set<String> robotClasses;

    public RobotClassLoader(ClassLoader parent, Set<String> robotClasses) {
        super(parent);
        this.robotClasses = robotClasses;
    }

    @Override
    protected Class<?> loadClass(String name, boolean resolve) throws ClassNotFoundException {
        if (!robotClasses.contains(name)) {
            return super.loadClass(name, resolve);
        }
        synchronized (getClassLoadingLock(name)) {
            Class<?> loaded = findLoadedClass(name);
            if (loaded != null) {
                return loaded;
            }
            try (InputStream in = getParent().getResourceAsStream(name.replace('.', '/') + ".class")) {
                byte[] bytes = in.readAllBytes();
                return defineClass(name, bytes, 0, bytes.length);
            } catch (IOException e) {
                throw new ClassNotFoundException(name, e);
            }
        }
    }
}
