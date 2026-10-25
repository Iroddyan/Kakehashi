# M0 Brief: Foundations

Save as `docs/briefs/M0-foundations.md`. Read `AGENTS.md` and `SPEC.md` first.

## Goal

A Rust workspace that builds and tests on Linux and Windows, a fake adb server that tests can talk to without a
device, and the capability checklist that tracks every command through to v1.0. No UI, no real device access.

## Tasks

Each task has its own acceptance criteria. Suggested PR order is in the last section.

### T1. Workspace

- Cargo workspace with three crates: `core` (`kakehashi-core`), `cli` (`kakehashi-cli`, a small harness), `fake-adb` (`kakehashi-fake-adb`, dev-only).
- Directories created now: `docs/briefs/`, `docs/decisions/`, `tests/fixtures/`. No `ui-linux/` yet.
- Done when: `cargo build --workspace` passes on Linux.

### T2. CI

- GitHub Actions workflow running on `ubuntu-latest` and `windows-latest`: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Done when: a trivial test passes on both runners.

### T3. Smart-socket framing (in `core`)

- Encode a request: four ASCII hex digits giving the payload length, then the payload.
- Read a response status (`OKAY` or `FAIL`), plus the length-prefixed payload variant used by most host services. `FAIL` carries a message.
- Unit tests: normal request and response, `FAIL` with message, truncated input, a length prefix larger than the data, non-hex length.
- Verify the exact wire format against the AOSP adb docs (overview and services) and cite them in comments.
- Done when: all cases above have tests, and no function panics on malformed input.

### T4. Fake adb server (`fake-adb`)

- Async TCP listener on `127.0.0.1` with an ephemeral port, exposed so tests can connect.
- Scripted responses for `host:version`, `host:devices` and `host:devices-l`. Tests can set the device list. Take exact response formats from the AOSP SERVICES doc; mark anything you cannot confirm `UNVERIFIED`.
- Done when: a test starts the server, sets two devices, and reads them back through a raw socket.

### T5. Core client

- `AdbServerClient` with `connect(addr)`, `version()` and `devices()`, typed results and typed errors (connection refused, `FAIL`, malformed reply).
- Done when: integration tests pass against the fake server, including the server being unreachable.

### T6. CLI harness

- `kakehashi-cli version` and `kakehashi-cli devices`, with a `--server host:port` option defaulting to `127.0.0.1:5037`.
- Done when: both commands work against the fake server in a test.

### T7. Capability checklist

- `docs/capabilities.md`: one row per command from `SPEC.md` section 3, with columns for Command, Tier, Transport (direct, spawn, or TBD), Minimum API or feature, Fallback, Source, Status.
- Pre-fill only what `SPEC.md` already states. Leave the rest `TBD`. Do not invent protocol details.
- Done when: every command in the spec table has a row.

### T8. Fixtures and ADRs

- `docs/fixtures.md`: how to capture real device output using read-only commands only, how to scrub personal file names, and the naming convention `<api>-<model>-<command>.txt`.
- `docs/decisions/000-template.md` and `001-core-architecture.md` (Rust core, hybrid transport, Linux first), summarizing decisions already in `SPEC.md`.
- `ARCHITECTURE.md` stub: the workspace layout and one short paragraph per crate.
- `GEMINI.md`: a short file pointing Gemini to `AGENTS.md` and `SPEC.md`.
- Done when: all of these files exist and ADR 001 links to the spec sections it summarizes.

## Out of scope for M0

UI of any kind, spawning the `adb` binary, shell execution, the sync protocol, file operations, and any access to a real device.

## Definition of done

- CI is green on Linux and Windows.
- `fmt` and `clippy` are clean.
- Framing and fake-server tests pass.
- The capability checklist is complete.
- Every PR lists its `UNVERIFIED` items.

## Suggested PR order

1. T1 and T2
2. T3
3. T4 and T5
4. T6
5. T7 and T8
