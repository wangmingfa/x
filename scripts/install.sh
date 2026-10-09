#!/usr/bin/env bash
#
# Install `x` by building from source (cargo) and copying the binary into PREFIX/bin.
#
#   curl -fsSL https://raw.githubusercontent.com/wangmingfa/x/main/scripts/install.sh | sh
#
# Environment / flags:
#   PREFIX=DIR        install prefix (default /usr/local)
#   X_VERSION=0.1.0   git ref to check out (tag, branch or sha)
#   X_REPO=URL        git repository (default https://github.com/wangmingfa/x)
#   --prefix=DIR      same as PREFIX
#   -y, --yes         skip the confirmation prompt
set -euo pipefail

X_VERSION="${X_VERSION:-0.1.0}"
X_REPO="${X_REPO:-https://github.com/wangmingfa/x}"
PREFIX="/usr/local"
ASSUME_YES=0

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix=*) PREFIX="${1#*=}" ;;
    -y|--yes) ASSUME_YES=1 ;;
    -h|--help) sed -n '3,13p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
  shift
done

BIN_DIR="$PREFIX/bin"
SRC_DIR="${X_SRC_DIR:-$HOME/.cache/x-src}"

info() { printf '\033[32m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33m==>\033[0m %s\n' "$*"; }

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo (Rust toolchain) is required. Install from https://rustup.rs" >&2
  exit 1
fi

info "installing x $X_VERSION into $BIN_DIR"
if [ "$ASSUME_YES" -ne 1 ]; then
  printf "Continue? [Y/n] "
  read -r answer
  case "$answer" in
    ""|y|Y|yes|YES) ;;
    *) echo "aborted"; exit 1 ;;
  esac
fi

info "cloning/updating $X_REPO @ $X_VERSION"
if [ -d "$SRC_DIR/.git" ]; then
  git -C "$SRC_DIR" fetch --tags --quiet
else
  git clone --quiet "$X_REPO" "$SRC_DIR"
fi
if ! git -C "$SRC_DIR" checkout --quiet "$X_VERSION" 2>/dev/null; then
  git -C "$SRC_DIR" checkout --quiet main
fi

info "building release binary (this can take a few minutes)"
( cd "$SRC_DIR" && cargo build --release --locked -p x-app )

BIN_SRC="$SRC_DIR/target/release/x"
if [ ! -f "$BIN_SRC" ]; then
  echo "build did not produce $BIN_SRC" >&2
  exit 1
fi

info "installing to $BIN_DIR/x"
mkdir -p "$BIN_DIR"
if [ -w "$BIN_DIR" ]; then
  cp "$BIN_SRC" "$BIN_DIR/x"
else
  sudo cp "$BIN_SRC" "$BIN_DIR/x"
fi
chmod 0755 "$BIN_DIR/x"

info "installed: $("$BIN_DIR/x" --version 2>/dev/null || echo x)"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) warn "add $BIN_DIR to your PATH (e.g. export PATH=$BIN_DIR:\$PATH), then open a new terminal" ;;
esac
info "done"
