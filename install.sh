#!/usr/bin/env sh
#
# Install the `canon` compiler.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh -s v0.1.0
#
# Env vars:
#   CANON_INSTALL  override install prefix (default: $HOME/.canon)
#   CANON_VERSION  override version (default: latest release)

set -eu

REPO="almaju/canon"
INSTALL_DIR="${CANON_INSTALL:-$HOME/.canon}"
BIN_DIR="$INSTALL_DIR/bin"

reset="\033[0m"
red="\033[31m"
green="\033[32m"
yellow="\033[33m"
bold="\033[1m"

info() { printf "%b%s%b\n" "$bold" "$1" "$reset"; }
ok()   { printf "%b%s%b\n" "$green" "$1" "$reset"; }
warn() { printf "%b%s%b\n" "$yellow" "$1" "$reset" >&2; }
die()  { printf "%b%s%b\n" "$red" "$1" "$reset" >&2; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || die "error: \`$1\` is required but was not found on PATH"
}

need uname
need tar
need mkdir
need rm
need mv

# `curl` or `wget` — we only need one
fetch() {
    url="$1"
    out="$2"
    if command -v curl >/dev/null 2>&1; then
        curl --fail --location --progress-bar --output "$out" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget --quiet --show-progress --output-document="$out" "$url"
    else
        die "error: neither \`curl\` nor \`wget\` is available"
    fi
}

# Detect OS + arch
os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
    Darwin) os_target="apple-darwin" ;;
    Linux)  os_target="unknown-linux-gnu" ;;
    *) die "error: unsupported OS \`$os\` (canon provides binaries for macOS and Linux)" ;;
esac

case "$arch" in
    arm64|aarch64) arch_target="aarch64" ;;
    x86_64|amd64)  arch_target="x86_64" ;;
    *) die "error: unsupported architecture \`$arch\`" ;;
esac

target="${arch_target}-${os_target}"

# Resolve version
version="${CANON_VERSION:-${1:-latest}}"

if [ "$version" = "latest" ]; then
    info "Resolving latest release from github.com/${REPO}…"
    redirect_url="https://github.com/${REPO}/releases/latest"
    if command -v curl >/dev/null 2>&1; then
        resolved="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$redirect_url" 2>/dev/null || true)"
    else
        resolved="$(wget --max-redirect=10 --server-response --spider "$redirect_url" 2>&1 | awk '/Location: /{u=$2} END{print u}')"
    fi
    # A repo with a published release redirects to …/releases/tag/<tag>.
    # A repo with NO releases redirects to the …/releases listing page, so
    # a naive "${resolved##*/}" would yield the literal word "releases" and
    # build a bogus "vreleases" tag. Require the tag form explicitly.
    case "$resolved" in
        */releases/tag/*) version="${resolved##*/}" ;;
        *) version="" ;;
    esac
    [ -n "$version" ] || die "error: no published release found for ${REPO}.
       The repository may not have cut a release yet — see
       https://github.com/${REPO}/releases
       Once a release exists, re-run this installer, or pin a version:
       CANON_VERSION=vX.Y.Z curl -fsSL https://raw.githubusercontent.com/${REPO}/main/install.sh | sh"
fi

case "$version" in
    v*) ;;
    *) version="v${version}" ;;
esac

archive="canon-${version}-${target}.tar.gz"
url="https://github.com/${REPO}/releases/download/${version}/${archive}"
sha_url="${url}.sha256"

info "Installing canon ${version} for ${target}…"
info "Source:  ${url}"
info "Target:  ${BIN_DIR}/canon"

tmpdir="$(mktemp -d 2>/dev/null || mktemp -d -t canon-install)"
trap 'rm -rf "$tmpdir"' EXIT INT TERM

fetch "$url" "$tmpdir/$archive"

# SHA256 verification (best-effort — skip silently if no checker is available)
if command -v shasum >/dev/null 2>&1 || command -v sha256sum >/dev/null 2>&1; then
    if fetch "$sha_url" "$tmpdir/$archive.sha256" 2>/dev/null; then
        expected="$(cat "$tmpdir/$archive.sha256" | tr -d '[:space:]')"
        if command -v shasum >/dev/null 2>&1; then
            actual="$(shasum -a 256 "$tmpdir/$archive" | awk '{print $1}')"
        else
            actual="$(sha256sum "$tmpdir/$archive" | awk '{print $1}')"
        fi
        if [ "$expected" != "$actual" ]; then
            die "error: checksum mismatch (expected $expected, got $actual)"
        fi
        ok "Checksum verified."
    else
        warn "warning: could not download checksum file, skipping verification"
    fi
fi

mkdir -p "$BIN_DIR"
tar -xzf "$tmpdir/$archive" -C "$tmpdir"

extracted="$tmpdir/canon-${version}-${target}"
[ -f "$extracted/canon" ] || die "error: archive did not contain expected \`canon\` binary"

mv "$extracted/canon" "$BIN_DIR/canon"
chmod +x "$BIN_DIR/canon"

ok ""
ok "  ✓ Installed canon ${version} to ${BIN_DIR}/canon"
ok ""

# Detect shell rc file and offer PATH instructions
case "${SHELL:-}" in
    */zsh)  shell_rc="$HOME/.zshrc"  ; shell_name="zsh"  ;;
    */bash) shell_rc="$HOME/.bashrc" ; shell_name="bash" ;;
    */fish) shell_rc="$HOME/.config/fish/config.fish" ; shell_name="fish" ;;
    *)      shell_rc=""              ; shell_name=""    ;;
esac

case ":${PATH:-}:" in
    *":$BIN_DIR:"*)
        info "${BIN_DIR} is already on your PATH. Try: canon help"
        ;;
    *)
        info "Add ${BIN_DIR} to your PATH:"
        echo
        if [ "$shell_name" = "fish" ]; then
            printf "    fish_add_path %s\n" "$BIN_DIR"
        else
            printf "    export PATH=\"%s:\$PATH\"\n" "$BIN_DIR"
        fi
        if [ -n "$shell_rc" ]; then
            echo
            printf "Or append to %s and restart your shell.\n" "$shell_rc"
        fi
        ;;
esac

echo
info "Note: \`canon run\` and \`canon build\` require \`rustc\` to be installed."
info "      Install Rust from https://rustup.rs if you don't have it."
