---
title: Using an IDE
description: Write a robot in IntelliJ IDEA, Eclipse or VS Code, build it with Gradle or Maven, and test it in robocode-s with output, debug painting, recordings and seeds.
---

You write a robot in your IDE against classic Robocode's API, and robocode-s runs the folder your IDE compiles into. Recompile, start a battle, and it runs the new code. No packaging step.

> **Status:** attaching your IDE's debugger (breakpoints, stepping) is not available yet; it comes with [#17](https://github.com/nhofsted/robocode-s/issues/17). Until then, use the robot's output, debug painting and properties, and recordings, below.

## 1. Get the API

Your robot compiles against `robocode.api` 1.11.1, the classic API. You need it at compile time only; robocode-s provides it at run time.

- **From Maven Central:** `net.sf.robocode:robocode.api:1.11.1` (Gradle and Maven snippets below).
- **From the install:** `robocode.api-1.11.1.jar` is in `lib/robot-host/` of the installation: `%LOCALAPPDATA%\Programs\robocode-s` on Windows, `/usr/lib/robocode-s` for the Linux packages. Add it to your project as a library if you build without Gradle or Maven.

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

JDK 21 compiles `--release 8` with a warning that the level is deprecated; it works. Put robot metadata in `src/main/resources/<package>/<Robot>.properties` ([Packaging](/docs/robot-development#package-it)); the build copies it next to the class.

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

## Find out what your robot does

Until a debugger can attach ([#17](https://github.com/nhofsted/robocode-s/issues/17)):

- **Output.** `out.println` goes to the robot's output tab in the GUI, and to the console on the command line.
- **Debug painting.** `onPaint(Graphics2D)` draws on the battlefield. In the GUI switch it on per robot with the pencil in its output tab. `--record FILE --paint` keeps the painting in a recording.
- **Debug properties.** `setDebugProperty(key, value)` shows the robot's current values in the GUI, and they are recorded with the battle.
- **Recordings and replays.** `--record FILE` records a battle; `robocode-s replay FILE` or the GUI's Watch Battle re-runs it, with scrubbing, output and painting.
- **Seeds.** Battles are deterministic by default: the same robots and `--seed N` give the same battle on every machine. When something goes wrong, note the seed and run it again. `--classic` uses classic's real-time timing instead. See [deterministic mode](/docs/robot-development#deterministic-mode).
- **Sandbox.** Robots run in a sandbox: no network, no files outside their own data folder. A robot that tries gets an exception. `--sandbox none` is for local experiments only.
- **Data files.** A robot's data files are kept from battle to battle, as in classic. `--fresh-data` starts every robot with none and keeps none; use it to reproduce a battle exactly.

More in [Developing a robot](/docs/robot-development).
