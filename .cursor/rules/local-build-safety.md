# Local Build Safety — OOM Prevention

This machine has 16 GB RAM, 2 GB swap, Intel i7-4770S (8 cores).
Uncontrolled Rust release builds WILL freeze the entire system.

## Critical Rules

1. **NEVER** run `cargo build --release` directly. The `[profile.release]` uses thin LTO which
   needs ~18 GB during linking and will OOM-kill processes or freeze the desktop.

2. **ALWAYS** use `just install-fork` (or `./scripts/install-codex-fork.sh`) to build. It uses
   the `release-local` profile (no LTO) with automatic memory-safe parallelism (`-j 2`).

3. **NEVER** run cargo build with more than `-j 3` for any release/optimized profile. Each rustc
   process uses 1-3 GB in release mode; 4+ concurrent processes exceeds available RAM.

4. If you must run a custom cargo command with optimization, prefix it with:
   `systemd-run --user --scope -p MemoryMax=12G cargo build ...`

5. After a system freeze/reboot, clear stale locks before building:
   `rm -f codex-rs/target/.package-cache`

## Build Commands Quick Reference

| Task | Command |
|------|---------|
| Build + install fork | `just install-fork` |
| Build with custom parallelism | `CODEX_FORK_JOBS=3 just install-fork` |
| Full LTO release (CI only) | `gh workflow run build-fork.yml -R mikhailsal/codex` |
| Debug build (safe) | `cargo build -p codex-cli` |
| Run tests (safe) | `just test -p codex-tui` |

## The Fork Setup

- `codex-fork` at `~/.local/bin/codex-fork` — our fork (profile: release-local, opt-level=3)
- `codex` at nvm path — official npm package (DO NOT touch)
- Rebuild after changes: `just install-fork` (incremental: ~2-5 min)
