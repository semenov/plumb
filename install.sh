#!/bin/sh
# Installs plumb on macOS or Linux, for this user:
#
#   curl -fsSL https://github.com/semenov/plumb/releases/latest/download/install.sh | sh
#
# It puts plumb in ~/.plumb/bin (PLUMB_HOME to change it) and adds that to
# your shell's PATH (--no-modify-path: it doesn't). Running it again
# upgrades plumb. The rest comes by itself when first needed: wasi-sdk for
# `plumb build --target wasm-server`, wasm-server for `plumb dev`.
#
#   --version vX.Y.Z   a release (the latest by default)
set -eu

REPO=semenov/plumb
VERSION=latest
MODIFY_PATH=1
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION=$2; shift 2 ;;
    --no-modify-path) MODIFY_PATH=""; shift ;;
    *) echo "error: unknown option $1" >&2; exit 2 ;;
  esac
done

say() { echo "→ $*" >&2; }
die() { echo "error: $*" >&2; exit 1; }

case "$(uname -s)" in
  Darwin) OS=macos ;;
  Linux) OS=linux ;;
  *) die "plumb runs on macOS and Linux" ;;
esac
case "$(uname -m)" in
  arm64|aarch64) ARCH=aarch64 ;;
  x86_64|amd64) ARCH=x86_64 ;;
  *) die "no plumb build for $(uname -m)" ;;
esac
command -v curl >/dev/null 2>&1 || die "curl is needed"

HOME_DIR=${PLUMB_HOME:-$HOME/.plumb}
BIN=$HOME_DIR/bin
mkdir -p "$BIN"
if [ "$VERSION" = latest ]; then BASE="https://github.com/$REPO/releases/latest/download"; else BASE="https://github.com/$REPO/releases/download/$VERSION"; fi
ASSET=plumb-$OS-$ARCH.gz
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
say "downloading plumb ($OS, $ARCH)"
curl -fsSL "$BASE/$ASSET" -o "$TMP/$ASSET" || die "can't download $BASE/$ASSET"
curl -fsSL "$BASE/SHA256SUMS" -o "$TMP/SHA256SUMS" || die "can't download the checksums"
WANT=$(grep " $ASSET\$" "$TMP/SHA256SUMS" | cut -d' ' -f1)
if command -v sha256sum >/dev/null 2>&1; then GOT=$(sha256sum "$TMP/$ASSET" | cut -d' ' -f1); else GOT=$(shasum -a 256 "$TMP/$ASSET" | cut -d' ' -f1); fi
[ -n "$WANT" ] && [ "$WANT" = "$GOT" ] || die "the download doesn't match its checksum"
gunzip -c "$TMP/$ASSET" > "$TMP/plumb"
chmod 755 "$TMP/plumb"
mv "$TMP/plumb" "$BIN/plumb"
"$BIN/plumb" help >/dev/null 2>&1 || die "the downloaded plumb doesn't run here"
say "installed $BIN/plumb"

# on the PATH
case ":$PATH:" in
  *":$BIN:"*) ;;
  *)
    LINE="export PATH=\"$BIN:\$PATH\""
    if [ -n "$MODIFY_PATH" ]; then
      case "$(basename "${SHELL:-sh}")" in
        zsh) RC=$HOME/.zshrc ;;
        bash) if [ "$OS" = macos ]; then RC=$HOME/.bash_profile; else RC=$HOME/.bashrc; fi ;;
        *) RC=$HOME/.profile ;;
      esac
      grep -qsF "$LINE" "$RC" || { printf '\n# plumb\n%s\n' "$LINE" >> "$RC"; say "added plumb to the PATH in $RC"; }
      echo "open a new terminal, or run: $LINE"
    else
      echo "add plumb to your PATH: $LINE"
    fi
    ;;
esac

# what native programs (plumb run, plumb test) need
if ! command -v cc >/dev/null 2>&1; then
  if [ "$OS" = macos ]; then
    echo "plumb run and plumb test need a C compiler: xcode-select --install"
  else
    echo "plumb run and plumb test need a C compiler and libraries: sudo apt-get install build-essential libsqlite3-dev libcurl4-openssl-dev zlib1g-dev libssl-dev"
  fi
fi
echo "plumb is ready: plumb help"
