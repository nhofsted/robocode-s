/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import java.util.Collections;
import java.util.Map;
import java.util.Properties;

/**
 * What robot code's {@code System.getenv}, {@code getProperty}, {@code getProperties} and
 * {@code lineSeparator} return in deterministic mode (ADR 0025, {@link WorkMeter}): no
 * environment, and neutral values for the properties that name the machine or its user,
 * the same on every machine. A robot that reads them then behaves the same everywhere, and
 * can't read the volunteer's secrets or name. Public, so robot classes link to it directly.
 */
public final class RobotSystem {

    /** The properties robot code sees with neutral values. */
    static final Map<String, String> NEUTRAL = Map.ofEntries(
            Map.entry("user.name", "robocode-s"),
            Map.entry("user.home", "/home/robot"),
            Map.entry("user.dir", "/home/robot"),
            Map.entry("os.name", "Linux"),
            Map.entry("os.version", "0"),
            Map.entry("os.arch", "amd64"),
            Map.entry("java.home", "/opt/java"),
            Map.entry("java.class.path", ""),
            Map.entry("java.library.path", ""),
            Map.entry("sun.boot.library.path", ""),
            Map.entry("jdk.module.path", ""),
            Map.entry("line.separator", "\n"));

    private RobotSystem() {
    }

    /** {@code System.getenv(name)}: no environment. */
    public static String getenv(String name) {
        name.getClass(); // the JDK's NullPointerException for null
        return null;
    }

    /** {@code System.getenv()}: no environment. */
    public static Map<String, String> getenv() {
        return Collections.emptyMap();
    }

    /** {@code System.getProperty(key)}. */
    public static String getProperty(String key) {
        String neutral = NEUTRAL.get(key);
        return neutral != null ? neutral : System.getProperty(key);
    }

    /** {@code System.getProperty(key, def)}. */
    public static String getProperty(String key, String def) {
        String neutral = NEUTRAL.get(key);
        return neutral != null ? neutral : System.getProperty(key, def);
    }

    /**
     * {@code System.getProperties()}: a copy, with the neutral values and without the agent's
     * hooks (objects, not text, that print differently every run).
     */
    public static Properties getProperties() {
        Properties copy = new Properties();
        for (String key : System.getProperties().stringPropertyNames()) {
            if (!key.startsWith("robocodes.")) {
                copy.setProperty(key, getProperty(key));
            }
        }
        return copy;
    }

    /** {@code System.lineSeparator()}: Unix's, on every machine. */
    public static String lineSeparator() {
        return "\n";
    }
}
