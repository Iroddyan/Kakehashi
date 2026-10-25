# AGENTS.md: Kakehashi (架け橋)

Instructions for AI coding agents (Codex, Gemini CLI, others) working in this repo.
Read `SPEC.md` first. Then read the current milestone brief in `docs/briefs/`.

## What this is

Kakehashi is a desktop ADB frontend. A Rust core talks to the adb server; each platform has its own thin UI.
Linux comes first (GTK4/libadwaita via gtk4-rs), Windows later. The headline feature is a file manager that
does all its work through adb. The goal is to cover every host-side command in `adb help`
(tiers are in `SPEC.md` section 3).

## Source of truth

- `SPEC.md` is the plan. `ARCHITECTURE.md` describes the code as actually built.
- `docs/decisions/` holds one short ADR per decision.
- If code and spec disagree, stop and flag it in the PR. Do not deviate silently.
- Do only what the current milestone brief asks. No extra features, no drive-by refactors.

## Hard rules

1. No UI code in `core/`. No adb logic, parsing or state handling in any UI crate.
2. Never build a shell command by concatenating unescaped input. Use the core's single quoting function, which has its own tests.
3. Remote (device) paths are POSIX strings, never `std::path::Path`.
4. Never guess protocol behavior. Cite the AOSP source (file and branch or commit) in a comment or ADR.
   If you cannot verify a claim, mark it `// UNVERIFIED:` and list it in the PR description.
5. Minimum supported Android is 9 (API 28). Use per-device feature negotiation, and give every capability a fallback.
6. Destructive operations (delete, move-overwrite, overwrite) need a preview and confirmation in the UI.
   Automated tests for them run only against the fake adb server, an emulator, or the designated experimental
   device, and only inside a dedicated test directory. Never run them against the owner's daily-use phone.
7. Do not kill an adb server this app did not start without explicit user confirmation.
8. No `unsafe` without an ADR. Never panic on device input: return typed errors.
9. Nothing OS-specific outside `core/src/platform/`. CI builds and tests on Linux and Windows.
10. Do not commit device fixtures that contain personal file names. Scrub first (`docs/fixtures.md`).

## Stack and commands

Rust stable, tokio, thiserror. Before opening a PR, all of these must pass:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Workflow

- Small PRs. Each one references a section of `SPEC.md` and starts its title with the milestone (`M0: ...`).
- PR description: what changed, how it was tested, and every `UNVERIFIED` claim.
- Only the repo owner merges.
- Research claims need a source link before they become decisions.
- When unsure, stop and ask in the PR. Do not invent adb behavior.

## Gemini
You have access to gemini-api through the shell.
Use it to call Gemini models, generate or edit media, analyze files, or launch managed agents.
1. Inspect --help or --usage to choose a command, and --schema when building a --body JSON payload.
2. Preview API calls with --dry-run (or --dry-run --output-format json).
3. For discrete JSON output or --jq filtering on streaming/background commands (generate, agent run, agent status), pass --stream=false --output-format json.
4. Capture returned file paths (from media commands) and interaction IDs (from agent/model runs) to continue multi-step tasks.
