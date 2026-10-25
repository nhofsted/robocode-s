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

**Java level:** compile with `--release 21`. robocode-s runs robots on a Temurin 21 JRE, so 21 is what your robot runs on: you get every language feature and library method (records, `var`, text blocks, `Math.clamp`), and a JDK 21 gives no warnings. Don't go above 21: the runtime can't load newer class files.

The one reason for a lower level is a robot that must also run in classic Robocode or in classic RoboRumble clients: classic 1.11.1 runs on Java 8, so compile such a robot with `--release 8` (`<maven.compiler.release>8</maven.compiler.release>` in Maven, `options.release = 8` in Gradle). The compiler then holds you to Java 8: `records are not supported in -source 8` (records need 16), `cannot find symbol` on a newer method such as `Math.clamp` (21). A JDK 21 also warns that 8 is obsolete; it works. Libraries count too: a library built for a newer level raises your robot's.

## 2. Build setup

Gradle (`build.gradle.kts`):

```kotlin
plugins { java }

repositories { mavenCentral() }

dependencies {
    // Provided at run time; never bundle it into your robot.
    compileOnly("net.sf.robocode:robocode.api:1.11.1")
}

tasks.withType<JavaCompile> { options.release = 21 }   // classes go to build/classes/java/main
```

Maven (`pom.xml`):

```xml
<properties>
  <maven.compiler.release>21</maven.compiler.release>
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

Put robot metadata in `src/main/resources/<package>/<Robot>.properties` ([Packaging](/docs/robot-development/release#package-it)); the build copies it next to the class.

## 3. Point robocode-s at the output

A folder of class files is a development folder: robocode-s reads it in place, and its robots are named with classic's `*` (a development robot has no fixed version).

```sh
robocode-s battle my.Robot*,sample.Walls --jar build/classes/java/main
```

In the GUI, open the options (the gear) and add the folder under the robot folders (`build/classes/java/main`, `target/classes`, or `bin` for Eclipse). Your robot appears in Start Battle; select it, add opponents and start. If its `.properties` names a version, the name is `my.Robot 1.0*`.

Recompile in the IDE and the next battle is new code. A battle already running keeps the old one.

## IntelliJ IDEA

1. New Project, Java, build system Gradle or Maven. Use the snippets above (IDEA reads the build file; pick a JDK 21 and keep `release` at 21).

   **Added the build file afterwards?** IDEA only reads `pom.xml` or `build.gradle.kts` in a project it created with that build system. In a plain Java project, `src/main/java` is just a folder, and nothing compiles against `robocode.api`. Right-click `pom.xml`, *Add as Maven Project* (Gradle: right-click `build.gradle.kts`, *Link Gradle Project*). You know it worked when `src/main/java` turns blue, `src/main/resources` purple, and *External Libraries* lists `robocode.api`.
2. Write `src/main/java/yourinitials/Kestrel.java` extending `AdvancedRobot`.
3. Build with Ctrl+F9 (Build Project) or `./gradlew classes`. The output is `build/classes/java/main` (Gradle) or `target/classes` (Maven). Without a build tool: File, Project Structure, add `robocode.api-1.11.1.jar` as a library, and the output is `out/production/<module>`.
4. Add a Run configuration of type Shell Script, or an External Tool (Settings, Tools, External Tools), running `robocode-s battle yourinitials.Kestrel*,sample.Walls --jar $ProjectFileDir$/build/classes/java/main`, so one key builds and fights.

## Eclipse

1. File, New, Java Project (or Gradle/Maven project with the snippets above). Set the compiler compliance to 21: Project, Properties, Java Compiler.
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

**Set it up once.** A run configuration of type *Application* with two things in it:

- the main class `io.github.nhofsted.robocodes.compat.RobotHost`, with every jar of your installation's `lib/robot-host` folder (the folder of the jar in [the table above](#1-get-the-api)) added to your project as a library;
- the program argument `--ide`.

How that looks in your IDE:

- **IntelliJ IDEA.**
  1. File, Project Structure (Ctrl+Alt+Shift+S), *Libraries*, **+**, *Java*. Paste the `lib/robot-host` folder into the file dialog's address bar (on Windows `AppData` is hidden, so browsing to it fails; see [the table above](#1-get-the-api)), select all the jars in it and confirm.
  2. IDEA asks which module gets the library: pick yours. If it didn't ask, or the library isn't listed under *External Libraries* afterwards, go to *Modules*, select your module, *Dependencies*, **+**, *Library…*, choose the one you just made and *Add Selected*. (The library exists in the project, but only a module's dependencies reach its classpath.)
  3. Run, Edit Configurations, **+**, *Application*. Main class `io.github.nhofsted.robocodes.compat.RobotHost`, program arguments `--ide`, and your module in the classpath box (`-cp`), so your build output is on the classpath.
  4. Build (Ctrl+F9), start robocode-s, and press the *Debug* button for this configuration.
- **Eclipse.** Run, Debug Configurations, *Java Application*, New. Choose your project, the main class above, and `--ide` under *Arguments*. Under *Classpath*, *User Entries*, *Add External JARs…*: the jars of `lib/robot-host`.
- **VS Code.** `robocode-s ide-config` prints a `launch.json` entry to paste into `.vscode/launch.json`; run it from *Run and Debug*.

That's all. The robot host finds your robots itself in your project's output (the class folders on its classpath), so there is nothing to change when you add or rename a robot. `robocode-s ide-config` prints the same, with ready-to-paste lines for IntelliJ IDEA, Eclipse and VS Code (the GUI's *From your IDE…* button, above the battle list, shows it with Copy buttons). It adds `--server` or `--data` only if you changed the port (7878 on 127.0.0.1; `--ide-port` and the options change it) or the data folder.

The robot proves it's yours with a token that robocode-s keeps in its data folder, readable by you only, so no other user on the machine can take the slot. The robot host reads it there; you never copy it.

**Then, each time:**

1. Start the battle. In the GUI, leave robocode-s open: it listens while it runs. On the command line, `robocode-s battle sample.Walls,@ide` (one `@ide` for each robot from the IDE; they come after the others in the battle) waits: *waiting for 1 robot from your IDE on 127.0.0.1:7878*.
2. Start the configuration with the IDE's Debug button. Your robots show in *From your IDE* at the top of the available robots, with a notice (up to 16; `--robot your.Kestrel` in the program arguments offers only that one). *Add to battle*, then Start Battle. On the command line the first one is taken.
3. Hit a breakpoint. The whole battle pauses with the robot: the others wait for it, however long you look. When you let it go, the battle goes on.
4. When the battle ends the robot's JVM ends with it. Start it again from the IDE for the next battle.

**What differs from a normal battle.** A robot you start from the IDE is never timed: no connect timeout, no turn deadline in classic mode, no hard limit in deterministic mode and no skipped turns by its work, and no limit for stopping it at the end of a round. Such a battle is a *debug battle*: the results, the recording and the replay say so, and it isn't comparable with a real one. The robot is named like any other (`your.Kestrel*`), and a second copy gets `(2)`. A few details:

- A robot from the IDE runs in the IDE's JVM, outside the sandbox, with its own home in robocode-s's data folder (`ide-home`; `--home` changes it): its data files are kept there, not with the other robots'.
- Teams need several JVMs, so they can't start from the IDE yet; the robot host leaves team robots out.
- Your robots come from class folders. A robot in a jar works with `--jar robot.jar`, but then the VM options also need `--add-opens=java.base/sun.net.www.protocol.jar=ALL-UNNAMED --add-opens=java.base/java.lang.reflect=ALL-UNNAMED` (the robot host says so).
- For seeded random numbers (the same battle again with `--seed N`), add `-javaagent:<the determinism agent>` to the VM options and `--seed N` to the program arguments; `ide-config` prints the path. Without it the robot's `Math.random()` isn't seeded.
- A deterministic battle (the default) needs the robot host of the same robocode-s version: use the jars `ide-config` names. `--classic` has no such limit.

**If it doesn't work.** What the robot host or the IDE says, and what to check:

| You see | Check |
|---|---|
| Your source folder is a plain folder; `robocode.api` can't be found | The build file isn't imported ([IntelliJ IDEA](#intellij-idea), step 1). |
| `records are not supported in -source 8`, `cannot find symbol` on a newer method | The compiler level in the build file is lower than your code needs: set it to 21 ([Java level](#1-get-the-api)). |
| `ClassNotFoundException` or `NoClassDefFoundError` for `io.github.nhofsted…` or `net.sf.robocode…` when the run configuration starts | The robot host's jars aren't on the configuration's classpath: not all of them, or the library isn't a dependency of your module (it must show under *External Libraries*). |
| `No class folders on the classpath` | The configuration doesn't have your module's classpath (IntelliJ's `-cp` box), so there is no build output to look in. |
| `No robots found in …: build your project first` | Nothing is compiled yet, or the `.class` files are elsewhere. Build, and check the output folder. |
| `robocode-s isn't listening on 127.0.0.1:7878` | robocode-s isn't running (open the GUI, or start `robocode-s battle …,@ide`), or it listens on another port (`--ide-port`; then add `--server 127.0.0.1:PORT` to the program arguments, as `ide-config` prints). |
| `server refused the robot` … `wrong IDE token` | The token doesn't match: the robot host and robocode-s use different data folders (`--data` names the one robocode-s uses, `ide-config` prints it when it isn't the default). |
| The battle refuses with `follows deterministic rules` | The robot host's jars are of another robocode-s version than the one running. After installing a new version, use its `lib/robot-host` jars (same folder, if it upgraded in place; re-index the library), or run a `--classic` battle. |
| `is a jar: robots in jars need the VM options …` | You gave `--jar robot.jar`. Give the class folder instead, or add the two `--add-opens` options it names. |

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
