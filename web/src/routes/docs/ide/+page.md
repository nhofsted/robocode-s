---
title: Using an IDE
description: Write a robot in IntelliJ IDEA, Eclipse or VS Code, build it with Gradle or Maven, and test it in robocode-s with output, debug painting, recordings and seeds.
---

You write a robot in your IDE against classic Robocode's API, and robocode-s runs the folder your IDE compiles into. Recompile, start a battle, and it runs the new code. No packaging step.

Breakpoints and stepping work too: your IDE's own debugger starts the robot, and robocode-s waits for it ([debug it from the IDE](#debug-it-with-your-ides-debugger), below).

## 1. Get the API

Your robot compiles against `robocode.api` 1.11.1, the classic API. You need it at compile time only; robocode-s provides it at run time.

- **From Maven Central:** `net.sf.robocode:robocode.api:1.11.1` (Gradle and Maven snippets below).
- **From your robocode-s installation**, if you build without Gradle or Maven: add this jar to your project as a library.

  | Installed with | The jar |
  |---|---|
  | Windows installer | `%LOCALAPPDATA%\Programs\robocode-s\lib\robot-host\robocode.api-1.11.1.jar` |
  | `.deb` or `.rpm` | `/usr/lib/robocode-s/lib/robot-host/robocode.api-1.11.1.jar` |
  | macOS app | `/Applications/robocode-s.app/Contents/Resources/lib/robot-host/robocode.api-1.11.1.jar` |
  | An unpacked archive | `lib/robot-host/robocode.api-1.11.1.jar` in the folder you unpacked |

  On Windows, `AppData` is a hidden folder: paste `%LOCALAPPDATA%\Programs\robocode-s\lib\robot-host` into Explorer's address bar (or the IDE's file dialog) to get there. `%LOCALAPPDATA%` is `C:\Users\<you>\AppData\Local`. In a macOS file dialog, Cmd+Shift+G takes a path; Finder shows the app's contents with right-click, Show Package Contents.

**Java level:** compile with `--release 8`. Classic Robocode 1.11.1 runs on Java 8, so a robot built for 8 also runs in classic and in classic RoboRumble clients, and robocode-s runs robots on a Temurin 21 JRE, which runs it unchanged. If you only target robocode-s, any level up to 21 works. Don't go above 21: the runtime can't load newer class files.

## 2. Build setup

Gradle (`build.gradle.kts`):

```kotlin
plugins { java }

repositories { mavenCentral() }

dependencies {
    // Provided at run time; never bundle it into your robot.
    compileOnly("net.sf.robocode:robocode.api:1.11.1")
}

tasks.withType<JavaCompile> { options.release = 8 }   // classes go to build/classes/java/main
```

Maven (`pom.xml`):

```xml
<properties>
  <maven.compiler.release>8</maven.compiler.release>
</properties>
<dependencies>
  <dependency>
    <groupId>net.sf.robocode</groupId>
    <artifactId>robocode.api</artifactId>
    <version>1.11.1</version>
    <scope>provided</scope>
  </dependency>
</dependencies>
<!-- classes go to target/classes -->
```

JDK 21 compiles `--release 8` with a warning that the level is deprecated; it works. Put robot metadata in `src/main/resources/<package>/<Robot>.properties` ([Packaging](/docs/robot-development/release#package-it)); the build copies it next to the class.

## 3. Point robocode-s at the output

A folder of class files is a development folder: robocode-s reads it in place, and its robots are named with classic's `*` (a development robot has no fixed version).

```sh
robocode-s battle my.Robot*,sample.Walls --jar build/classes/java/main
```

In the GUI, open the options (the gear) and add the folder under the robot folders (`build/classes/java/main`, `target/classes`, or `bin` for Eclipse). Your robot appears in Start Battle; select it, add opponents and start. If its `.properties` names a version, the name is `my.Robot 1.0*`.

Recompile in the IDE and the next battle is new code. A battle already running keeps the old one.

## IntelliJ IDEA

1. New Project, Java, build system Gradle or Maven. Use the snippets above (IDEA reads the build file; pick a JDK 21 and keep `release` at 8).
2. Write `src/main/java/yourinitials/Kestrel.java` extending `AdvancedRobot`.
3. Build with Ctrl+F9 (Build Project) or `./gradlew classes`. The output is `build/classes/java/main` (Gradle) or `target/classes` (Maven). Without a build tool: File, Project Structure, add `robocode.api-1.11.1.jar` as a library, and the output is `out/production/<module>`.
4. Add a Run configuration of type Shell Script, or an External Tool (Settings, Tools, External Tools), running `robocode-s battle yourinitials.Kestrel*,sample.Walls --jar $ProjectFileDir$/build/classes/java/main`, so one key builds and fights.

## Eclipse

1. File, New, Java Project (or Gradle/Maven project with the snippets above). Set the compiler compliance to 8: Project, Properties, Java Compiler.
2. Project, Properties, Java Build Path, Libraries, Add External JARs: `robocode.api-1.11.1.jar` (skip this with Gradle or Maven).
3. Eclipse builds on save into `bin` (plain projects) or `build/classes/java/main` / `target/classes` (Gradle, Maven). Point robocode-s at that folder.
4. Optional: Run, External Tools Configurations, a Program running `robocode-s` with `battle yourinitials.Kestrel*,sample.Walls --jar ${project_loc}/bin`.

## VS Code

1. Install the *Extension Pack for Java* (Red Hat's Java support). Open a Gradle or Maven folder with the snippets above, or a plain folder: `Java: Create Java Project`, then add `robocode.api-1.11.1.jar` under Java Projects, Referenced Libraries.
2. Use JDK 21 for the project and set the compiler level in the Gradle or Maven file as above; the extension follows the build file.
3. The output goes to `bin` (plain), `build/classes/java/main` (Gradle) or `target/classes` (Maven).
4. A task in `.vscode/tasks.json` runs the fight:

```json
{
  "version": "2.0.0",
  "tasks": [{
    "label": "fight",
    "type": "shell",
    "command": "./gradlew classes && robocode-s battle yourinitials.Kestrel*,sample.Walls --jar build/classes/java/main",
    "problemMatcher": []
  }]
}
```

## Debug it with your IDE's debugger

The IDE starts the robot itself (its Debug button), so breakpoints, stepping and watches work as in any Java program; there is no remote debug port to attach to. The robot waits for a battle, and robocode-s makes the battle from it.

**Set it up once.** `robocode-s ide-config` prints the run configuration, with ready-to-paste lines for IntelliJ IDEA, Eclipse and VS Code (the GUI's *From your IDE…* button, above the battle list, shows the same with Copy buttons). It has:

- the main class, `io.github.nhofsted.robocodes.compat.RobotHost`;
- the classpath: every jar in your installation's `lib/robot-host` folder (the folder of the jar in [the table above](#1-get-the-api)), besides your project's own;
- the program arguments `--ide --server 127.0.0.1:7878 --robot your.Robot --jar <output-folder> --home <folder>`: your robot's class name, and the folder your IDE compiles to;
- the token, in the environment variable `ROBOCODES_IDE_TOKEN`. It is made once and kept in robocode-s's data folder, readable by you only, so the configuration never changes and no other user on the machine can take the slot. The port is fixed too (7878 on 127.0.0.1; `--ide-port` and the options change it).

**Then, each time:**

1. Start the battle. On the command line, `robocode-s battle yourinitials.Kestrel,sample.Walls,@ide` (one `@ide` for each robot you start from the IDE; they come after the others in the battle) prints the configuration and waits: *waiting for 1 robot from your IDE on 127.0.0.1:7878*. In the GUI, leave robocode-s open: it listens while it runs. Start your robot with the IDE's Debug button; it shows in *From your IDE* at the top of the available robots, with a notice. *Add to battle*, then Start Battle.
2. Hit a breakpoint. The whole battle pauses with the robot: the others wait for it, however long you look. When you let it go, the battle goes on.
3. When the battle ends the robot's JVM ends with it. Start it again from the IDE for the next battle.

**What differs from a normal battle.** A robot you start from the IDE is never timed: no connect timeout, no turn deadline in classic mode, no hard limit in deterministic mode and no skipped turns by its work, and no limit for stopping it at the end of a round. Such a battle is a *debug battle*: the results, the recording and the replay say so, and it isn't comparable with a real one. The robot is named like any other (`your.Kestrel*`), and a second copy gets `(2)`. A few details:

- A robot from the IDE runs in the IDE's JVM, outside the sandbox, with your IDE's `--home` folder: its data files are kept there, not with the other robots'.
- Teams need several JVMs, so they can't start from the IDE yet.
- For seeded random numbers (the same battle again with `--seed N`), add `-javaagent:<the determinism agent>` to the VM options and `--seed N` to the program arguments; `ide-config` prints the path. Without it the robot's `Math.random()` isn't seeded.
- A deterministic battle (the default) needs the robot host of the same robocode-s version: use the jars `ide-config` names. `--classic` has no such limit.

## Find out what your robot does

Besides the debugger:

- **Output.** `out.println` goes to the robot's output tab in the GUI, and to the console on the command line.
- **Debug painting.** `onPaint(Graphics2D)` draws on the battlefield. In the GUI switch it on per robot with the pencil in its output tab. `--record FILE --paint` keeps the painting in a recording.
- **Debug properties.** `setDebugProperty(key, value)` shows the robot's current values in the GUI, and they are recorded with the battle.
- **Recordings and replays.** `--record FILE` records a battle; `robocode-s replay FILE` or the GUI's Watch Battle re-runs it, with scrubbing, output and painting.
- **Seeds.** Battles are deterministic by default: the same robots and `--seed N` give the same battle on every machine. When something goes wrong, note the seed and run it again. `--classic` uses classic's real-time timing instead. See [deterministic mode](/docs/robot-development/deterministic-mode).
- **Sandbox.** Robots run in a sandbox: no network, no files outside their own data folder. A robot that tries gets an exception. `--sandbox none` is for local experiments only.
- **Data files.** A robot's data files are kept from battle to battle, as in classic. `--fresh-data` starts every robot with none and keeps none; use it to reproduce a battle exactly.

More in [Developing a robot](/docs/robot-development).
