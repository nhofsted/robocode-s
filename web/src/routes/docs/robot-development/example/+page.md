---
title: "Example: your first robot"
description: A worked example from an empty folder to a released robot jar, with Maven or Gradle, an IDE debugger and a library.
---

This page takes you from an empty folder to a robot jar for the rumble: the names, the files, the build (with Maven or with Gradle), a battle, your IDE's debugger, a library and a release. Every file here is complete and builds as shown.

The example author is Jane Doe, so the package is `jd` and the robot is `jd.MyRobot`. Use your own initials, package and class name wherever you see them.

## 1. Pick the names

A robot's name is its full class name. Put your robots in a package named after your initials, as is the custom in Robocode: for Jane Doe that is **`jd.MyRobot`**, and version 1.0 of it is `jd.MyRobot 1.0` in results and in the rumble.

- The **package** decides where the files go: `jd` means folders `jd/`. A longer package such as `jane.doe.robots` works too, but every results table then shows `jane.doe.robots.MyRobot`.
- The Maven `groupId` or Gradle `group` is something else: it only matters if you publish to a Maven repository. The example uses `jd` there as well.
- Classic Robocode's rules: the first part of the package can't be `robocode` and has at most 32 characters, the class name has at most 32, and the robot can't be a nested class.

## 2. The files

```
myrobot/
├── pom.xml                         with Maven (or build.gradle.kts and settings.gradle.kts with Gradle)
└── src/main/
    ├── java/jd/MyRobot.java        the robot
    └── resources/jd/MyRobot.properties
```

The `.properties` file must end up next to the class in the jar (`jd/MyRobot.class` and `jd/MyRobot.properties`), so its folder under `resources` matches the package, just as the source's folder under `java` does.

## 3. The robot

`src/main/java/jd/MyRobot.java`:

```java
package jd;

import robocode.HitByBulletEvent;
import robocode.Robot;
import robocode.ScannedRobotEvent;

/** A first robot, after the one in the classic Robocode tutorial: a seesaw that shoots what it sees. */
public class MyRobot extends Robot {

    @Override
    public void run() {
        out.println("MyRobot is ready");
        while (true) {
            ahead(100);        // forward, then
            turnGunRight(360); // a full turn of the gun scans for enemies
            back(100);         // backward, then
            turnGunRight(360); // the same again
        }
    }

    /** The gun saw another robot: fire. */
    @Override
    public void onScannedRobot(ScannedRobotEvent e) {
        fire(1);
    }

    /** A bullet hit us: turn across its path, to be harder to hit by the next one. */
    @Override
    public void onHitByBullet(HitByBulletEvent e) {
        turnLeft(90 - e.getBearing());
    }
}
```

This is the robot of the classic Robocode tutorial, `MyFirstRobot`, in your package:

- `run()` is the robot's main loop. It drives forward and back 100 pixels, and after each move turns the gun a full circle, which scans for other robots.
- `onScannedRobot` runs when the gun sees one, and fires a bullet of power 1.
- `onHitByBullet` runs when a bullet hits: the robot turns across the bullet's path.
- `out.println` writes to the robot's console.

It extends `Robot`, the simplest base class. `AdvancedRobot` adds non-blocking commands for robots that move, aim and fire in the same turn; switch when you need it. The [RoboWiki](https://robowiki.net/) is the place to learn how to make it better.

## 4. The properties file

`src/main/resources/jd/MyRobot.properties`:

```properties
robot.classname=jd.MyRobot
robot.version=${project.version}
robot.author.name=Jane Doe
robot.description=Seesaws back and forth, spins its gun and fires at what it sees
robocode.version=1.11.1
```

The version comes from the build file, so there is one place to change it for a release. With Gradle, write `robot.version=${version}` instead. More on these fields in [Packaging and release](/docs/robot-development/release).

## 5. The build

### With Maven

`pom.xml`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0"
         xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
         xsi:schemaLocation="http://maven.apache.org/POM/4.0.0 https://maven.apache.org/xsd/maven-4.0.0.xsd">
  <modelVersion>4.0.0</modelVersion>

  <groupId>jd</groupId>
  <artifactId>myrobot</artifactId>
  <!-- The robot's version: Maven writes it into MyRobot.properties. -->
  <version>1.0</version>

  <properties>
    <maven.compiler.release>21</maven.compiler.release>
    <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>
  </properties>

  <dependencies>
    <!-- Provided by Robocode when the robot runs: never bundled into the jar. -->
    <dependency>
      <groupId>net.sf.robocode</groupId>
      <artifactId>robocode.api</artifactId>
      <version>1.11.1</version>
      <scope>provided</scope>
    </dependency>
  </dependencies>

  <build>
    <resources>
      <!-- Fills ${project.version} into MyRobot.properties. -->
      <resource>
        <directory>src/main/resources</directory>
        <filtering>true</filtering>
      </resource>
    </resources>
    <plugins>
      <plugin>
        <groupId>org.apache.maven.plugins</groupId>
        <artifactId>maven-compiler-plugin</artifactId>
        <version>3.13.0</version>
      </plugin>
    </plugins>
  </build>
</project>
```

`mvn package` makes `target/myrobot-1.0.jar`.

### With Gradle

`settings.gradle.kts`:

```kotlin
rootProject.name = "myrobot"
```

`build.gradle.kts`:

```kotlin
plugins { java }

group = "jd"
version = "1.0"   // the robot's version: Gradle writes it into MyRobot.properties

repositories { mavenCentral() }

dependencies {
    // Provided by Robocode when the robot runs: never bundled into the jar.
    compileOnly("net.sf.robocode:robocode.api:1.11.1")
}

tasks.withType<JavaCompile> { options.release = 21 }

// Fills ${version} into MyRobot.properties.
tasks.processResources {
    inputs.property("version", version)
    filesMatching("**/*.properties") { expand("version" to version) }
}
```

`gradle jar` (or `./gradlew jar` with a wrapper) makes `build/libs/myrobot-1.0.jar`.

Both compile for Java 21, the Java that robocode-s runs robots on, so you can use everything in it (records, `var`, text blocks, `Math.clamp`). Above 21 the runtime can't load the class files. If you want your robot to run in classic Robocode and in classic RoboRumble clients too, set the level to 8 (`<maven.compiler.release>8</maven.compiler.release>` or `options.release = 8`); the compiler then holds you to Java 8 (`records are not supported in -source 8`).

In an IDE, open the folder as a Maven or Gradle project, so that the IDE reads the build file: otherwise `src/main/java` is just a folder, and nothing finds `robocode.api`. In IntelliJ IDEA, if you added the build file to an existing project, right-click it and choose *Add as Maven Project* (or *Link Gradle Project*); `src/main/java` turns blue when it worked ([details](/docs/ide#intellij-idea)).

## 6. Fight

```sh
robocode-s battle "jd.MyRobot 1.0,sample.Walls" --jar target/myrobot-1.0.jar --rounds 5 --seed 1
```

(`build/libs/myrobot-1.0.jar` with Gradle; the quotes are for the space in the name.) Something like:

```
Rank  Robot Name      Total Score  Survival  Surv Bonus  Bullet Dmg  Bullet Bonus  ...  1sts  2nds
 1st  sample.Walls      804 (94%)       250          50         418            84  ...     5     0
 2nd  jd.MyRobot 1.0      52 (6%)         0           0          52             0  ...     0     5
```

`sample.Walls` wins, and that is fine for a first robot: `MyRobot` drives, aims by spinning its gun and shoots, which is all the tutorial robot does, and you now have something to improve and a number to improve it against. The same seed gives the same battle again, so you can rerun one you want to look at. Your `out.println` output goes to the console here, and to the robot's output tab in the GUI.

**While you work on it**, skip the jar and point robocode-s at the folder your IDE or build compiles to. A folder is a development robot, named with a `*`, and robocode-s reads it in place, so the next battle runs your newest code:

| Build | Folder | Robot name |
|---|---|---|
| Maven | `target/classes` | `jd.MyRobot 1.0*` |
| Gradle | `build/classes/java/main` | `jd.MyRobot*` (Gradle keeps the `.properties` in another folder, so there's no version) |

```sh
robocode-s battle "jd.MyRobot 1.0*,sample.Walls" --jar target/classes
```

In the GUI, add the folder under the robot folders in the options (the gear), and your robot appears in Start Battle.

## 7. Debug it in the IDE

To stop at a breakpoint, add one run configuration to your IDE, once:

- main class `io.github.nhofsted.robocodes.compat.RobotHost`, with the jars of robocode-s's `lib/robot-host` folder added to the project as a library (in IntelliJ: Project Structure, *Libraries*, **+**, *Java*, and make sure the library is a dependency of your module, which shows as *External Libraries*);
- program argument `--ide`.

Build the project, start robocode-s, and press your IDE's Debug button: `jd.MyRobot` shows up under *From your IDE*. Add it to the battle and start. The battle waits whenever your robot is stopped at a breakpoint. [Using an IDE](/docs/ide#debug-it-with-your-ides-debugger) has the steps for IntelliJ IDEA, Eclipse and VS Code, and a table of what its messages mean if something doesn't work.

## 8. Use a library

To use a library such as [Apache Commons Lang](https://commons.apache.org/proper/commons-lang/) in your robot, for example to format its output:

```java
import org.apache.commons.lang3.StringUtils;
...
        out.println(StringUtils.center(" MyRobot is ready ", 30, '*'));
```

A robot is one jar, and robocode-s gives a battle nothing else: no other jars, and the sandbox reads no other files. So the library's classes go **into** your jar.

With Maven, the dependency and the shade plugin, which bundles it (in `<plugins>`, after the compiler plugin):

```xml
<!-- In <dependencies>: bundled into the robot jar by the shade plugin. -->
<dependency>
  <groupId>org.apache.commons</groupId>
  <artifactId>commons-lang3</artifactId>
  <version>3.17.0</version>
</dependency>
```

```xml
<!-- In <plugins>: the robot jar, target/myrobot-1.0.jar, with the library classes the robot uses. -->
<plugin>
  <groupId>org.apache.maven.plugins</groupId>
  <artifactId>maven-shade-plugin</artifactId>
  <version>3.6.0</version>
  <executions>
    <execution>
      <phase>package</phase>
      <goals><goal>shade</goal></goals>
      <configuration>
        <minimizeJar>true</minimizeJar>
        <createDependencyReducedPom>false</createDependencyReducedPom>
        <filters>
          <filter>
            <artifact>*:*</artifact>
            <excludes>
              <exclude>module-info.class</exclude>
              <exclude>META-INF/versions/**</exclude>
              <exclude>META-INF/maven/**</exclude>
            </excludes>
          </filter>
        </filters>
      </configuration>
    </execution>
  </executions>
</plugin>
```

With Gradle, the [Shadow](https://gradleup.com/shadow/) plugin does the same:

```kotlin
plugins {
    java
    id("com.gradleup.shadow") version "9.0.0"
}

dependencies {
    compileOnly("net.sf.robocode:robocode.api:1.11.1")
    // A library: bundled into the robot jar by shadowJar.
    implementation("org.apache.commons:commons-lang3:3.17.0")
}

// The robot jar, build/libs/myrobot-1.0.jar, with the library classes the robot uses.
tasks.shadowJar {
    archiveClassifier = ""
    minimize()
    exclude("module-info.class", "META-INF/versions/**", "META-INF/maven/**")
}
tasks.jar { enabled = false }
tasks.assemble { dependsOn(tasks.shadowJar) }
```

and `gradle assemble` builds the jar. Keep the rest of the file as in step 5.

What to know about libraries:

- **Only what you use.** `minimizeJar` and `minimize()` keep the library classes your robot reaches (here 140 of commons-lang3's several hundred) and drop the rest. That matters: [codesize](/docs/robot-development/release) counts every class in the jar, the library's too, so a library can move a robot out of the nano, micro and mini classes. A library that loads its classes by reflection can lose some this way; test the jar.
- **The library's Java level counts.** The robot runs on Java 21, so a library built for any level up to 21 works. commons-lang3 3.17 runs on Java 8, so it would also keep `MyRobot` usable in classic; a library built for Java 17 would not.
- **The library's license stays in the jar** (`META-INF/LICENSE.txt` and `NOTICE.txt`); the Apache license asks for that when you hand the library on.
- **The sandbox applies to the library too:** no network, no files outside your robot's data folder.
- **Classes named `java.*`, `javax.*`, `kotlin.*` or `robocode*` are never loaded from your jar**, so don't bundle those (Kotlin's standard library is provided).
- **Test the jar itself** (`robocode-s battle "jd.MyRobot 1.0,sample.Walls" --jar target/myrobot-1.0.jar`). In the IDE's debugger the library is on the classpath anyway, so a robot that forgot to bundle it works there and fails in a real battle.

## 9. Release it

For each release, change the version in one place, `<version>` in `pom.xml` or `version` in `build.gradle.kts` (the rumble needs a new version every time), build, test the jar, and submit it. [Packaging and release](/docs/robot-development/release) explains the checks the rumble runs and how codesize decides your weight class.
