---
title: Packaging and release
description: Package a robot as a jar with its properties file, learn how codesize sets its weight class, and release it to the rumble.
---

## Package it

A robot jar is the same as in classic: your classes plus a `.properties` file next to the robot class, for example `jd/MyRobot.properties`:

```properties
robot.classname=jd.MyRobot
robot.version=1.2
robot.author.name=Jane Doe
robot.description=Seesaws back and forth, spins its gun and fires at what it sees
robot.webpage=https://example.com/myrobot
robocode.version=1.11.1
```

The rumble identifies a robot as `classname version` (here `jd.MyRobot 1.2`), so **every release needs a new version string**. Classic's packager in the Robocode GUI writes this file for you; with Maven or Gradle, write it as a resource and let the build fill in the version, as [Your first robot](/docs/robot-development/example) shows.

**Codesize** decides your robot's weight class: under 250 bytes is a nano bot, under 750 a micro, under 1500 a mini, anything larger competes only in the general rumbles. Codesize counts the bytecode of your classes, not the jar size.

## Release it to the rumble

Submit a bot on the [submit page](/submit) after signing in with GitHub. [The rumble](/docs/rumble) explains what happens next: static checks, probation, then ranking.

1. **Sign in with GitHub** on this site and [submit](/submit) your jar, or a URL the rumble mirrors it from. Choose the divisions to enter (limits: a 5 MiB jar, 5 bots per account, 5 new versions of a bot per day). Your versions and their states are on [My bots](/account/bots).
2. **Static checks** run on upload: jar structure, the properties file, the robot class, codesize.
3. **Probation:** your new version fights a few battles on trusted clients. If it fails to load, crashes or gets disabled every round, it is deactivated and you get a message.
4. **Ranked:** after probation it enters normal scheduling and fights every other bot in its rumbles. Pairings start fresh for every new version; your previous version is retired.

Tips for a smooth release: test the exact jar you upload, keep data files small and expect them to start empty, and don't release several versions a day: each version needs a few hundred battles before its scores settle.
