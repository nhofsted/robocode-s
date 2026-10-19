robocode-s @VERSION@ (@PLATFORM@)
=================================

A Robocode reimplementation: a Rust battle engine with bit-exact classic
compatibility, a battlefield viewer, robots out of process. Licensed under the
EPL-2.0 (LICENSE); third-party notices: THIRD-PARTY-NOTICES.txt.
https://github.com/nhofsted/robocode-s

Starting
--------
@START@

The GUI needs a graphics driver (Vulkan or OpenGL). The Java runtime for the
robots (Eclipse Temurin 21, about 50 MB) is downloaded on the first battle; this
folder holds the robot host, the determinism agent and the sample robots
(robots/), and the programs find them next to themselves. The folder can be
moved or deleted as a whole.

Examples:
  robocode-s battle sample.Fire,sample.Walls,sampleteam.MyFirstTeam --rounds 10
  robocode-s battle --help
  robocode-s-gui

Your files
----------
The downloaded Java runtime and the robots' data files are kept in
  @DATA@
and the GUI's settings in
  @CONFIG@
(ROBOCODES_DATA and ROBOCODES_GUI_CONFIG name other places). Delete these to
remove everything robocode-s keeps besides this folder.

Sandbox
-------
Robots are code from anyone, so they run in a sandbox. `--sandbox os`, the
default, is @OSSANDBOX@. `--sandbox docker` runs each robot in a container
(needs Docker), `--sandbox none` is a plain process. All options:
`robocode-s battle --help`.
@EXTRA@
