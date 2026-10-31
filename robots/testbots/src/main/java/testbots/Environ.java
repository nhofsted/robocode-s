// SPDX-License-Identifier: EPL-2.0
package testbots;

import java.io.IOException;
import java.io.PrintStream;
import java.util.Map;
import java.util.TreeMap;
import robocode.AdvancedRobot;
import robocode.RobocodeFileOutputStream;

/**
 * Writes the environment and the system properties it sees to its data file
 * {@code environ.txt}, then drives in circles (test robot for ADR 0025: robots see a fixed
 * environment and neutral properties, never the user's).
 */
public class Environ extends AdvancedRobot {

    public void run() {
        if (getRoundNum() == 0) {
            try (PrintStream out = new PrintStream(new RobocodeFileOutputStream(getDataFile("environ.txt")))) {
                for (Map.Entry<String, String> e : new TreeMap<>(System.getenv()).entrySet()) {
                    out.print("env " + e.getKey() + "=" + e.getValue() + "\n");
                }
                for (String key : new String[] {"user.name", "user.home", "user.dir", "os.name", "java.home",
                        "line.separator", "user.language", "user.timezone", "file.encoding"}) {
                    out.print("property " + key + "=" + String.valueOf(System.getProperty(key)).replace("\n", "\\n")
                            + "\n");
                }
            } catch (IOException e) {
                out.println("can't write: " + e);
            }
        }
        while (true) {
            setAhead(100);
            setTurnRight(30);
            execute();
        }
    }
}
