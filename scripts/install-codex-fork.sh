#!/usr/bin/env bash
# Build this fork's CLI with the memory-efficient `release-local` profile and
# install it as `codex-fork` so it does not collide with an official `codex`
# on PATH (e.g. the npm package).
#
# Usage:
#   ./scripts/install-codex-fork.sh
#   just install-fork
#
# Override install location / name:
#   CODEX_FORK_INSTALL_DIR=~/bin CODEX_FORK_NAME=mycodex ./scripts/install-codex-fork.sh
#
# Memory safety:
#   This script auto-detects available RAM and limits cargo parallelism to
#   prevent OOM freezes on machines with <= 16 GB RAM.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CODEX_RS_DIR="${REPO_ROOT}/codex-rs"
INSTALL_DIR="${CODEX_FORK_INSTALL_DIR:-${HOME}/.local/bin}"
INSTALL_NAME="${CODEX_FORK_NAME:-codex-fork}"
DEST="${INSTALL_DIR}/${INSTALL_NAME}"
PROFILE="release-local"

# --- Memory-safe job count calculation ---
# Each rustc process in release mode can use 1-3 GB.
# We reserve 4 GB for the OS/desktop/IDE and divide the rest.
TOTAL_MEM_KB=$(awk '/MemTotal/ {print $2}' /proc/meminfo 2>/dev/null || echo 0)
TOTAL_MEM_GB=$(( TOTAL_MEM_KB / 1024 / 1024 ))
RESERVED_GB=4
AVAILABLE_FOR_BUILD_GB=$(( TOTAL_MEM_GB - RESERVED_GB ))
if (( AVAILABLE_FOR_BUILD_GB < 2 )); then
  AVAILABLE_FOR_BUILD_GB=2
fi

# ~2 GB per job is a conservative estimate for this codebase in release mode.
SAFE_JOBS=$(( AVAILABLE_FOR_BUILD_GB / 2 ))
NPROC=$(nproc 2>/dev/null || echo 4)
if (( SAFE_JOBS > NPROC )); then
  SAFE_JOBS=$NPROC
fi
if (( SAFE_JOBS < 1 )); then
  SAFE_JOBS=1
fi

# Allow user override via CODEX_FORK_JOBS
JOBS="${CODEX_FORK_JOBS:-$SAFE_JOBS}"

mkdir -p "${INSTALL_DIR}"

echo "=== codex-fork install ==="
echo "RAM: ${TOTAL_MEM_GB} GB total, reserving ${RESERVED_GB} GB for system"
echo "Build parallelism: -j ${JOBS} (override with CODEX_FORK_JOBS=N)"
echo "Profile: ${PROFILE} (opt-level=3, no LTO, codegen-units=16)"
echo "Target: ${DEST}"
echo ""

# --- Optional: use systemd memory capping if available ---
MEMORY_CAP_GB=$(( TOTAL_MEM_GB - 3 ))
if (( MEMORY_CAP_GB < 4 )); then
  MEMORY_CAP_GB=4
fi

CARGO_CMD="cargo build --profile ${PROFILE} -p codex-cli --bin codex -j ${JOBS}"

if command -v systemd-run &>/dev/null && systemctl --user is-active -- init.scope &>/dev/null 2>&1; then
  echo "Using systemd memory cap: ${MEMORY_CAP_GB}G (prevents system freeze)"
  echo ""
  BUILD_CMD="systemd-run --user --scope -p MemoryMax=${MEMORY_CAP_GB}G --description=codex-fork-build ${CARGO_CMD}"
else
  echo "systemd-run not available; relying on -j ${JOBS} for memory safety"
  echo ""
  BUILD_CMD="${CARGO_CMD}"
fi

(
  cd "${CODEX_RS_DIR}"
  eval "${BUILD_CMD}"
)

SRC="${CODEX_RS_DIR}/target/${PROFILE}/codex"
if [[ ! -x "${SRC}" ]]; then
  echo "error: expected executable at ${SRC}" >&2
  exit 1
fi

# Copy (not symlink) so a later cargo clean / rebuild does not break the
# installed command until you re-run this script.
install -m 755 "${SRC}" "${DEST}"

echo ""
echo "=== Success ==="
echo "Installed: ${DEST}"
echo "Official codex (unchanged): $(command -v codex 2>/dev/null || echo 'not found')"
echo "Fork command:               $(command -v "${INSTALL_NAME}" 2>/dev/null || echo "${DEST}")"
"${DEST}" --version
