# 0022. The programs look for a newer release and install it when the user agrees

- Status: Proposed
- Date: 2026-10-25

## Context

robocode-s ships as archives (.zip, .tar.gz, the macOS .app and .dmg), a per-user Windows installer (also what winget runs), .deb and .rpm packages from our apt/dnf repository, and a Homebrew cask (M3, #79). Nothing told users of a new release: they found out from the website, or from the rumble refusing them (`minClient`, an engine version the client doesn't have). Rumble volunteers in particular need to keep up with the engine the rumble ranks. Code signing is postponed, so whatever updates the programs must not lean on signatures, and must not surprise users who chose a package manager.

## Decision

- **What is current** is `/release.json` on the website, made from `web/src/lib/release.ts` at build time (`{version, notes, download, files}`). It changes when the site is deployed, which the owner does at the end of a release, so the deploy is the switch that makes the programs offer it. GitHub's "latest release" is not used: it flips when the draft is published, before the files are complete and the other channels have it.
- **When:** the GUI looks once a day at start (in the background; option "Look for a newer release once a day", on by default; "Look now" on the options page); the command line looks once a day in the background and says so on stderr when it is a terminal; `robocode-s update` looks at once. `ROBOCODES_NO_UPDATE_CHECK` turns the daily looks off. A build from source never looks. The state (last look, the release found, a skipped release, when the user was last told) is `update.json` in the user data folder.
- **Asking:** nothing is downloaded before the user agrees. The GUI's notice offers *Update and restart*, *What's new*, *Skip this version* and *Later* (a day); updating waits while a battle runs or the rumble is being run. `robocode-s update` asks y/N on a terminal and refuses without one unless `--yes`.
- **How depends on the installation** (`crates/update`, `Install::detect`):
  - *Archive or .app the user put somewhere:* the release's archive for the platform is downloaded, checked against the release's `SHA256SUMS`, unpacked next to the installation and swapped in entry by entry (the old entries moved aside, then deleted). Entries the new release has are replaced whole, so no old jar stays in `lib`; robots the user added to `robots` are carried over, the old sample jars are not; anything else the user put in the folder stays. On an error everything moved is moved back. A running program can be moved (also on Windows, where what can't be deleted yet is removed at the next start). The GUI then starts the new release, which waits for the old process (the IDE port).
  - *Windows installer's installation* (`uninstall.exe` in the folder): the new installer runs with `/S /UPDATE /RELAUNCH`. `/UPDATE` keeps the components the installation has (Start menu, desktop, PATH, file types) and waits up to two minutes for the programs to exit; `/RELAUNCH` starts the GUI after.
  - *apt, dnf* (`/usr/lib/robocode-s` with our repository's source file) and *Homebrew* (a Caskroom entry): the package manager owns the files, so the programs show the command (`sudo apt update && sudo apt install --only-upgrade robocode-s`, `sudo dnf upgrade --refresh robocode-s`, `brew upgrade --cask robocode-s`).
  - *A .deb or .rpm installed without the repository:* the download page.
- **Trust:** the archive's checksum comes from the same GitHub release over TLS: it catches a broken download, not a compromised release or account. That is the same trust as downloading from the website by hand; signing (postponed) would be the next step.

## Consequences

- Releases need one change of order: deploy the site *after* the apt/dnf repository has the release (winget and Homebrew users aren't told a command that depends on it, but apt/dnf users are; until the repository has it, their command finds nothing new). A release must keep the file names `robocode-s-<v>-<platform>.<ext>` and the archive layout (one top folder; `lib/robot-host`, `lib/agent`, `robots`), which `crates/update` relies on.
- Installers older than this change don't know `/UPDATE`, but it is the *new* release's installer that runs, so every release from this one on can update the ones before.
- Untested for real: the .app swap on a Mac (no quarantine attribute is set on files the program downloads itself, so Gatekeeper shouldn't object), and the Windows installer's `/UPDATE` run over an existing installation (compiled, not run, to leave the owner's installation alone). The Windows .zip and Linux .tar.gz paths were run end to end against a local release server.
