#!/usr/bin/env bash
#
# Install `x` by downloading the latest (or a pinned) release binary from
# GitHub Releases. No Rust toolchain required.
#
#   curl -fsSL https://raw.githubusercontent.com/wangmingfa/x/main/scripts/install-release.sh | sh
#
# Environment / flags:
#   PREFIX=DIR        install prefix (default /usr/local)
#   X_VERSION=TAG     release tag to install (default: latest release)
#   X_REPO=OWNER/NAME GitHub repository (default wangmingfa/x)
#   --prefix=DIR      same as PREFIX
#   X_VERSION=v0.1.2  same as the environment variable
#   -y, --yes         skip the confirmation prompt
#   -h, --help        this text
#
# The macOS asset is a universal binary (Apple Silicon and Intel in one
# file), so Darwin needs no architecture check; Linux ships one asset per
# architecture, chosen from `uname -m`.
set -euo pipefail

X_VERSION="${X_VERSION:-}"
X_REPO="${X_REPO:-wangmingfa/x}"
PREFIX="/usr/local"
ASSUME_YES=0

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix=*) PREFIX="${1#*=}" ;;
    --version=*) X_VERSION="${1#*=}" ;;
    -y|--yes) ASSUME_YES=1 ;;
    -h|--help) sed -n '3,20p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
  shift
done

BIN_DIR="$PREFIX/bin"

info() { printf '\033[32m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33m==>\033[0m %s\n' "$*"; }
fail() { printf '\033[31m==>\033[0m %s\n' "$*" >&2; exit 1; }

# --- pick the asset name for this machine ------------------------------------
case "$(uname -s)" in
  Darwin) ASSET="macOS-universal" ;;
  Linux)
    case "$(uname -m)" in
      x86_64)  ASSET="Linux-x86_64" ;;
      aarch64|arm64) ASSET="Linux-aarch64" ;;
      *) fail "unsupported architecture: $(uname -m)" ;;
    esac
    ;;
  *) fail "unsupported platform: $(uname -s) (Windows uses scripts/install.ps1 -FromRelease)" ;;
esac

# --- resolve which release to download ---------------------------------------
# /releases/latest only answers for stable releases - a repo whose newest
# release is marked prerelease gets a 404. List releases and take the first
# non-draft entry: GitHub returns them newest-first, prereleases included.
if [ -z "$X_VERSION" ]; then
  info "resolving the latest release of $X_REPO"
  X_VERSION=$(
    curl -fsSL "https://api.github.com/repos/$X_REPO/releases" |
      sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1
  )
  [ -n "$X_VERSION" ] || fail "could not determine the latest release (check your network or set X_VERSION=v0.1.2)"
fi

URL="https://github.com/$X_REPO/releases/download/$X_VERSION/x-${X_VERSION}-${ASSET}.tar.gz"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

info "installing x $X_VERSION into $BIN_DIR"
if [ "$ASSUME_YES" -ne 1 ]; then
  printf "Continue? [Y/n] "
  read -r answer
  case "$answer" in
    ""|y|Y|yes|YES) ;;
    *) echo "aborted"; exit 1 ;;
  esac
fi

info "downloading $URL"
curl -fSL --retry 3 -o "$TMP_DIR/x.tar.gz" "$URL" || fail "download failed"
[ -s "$TMP_DIR/x.tar.gz" ] || fail "downloaded file is empty"

mkdir -p "$TMP_DIR/stage"
tar -xzf "$TMP_DIR/x.tar.gz" -C "$TMP_DIR/stage"
[ -f "$TMP_DIR/stage/x" ] || fail "archive did not contain the x binary"

info "installing to $BIN_DIR/x"
mkdir -p "$BIN_DIR"
if [ -w "$BIN_DIR" ]; then
  cp "$TMP_DIR/stage/x" "$BIN_DIR/x"
else
  sudo cp "$TMP_DIR/stage/x" "$BIN_DIR/x"
fi
chmod 0755 "$BIN_DIR/x"

info "installed: $("$BIN_DIR/x" --version 2>/dev/null || echo x)"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) warn "add $BIN_DIR to your PATH (e.g. export PATH=$BIN_DIR:\$PATH)" ;;
esac
info "done"
