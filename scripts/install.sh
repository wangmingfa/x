#!/usr/bin/env bash
#
# Install `x` by building from source (cargo) and copying the binary into PREFIX/bin.
#
#   curl -fsSL https://raw.githubusercontent.com/wangmingfa/x/main/scripts/install.sh | sh
#
# Environment / flags:
#   PREFIX=DIR        install prefix (default ~/.local — installs to
#                     ~/.local/bin, which needs no sudo; pass --prefix=/usr/local
#                     to keep the old behaviour)
#   X_VERSION=0.1.0   git ref to check out (tag, branch or sha)
#   X_REPO=URL        git repository (default https://github.com/wangmingfa/x)
#   --prefix=DIR      same as PREFIX
#   -y, --yes         skip the confirmation prompt
# POSIX-only: `pipefail` is bash-only and breaks under dash (the default /bin/sh
# on Debian/Ubuntu when invoked as `curl ... | sh`).
set -eu

X_VERSION="${X_VERSION:-0.1.0}"
X_REPO="${X_REPO:-https://github.com/wangmingfa/x}"
# ~/.local/bin keeps the install user-owned: `x upgrade` later replaces the
# binary in place without sudo, and the sudo fallback below still covers an
# explicit --prefix=/usr/local.
PREFIX="${PREFIX:-$HOME/.local}"
ASSUME_YES=0

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix=*) PREFIX="${1#*=}" ;;
    -y|--yes) ASSUME_YES=1 ;;
    -h|--help) sed -n '3,15p' "$0"; exit 0 ;;
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
  # Read from the terminal, not stdin: under `curl | sh` stdin is the pipe
  # that feeds the script itself, and a plain `read` would swallow the next
  # script line instead of the user's answer. From a saved file stdin is the
  # terminal anyway, so nothing changes there.
  read -r answer </dev/tty
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
# A missing ref is an error, not a silent fallback to `main`: X_VERSION always
# names a requested ref (even its default is a tag), and the user must not
# quietly get some other code installed under that name.
if ! git -C "$SRC_DIR" checkout --quiet "$X_VERSION" 2>/dev/null; then
  echo "git ref '$X_VERSION' not found in $X_REPO (tag, branch or sha?)" >&2
  exit 1
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
  *)
    warn "$BIN_DIR is not in your PATH. Add it and open a new terminal, e.g.:"
    warn "  zsh:  echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.zshrc"
    warn "  bash: echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.bashrc"
    ;;
esac
# An old copy earlier in PATH would shadow the fresh install and make the
# upgrade command replace the wrong file later.
existing=$(command -v x 2>/dev/null || true)
if [ -n "$existing" ] && [ "$existing" != "$BIN_DIR/x" ]; then
  warn "an x already resolves to $existing; make sure $BIN_DIR comes first in PATH, or remove the old copy"
fi
info "done"
