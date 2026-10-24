---
title: robocode-s replay
description: Re-run a recorded battle, check every turn against the recording and print the results.
---

```sh
robocode-s replay [--json] <recording>
```

Re-runs the recorded battle in the engine, checks every turn against the recording and prints the results. `--json` prints them as JSON.

A recording is made with [`robocode-s battle --record FILE`](/docs/cli/battle) (add `--paint` to keep the robots' debug painting), or by the GUI. It keeps the engine version it ran with. `robocode-s --version` shows yours, and a replay by another version warns.

To watch a recording rather than check it, open it in the GUI (Watch Battle, or `robocode-s-gui FILE`): it shows the robots' output and debug painting, and can be scrubbed through.
