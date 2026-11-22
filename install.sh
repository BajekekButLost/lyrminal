#!/bin/sh
set -eu

REPO="BajekekButLost/lyrminal"
BIN="lyrminal"
INSTALL_DIR="${LYRMINAL_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${LYRMINAL_VERSION:-latest}"
FROM_SOURCE=0
FORCE=0

say() {
    printf '\033[1;32m==>\033[0m %s\n' "$*"
}

warn() {
    printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2
}

die() {
    printf '\033[1;31merror:\033[0m %s\n' "$*" >&2
    exit 1
}

usage() {
    cat <<EOF
Install or update Lyrminal.

Usage: install.sh [options]

Options:
  -v, --version <vX.Y.Z>  Install a specific version (default: latest)
  -d, --dir <path>        Install directory (default: ~/.local/bin)
  -s, --from-source       Build from source with cargo instead of downloading a binary
  -f, --force             Reinstall even if the same version is already installed
  -h, --help              Show this help

Environment:
  LYRMINAL_VERSION        Same as --version
  LYRMINAL_INSTALL_DIR    Same as --dir
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        -v | --version)
            [ $# -ge 2 ] || die "$1 needs a value"
            VERSION="$2"
            shift 2
            ;;
        -d | --dir)
            [ $# -ge 2 ] || die "$1 needs a value"
            INSTALL_DIR="$2"
            shift 2
            ;;
        -s | --from-source)
            FROM_SOURCE=1
            shift
            ;;
        -f | --force)
            FORCE=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            die "unknown option: $1"
            ;;
    esac
done

has() {
    command -v "$1" >/dev/null 2>&1
}

fetch() {
    if has curl; then
        curl -fsSL --retry 3 -o "$2" "$1"
    elif has wget; then
        wget -q -O "$2" "$1"
    else
        die "curl or wget is required"
    fi
}

resolve_tag() {
    if [ "$VERSION" != "latest" ]; then
        case "$VERSION" in
            v*) echo "$VERSION" ;;
            *) echo "v$VERSION" ;;
        esac
        return
    fi

    tag=""
    if fetch "https://api.github.com/repos/$REPO/releases/latest" "$TMP/release.json" 2>/dev/null; then
        tag=$(sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' "$TMP/release.json" | head -n 1)
    fi
    [ -n "$tag" ] || die "could not find the latest release of $REPO"
    echo "$tag"
}

prebuilt_target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os/$arch" in
        Linux/x86_64 | Linux/amd64) echo "x86_64-linux" ;;
        *) echo "" ;;
    esac
}

download_binary() {
    asset="$BIN-$TAG-$1"
    say "Downloading $asset"
    fetch "https://github.com/$REPO/releases/download/$TAG/$asset" "$TMP/$BIN" 2>/dev/null
}

build_from_source() {
    has cargo || die "cargo is required to build from source, install Rust from https://rustup.rs"
    has tar || die "tar is required to build from source"

    say "Downloading the $TAG source"
    fetch "https://github.com/$REPO/archive/refs/tags/$TAG.tar.gz" "$TMP/src.tar.gz" 2>/dev/null ||
        die "could not download the $TAG source"

    mkdir -p "$TMP/src"
    tar -xzf "$TMP/src.tar.gz" -C "$TMP/src" --strip-components=1

    say "Building $BIN $TAG, this can take a few minutes"
    CARGO_TARGET_DIR="$TMP/target" cargo build --release --locked --manifest-path "$TMP/src/Cargo.toml"
    cp "$TMP/target/release/$BIN" "$TMP/$BIN"
}

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

TAG=$(resolve_tag)
WANT="$BIN ${TAG#v}"
DEST="$INSTALL_DIR/$BIN"

if [ "$FORCE" -eq 0 ] && [ -x "$DEST" ] && [ "$("$DEST" --version 2>/dev/null || true)" = "$WANT" ]; then
    say "$WANT is already installed at $DEST"
    exit 0
fi

TARGET=$(prebuilt_target)
if [ "$FROM_SOURCE" -eq 1 ]; then
    build_from_source
elif [ -z "$TARGET" ]; then
    warn "no prebuilt binary for $(uname -s) $(uname -m), building from source"
    build_from_source
elif ! download_binary "$TARGET"; then
    warn "no prebuilt binary for $TAG, building from source"
    build_from_source
fi

chmod 755 "$TMP/$BIN"
GOT=$("$TMP/$BIN" --version 2>/dev/null || true)
[ "$GOT" = "$WANT" ] || die "the downloaded binary reports '$GOT', expected '$WANT'"

mkdir -p "$INSTALL_DIR"
cp "$TMP/$BIN" "$INSTALL_DIR/.$BIN.new"
mv -f "$INSTALL_DIR/.$BIN.new" "$DEST"

say "Installed $WANT to $DEST"

case ":$PATH:" in
    *":$INSTALL_DIR:"*)
        FOUND=$(command -v "$BIN" 2>/dev/null || true)
        if [ -n "$FOUND" ] && [ "$FOUND" != "$DEST" ]; then
            warn "$FOUND comes before $DEST on your PATH, remove it or reorder your PATH"
        fi
        ;;
    *)
        warn "$INSTALL_DIR is not on your PATH, add this to your shell config:"
        printf '\n    export PATH="%s:$PATH"\n\n' "$INSTALL_DIR"
        ;;
esac
