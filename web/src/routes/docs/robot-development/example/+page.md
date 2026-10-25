---
title: "Example: Jane Doe's first robot"
description: A worked example from an empty folder to a released robot jar, with Maven or Gradle, an IDE debugger and a library.
---

Jane Doe writes her first robot, `MyRobot`. This page follows her from an empty folder to a jar for the rumble: the names, the files, the build (with Maven or with Gradle), a battle, her IDE's debugger, a library and a release. Every file here is complete and builds as shown.

## 1. Pick the names

A robot's name is its full class name. Jane puts her robots in a package named after her initials, as is the custom in Robocode, so her robot is **`jd.MyRobot`**, and version 1.0 of it is `jd.MyRobot 1.0` in results and in the rumble.

- The **package** decides where the files go: `jd` means folders `jd/`. A longer package such as `jane.doe.robots` works too, but every results table then shows `jane.doe.robots.MyRobot`.
- The Maven `groupId` or Gradle `group` is something else: it only matters if you publish to a Maven repository. Jane uses `jd` there as well.
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

import robocode.AdvancedRobot;
import robocode.HitWallEvent;
import robocode.ScannedRobotEvent;
import robocode.util.Utils;

/** Keeps its radar on the enemy, circles it and fires where it is. */
public class MyRobot extends AdvancedRobot {

    /** 1 or -1: which way it circles. */
    private int direction = 1;

    @Override
    public void run() {
        // Turn gun and radar on their own, not with the body.
        setAdjustGunForRobotTurn(true);
        setAdjustRadarForGunTurn(true);
        out.println("MyRobot is ready");
        while (true) {
            // Until an enemy is seen; then onScannedRobot keeps the radar on it.
            if (getRadarTurnRemaining() == 0) {
                setTurnRadarRight(Double.POSITIVE_INFINITY);
            }
            execute();
        }
    }

    @Override
    public void onScannedRobot(ScannedRobotEvent e) {
        double bearing = getHeadingRadians() + e.getBearingRadians();
        // Radar: overshoot a little, so the enemy stays in the beam.
        setTurnRadarRightRadians(2 * Utils.normalRelativeAngle(bearing - getRadarHeadingRadians()));
        // Gun: where the enemy will be if it keeps going (linear targeting), and fire once it
        // points there.
        double power = Math.min(2, getEnergy() / 10);
        double sideways = e.getVelocity() * Math.sin(e.getHeadingRadians() - bearing);
        double aim = bearing + Math.asin(sideways / (20 - 3 * power));
        setTurnGunRightRadians(Utils.normalRelativeAngle(aim - getGunHeadingRadians()));
        if (Math.abs(getGunTurnRemaining()) < 10) {
            setFire(power);
        }
        // Body: sideways to the enemy, changing direction now and then.
        if (getTime() % 40 == 0) {
            direction = -direction;
        }
        setTurnRightRadians(Utils.normalRelativeAngle(e.getBearingRadians() + Math.PI / 2));
        setAhead(100 * direction);
    }

    @Override
    public void onHitWall(HitWallEvent e) {
        direction = -direction;
    }
}
```

## 4. The properties file

`src/main/resources/jd/MyRobot.properties`:

```properties
robot.classname=jd.MyRobot
robot.version=${project.version}
robot.author.name=Jane Doe
robot.description=Circles its enemy and fires where it is
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
    <maven.compiler.release>8</maven.compiler.release>
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

tasks.withType<JavaCompile> { options.release = 8 }

// Fills ${version} into MyRobot.properties.
tasks.processResources {
    inputs.property("version", version)
    filesMatching("**/*.properties") { expand("version" to version) }
}
```

`gradle jar` (or `./gradlew jar` with a wrapper) makes `build/libs/myrobot-1.0.jar`.

Both compile for Java 8, so the robot also runs in classic Robocode and in classic RoboRumble clients. A JDK 21 warns that 8 is obsolete; that is fine.

If her code later uses something newer (a `record`, `Math.clamp`, ...), the compiler says so (`records are not supported in -source 8`). She then raises the level, `<maven.compiler.release>21</maven.compiler.release>` or `options.release = 21`, and the robot runs in robocode-s but no longer in classic. Up to 21 works; above, the runtime can't load it.

In an IDE she opens the folder as a Maven or Gradle project, so that the IDE reads the build file: otherwise `src/main/java` is just a folder, and nothing finds `robocode.api`. In IntelliJ IDEA, if she added the build file to an existing project, right-click it and choose *Add as Maven Project* (or *Link Gradle Project*); `src/main/java` turns blue when it worked ([details](/docs/ide#intellij-idea)).

## 6. Fight

```sh
robocode-s battle "jd.MyRobot 1.0,sample.Walls" --jar target/myrobot-1.0.jar --rounds 5 --seed 1
```

(`build/libs/myrobot-1.0.jar` with Gradle; the quotes are for the space in the name.) Something like:

```
Rank  Robot Name      Total Score  Survival  Surv Bonus  Bullet Dmg  Bullet Bonus  ...  1sts  2nds
 1st  jd.MyRobot 1.0    852 (98%)       250          50         460            92  ...     5     0
 2nd  sample.Walls        14 (2%)         0           0          10             0  ...     0     5
```

The same seed gives the same battle again, so Jane can rerun one she wants to look at. Her `out.println` output goes to the console here, and to the robot's output tab in the GUI.

**While she works on it** she skips the jar and points robocode-s at the folder her IDE or build compiles to. A folder is a development robot, named with a `*`, and robocode-s reads it in place, so the next battle runs her newest code:

| Build | Folder | Robot name |
|---|---|---|
| Maven | `target/classes` | `jd.MyRobot 1.0*` |
| Gradle | `build/classes/java/main` | `jd.MyRobot*` (Gradle keeps the `.properties` in another folder, so there's no version) |

```sh
robocode-s battle "jd.MyRobot 1.0*,sample.Walls" --jar target/classes
```

In the GUI, she adds the folder under the robot folders in the options (the gear), and her robot appears in Start Battle.

## 7. Debug it in the IDE

To stop at a breakpoint, Jane adds one run configuration to her IDE, once:

- main class `io.github.nhofsted.robocodes.compat.RobotHost`, with the jars of robocode-s's `lib/robot-host` folder added to the project as a library (in IntelliJ: Project Structure, *Libraries*, **+**, *Java*, and make sure the library is a dependency of her module, which shows as *External Libraries*);
- program argument `--ide`.

She builds the project, starts robocode-s, presses her IDE's Debug button, and `jd.MyRobot` shows up under *From your IDE*. She adds it to the battle and starts. The battle waits whenever her robot is stopped at a breakpoint. [Using an IDE](/docs/ide#debug-it-with-your-ides-debugger) has the steps for IntelliJ IDEA, Eclipse and VS Code, and a table of what its messages mean if something doesn't work.

## 8. Use a library

Jane wants [Apache Commons Lang](https://commons.apache.org/proper/commons-lang/) in her robot, for example to format its output:

```java
import org.apache.commons.lang3.StringUtils;
...
        out.println(StringUtils.center(" MyRobot is ready ", 30, '*'));
```

A robot is one jar, and robocode-s gives a battle nothing else: no other jars, and the sandbox reads no other files. So the library's classes go **into** her jar.

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

- **Only what she uses.** `minimizeJar` and `minimize()` keep the library classes her robot reaches (here 140 of commons-lang3's several hundred) and drop the rest. That matters: [codesize](/docs/robot-development/release) counts every class in the jar, the library's too, so a library can move a robot out of the nano, micro and mini classes. A library that loads its classes by reflection can lose some this way; test the jar.
- **The library's Java level counts.** commons-lang3 3.17 runs on Java 8; a library built for Java 17 would make her robot need 17, and classic clients couldn't run it.
- **The library's license stays in the jar** (`META-INF/LICENSE.txt` and `NOTICE.txt`); the Apache license asks for that when you hand the library on.
- **The sandbox applies to the library too:** no network, no files outside her robot's data folder.
- **Classes named `java.*`, `javax.*`, `kotlin.*` or `robocode*` are never loaded from her jar**, so she doesn't bundle those (Kotlin's standard library is provided).
- **Test the jar itself** (`robocode-s battle "jd.MyRobot 1.0,sample.Walls" --jar target/myrobot-1.0.jar`). In the IDE's debugger the library is on the classpath anyway, so a robot that forgot to bundle it works there and fails in a real battle.

## 9. Release it

For each release Jane changes the version in one place, `<version>` in `pom.xml` or `version` in `build.gradle.kts` (the rumble needs a new version every time), builds, tests the jar, and submits it. [Packaging and release](/docs/robot-development/release) explains the checks the rumble runs and how codesize decides her weight class.
