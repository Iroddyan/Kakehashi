# Phase 0 Spec: Full-Coverage ADB Frontend

Draft v0.1 · 25 Oct 2026 · Name: Kakehashi (架け橋) · For review by Iroddyan, Claude, Gemini and Codex

## 1. Goal and non-goals

**Goal.** One desktop app that covers every host-side command in `adb help` (adb 1.0.41, platform-tools 37.0.0). The headline feature is a file manager that does all its work through adb. Beyond that: dedicated UIs for the most-used shell tools, and a raw command console for everything else.

**Non-goals for now.** fastboot (a separate tool), reimplementing scrcpy-style screen mirroring, and Android-build-only workflows beyond exposing their commands.

**Positioning.** This is not the first adb-based file manager (aafm, adbtuifm, QtADB and AYA all exist). Our differentiators are speed, reliability, full command coverage, and shipping on both Linux and Windows. Do not market it as a first.

**Supported Android versions.** Android 9 (API 28) and newer. Test devices to start: a Huawei P Smart on Android 9 and a Galaxy S22+ on Android 16, plus emulator images for the versions in between.

## 2. Definition of done (v1.0)

- Every Tier 1 and Tier 2 command in section 3 is implemented in the core, covered by tests, and reachable from the UI.
- Every Tier 3 command is reachable through the command console, with its flags documented.
- A written list of supported shell tools (`pm`, `am`, `dumpsys`, and so on) with the Android versions each was verified on.
- Linux UI shipped. The Windows UI follows (section 6).

## 3. Capability inventory

Source: the `adb help` output supplied by Iroddyan. Tiers set build order, not importance.

| Command | Tier | Notes |
|---|---|---|
| `devices [-l]`, `get-state`, `get-serialno`, `get-devpath`, `wait-for-*` | 1 | Device tracking should use the server's track feature, not polling |
| `connect`, `disconnect`, `pair`, `mdns check/services` | 1 | Wireless debugging; pairing needs a code |
| `push`, `pull` | 1 | Headline feature. Flags: `-z/-Z` compression, `--sync`, `-a`, `-n`, `-q` |
| `shell` (command and interactive) | 1 | Flags `-e -n -T -t -x`; interactive mode needs a pty |
| `install`, `install-multiple`, `install-multi-package`, `uninstall` | 1 | Many flags; fastdeploy options are advanced |
| `logcat` | 1 | Streaming, filtering, saving |
| `reboot` (system, bootloader, recovery, sideload, sideload-auto-reboot) | 1 | Always confirm first |
| `start-server`, `kill-server`, `reconnect` (host, device, offline) | 1 | Never kill a server we did not start without asking |
| `forward`, `reverse` (list, add, remove, remove-all) | 2 | Many socket types |
| `bugreport` | 2 | Long-running; needs progress |
| `jdwp` | 2 | Lists pids |
| `root`, `unroot`, `remount [-R]`, `usb`, `tcpip` | 2 | Restarts adbd; warn the user |
| `sideload` | 2 | Recovery mode only |
| `attach`, `detach` | 2 | USB only |
| Global options `-d -e -s -t -H -P -L -a` | 2 | `-a` exposes the server on all interfaces; show a security warning |
| `sync` | 3 | Needs a local Android build tree (`$ANDROID_PRODUCT_OUT`) |
| `emu` | 3 | Emulator console |
| `keygen`, `disable-verity`, `enable-verity`, `--one-device` | 3 | Userdebug builds and advanced use |
| Environment variables (`ADB_TRACE`, `ADB_VENDOR_KEYS`, `ANDROID_SERIAL`, `ADB_MDNS_AUTO_CONNECT`, and others) | 3 | Advanced settings page |

**Gap that matters for the file manager.** `adb help` has no `ls`, `cp`, `mv`, `rm` or `mkdir`. Only `push` and `pull` are file commands. Everything else goes through `adb shell` or the sync protocol (section 4).

## 4. Architecture

**Layers.** A Rust core crate with no UI code and no OS-specific code outside a single `platform` module. One UI per platform sits on top. UIs contain no adb logic, parsing or state handling.

**Transport (hybrid).** Talk to the adb server directly on port 5037 for device tracking, shell, logcat streaming, file sync (LIST, STAT, SEND, RECV) and forwarding. Spawn the `adb` binary for commands where reimplementing the protocol is not worth it (likely `pair`, `bugreport`, advanced `install` flags, `sideload`). Each feature has one primary path plus a fallback for devices that lack a capability, decided in the M1 table with a source link per row. The AOSP adb docs (OVERVIEW, SERVICES, SYNC) must be checked before each decision. Gemini's audit is the starting point, but it contained errors (shell,v2 packet IDs, compression algorithms), so no row is trusted until checked against the AOSP source.

**Server lifecycle.** Use a running server if there is one, otherwise start one. Other tools, such as Android Studio, share the same server, so never kill one we did not start unless the user explicitly asks.

**Core API.** Async (tokio). Typed commands return typed results or event streams. Typed errors. Cancellation and progress events on every long operation. Device selection by serial or transport id. The public API must be FFI-friendly: owned data, no generics on the public surface, streams exposed as callbacks or channels. This is what keeps the Windows UI possible later.

**Device model.** Serial, transport id, state (device, offline, unauthorized, recovery, sideload, bootloader), model and product, and transport type (USB or TCP).

**Capability negotiation.** Each device reports feature strings (shell protocol v2, the sync v2 variants, `abb` and others). The core reads them on connect and picks the code path from them, with a fallback for each: `shell,v2:` to `shell:`, sync v2 to sync v1, `abb` to `pm`. The M1 transport table records the feature or API level each path needs.

**Server management.** On connect, check the server's version (`host:version`). If it differs from what the core expects, do not silently kill it, because other tools such as Android Studio may own it. Ask the user, then use `host:kill` and restart. We need an `adb` binary regardless, since only the binary starts the server, so bundling it is a required decision.

**Protocol facts checked against AOSP source.** The rest of Gemini's audit stays unverified until it has per-claim sources.

- `shell,v2:` needs API 24+. Packets are a 1-byte id and a 4-byte length: 0 stdin, 1 stdout, 2 stderr, 3 exit code, 4 close stdin, 5 window-size change.
- `abb:` and `abb_exec:` need API 30+.
- Sync data chunks are at most 64 KiB. Sync v2 compression is brotli, LZ4 or Zstd (not zlib), each gated by its own feature flag.
- Sync v2 STAT carries mode, uid, gid, link count, 64-bit size and three timestamps. Sync v1 STAT carries only mode, 32-bit size and mtime.
- The server also supports `host:track-devices` with protobuf variants, `host:version`, `host:kill` and a server-status query.

## 5. File manager rules

- A `RemoteFs` trait: list, stat, read, write, mkdir, rename, delete, copy, move, chmod. Two backends: `SyncFs` (list, stat, push, pull over the sync protocol) and `ShellFs` (copy, move, delete, mkdir, chmod through shell commands).
- Copy and move within a device run on the device (`cp`, `mv`), never through the host. Device-to-device transfers go pull then push.
- Prefer `stat` with a format string over parsing `ls -l`. Where the device supports sync v2, use sync LIST and STAT instead: they carry mode, uid, gid, 64-bit size and timestamps. Sync v1 has a 32-bit size field, so older devices and large files use the shell stat fallback. Output varies by Android version, so verify on every supported version and keep real outputs as test fixtures.
- Remote paths are always POSIX strings, never the host's path type. Handle spaces, quotes, newlines and non-ASCII names.
- Never build a shell command by concatenating unescaped input. All arguments go through one quoting function with its own tests.
- Destructive operations (delete, move-overwrite, anything touching system partitions) need a preview with counts and a confirmation. No silent overwrites. Android has no trash, so deletes are permanent; say so in the dialog.
- A transfer queue with a concurrency limit, cancel, and per-file progress. `push` has no resume, so a failed transfer restarts the file. Compression (brotli, LZ4 or Zstd, via sync v2) is a setting, offered only when the device reports the matching feature.

## 6. UI and platform plan

- **Linux UI (first):** Rust with gtk4-rs and libadwaita. Embedded terminal uses VTE.
- **Windows UI (later):** toolkit decision deferred to before M5 (WinUI 3 in C# is the leading candidate). It calls the same core through bindings. The terminal component is platform-specific.
- **Parity rule:** a feature counts as done on a platform only when its core tests pass and its UI checklist is ticked. Keep a parity tracker table in `docs/parity.md`.
- **Windows safety net:** CI builds and tests the core on Windows from M0. There is no Windows device testing until the Windows UI phase, including USB drivers.

## 7. Testing

- **Parser tests:** fixtures captured from real devices across Android versions in `tests/fixtures/`.
- **Fake adb server:** speaks the server protocol with scripted scenarios (device appears or disappears, unauthorized, offline, large listings, slow transfer, disconnect mid-transfer). CI runs entirely on it, with no device attached.
- **Integration tests:** real devices (Android 9 and Android 16 to start) plus emulator images for the versions in between; the emulator uses the same protocol. The Android 9 device will most likely exercise the fallback paths, so check the features each device reports. Destructive tests only run inside a dedicated test directory on the device, and only on the Huawei P Smart (the designated experimental device) or an emulator, never on the Galaxy S22+, which is a daily-use phone and stays read-only.
- **Equivalence tests:** where safe, run the `adb` CLI and the core on the same operation and compare results.
- **CI:** Linux and Windows build and test on every commit (GitHub Actions).

## 8. Repo layout and workflow

```
core/            Rust core crate
ui-linux/        GTK4 / libadwaita app
ui-windows/      added later
tests/fixtures/  captured device outputs
docs/decisions/  one short ADR per decision (NNN-title.md)
SPEC.md  ARCHITECTURE.md  AGENTS.md  GEMINI.md
```

**Roles.** Iroddyan and Codex do the heavy implementation. Claude reviews, plans and does light coding. Gemini and Iroddyan do research and light planning. Only Iroddyan merges.

**Rules.**

- Codex works from a milestone brief and reads `AGENTS.md`. Gemini reads `GEMINI.md`. Claude gets repo access by URL or pasted diffs.
- Small PRs, each referencing a section of this spec.
- Research claims need a source link before they become decisions. Unverified claims are marked as such.
- Every architecture decision gets an ADR. If an agent disagrees with this spec, it raises it in review and does not silently deviate.
- Loop per milestone: plan with acceptance criteria, code, test (fake server, then a real device), debug, review, merge, update the spec.

## 9. Milestones

- **M0.** Repo, CI on Linux and Windows, fake server skeleton, this inventory as a tracked checklist, first fixtures collected.
- **M1.** Core: connect to the server, device tracking, shell execution, server version check, per-device feature negotiation. A small CLI harness exercises the core. The per-feature transport table is filled in, with a fallback, minimum API and source link per row, and verified against the AOSP source.
- **M2.** Read-only file layer (list, stat, pull). Linux window with device list and a read-only file browser.
- **M3.** File writes: push, mkdir, rename, copy, move, delete, with the safety rules and the transfer queue. **This is v0.1.**
- **M4.** Remaining Tier 1: install and uninstall, logcat, reboot, wireless pair and connect.
- **M5.** Tier 2. Windows UI toolkit decision.
- **M6.** Interactive shell terminal.
- **M7.** Tier 3, advanced settings, polish. Then the Windows UI.

Target v0.1 date (end of M3): 30 January 2027. Set before starting M1.

## 10. Open decisions

1. Confirm the name Kakehashi is free on crates.io and Flathub (the GitHub repo already exists at Iroddyan/Kakehashi).
2. Which emulator images join the test matrix beside the two phones (chosen in M0).
3. Raw protocol or CLI for each command (decided in M1, with sources).
4. Windows UI toolkit (before M5).
5. Bundle platform-tools with the app or require it installed (the Windows build probably needs to bundle it; check the license terms).
6. Project license.
7. Whether to launch scrcpy externally later, or never.
8. Whether fastboot becomes a separate module after v1.0.
