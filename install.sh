#!/usr/bin/env bash
# Installs rnm on macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/christophersarmiento/rnm/main/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/christophersarmiento/rnm/main/install.sh | bash -s -- --skip-shell
#
# Downloads a release archive from GitHub, checks it against the release's
# SHA256SUMS, puts the binary in ~/.local/share/rnm/bin, and adds rnm to
# your shell's startup file. Run with --help for the options.
#
# Works with bash 3.2 (macOS's /bin/bash) and later.

set -euo pipefail

REPO="christophersarmiento/rnm"
MARKER="# rnm"

version="latest"
install_dir="${RNM_INSTALL_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/rnm/bin}"
base_url="${RNM_INSTALL_BASE_URL:-}"
skip_shell=false
tmp=""
use_on_cd=false

usage() {
  cat <<'EOF'
Install rnm, the fast nvm-compatible Node.js version manager.

Usage: install.sh [options]

Options:
  --version <tag>      Install this release, for example v0.1.0 (default: latest)
  --nightly            Install the newest nightly build of the dev branch
  --install-dir <dir>  Put the rnm binary here (default: ~/.local/share/rnm/bin)
  --skip-shell         Do not change any shell startup file
  --use-on-cd          Set up rnm to switch versions on every cd
  -h, --help           Show this help

Environment:
  RNM_INSTALL_DIR       Same as --install-dir
  RNM_INSTALL_BASE_URL  Download from this URL or local directory instead of
                        GitHub; it must hold the archive and SHA256SUMS
EOF
}

say() { printf '%s\n' "$*"; }
fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      [ $# -ge 2 ] || fail "--version needs a value, for example v0.1.0"
      version="$2"
      shift 2
      ;;
    --version=*)
      version="${1#*=}"
      shift
      ;;
    --nightly)
      version="nightly"
      shift
      ;;
    --install-dir)
      [ $# -ge 2 ] || fail "--install-dir needs a directory"
      install_dir="$2"
      shift 2
      ;;
    --install-dir=*)
      install_dir="${1#*=}"
      shift
      ;;
    --skip-shell)
      skip_shell=true
      shift
      ;;
    --use-on-cd)
      use_on_cd=true
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *) fail "unknown option '$1' (see --help)" ;;
  esac
done

# The Rust target triple of the release archive for this machine.
detect_target() {
  local os arch
  case "$(uname -s)" in
    Darwin) os="apple-darwin" ;;
    Linux) os="unknown-linux-musl" ;;
    MINGW* | MSYS* | CYGWIN*)
      fail "on Windows, run install.ps1 in PowerShell instead:
  irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex"
      ;;
    *) fail "rnm has no prebuilt binary for $(uname -s); build it with cargo install --git https://github.com/$REPO" ;;
  esac
  case "$(uname -m)" in
    x86_64 | amd64) arch="x86_64" ;;
    arm64 | aarch64 | armv8*) arch="aarch64" ;;
    *) fail "rnm has no prebuilt binary for $(uname -m); build it with cargo install --git https://github.com/$REPO" ;;
  esac
  # An x64 shell under Rosetta 2 still runs on Apple Silicon.
  if [ "$os" = "apple-darwin" ] && [ "$arch" = "x86_64" ] &&
    [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
    arch="aarch64"
  fi
  printf '%s-%s\n' "$arch" "$os"
}

release_url() {
  if [ -n "$base_url" ]; then
    printf '%s\n' "${base_url%/}"
  elif [ "$version" = "latest" ]; then
    printf 'https://github.com/%s/releases/latest/download\n' "$REPO"
  else
    printf 'https://github.com/%s/releases/download/%s\n' "$REPO" "$version"
  fi
}

# Copies $1 (a URL, or a path when the base is a local directory) to $2.
fetch() {
  case "$1" in
    http://* | https://* | file://*)
      if command -v curl >/dev/null 2>&1; then
        curl --proto '=https,http,file' -fsSL "$1" -o "$2"
      elif command -v wget >/dev/null 2>&1; then
        wget -q "$1" -O "$2"
      else
        fail "curl or wget is needed to download rnm"
      fi
      ;;
    *) cp "$1" "$2" ;;
  esac
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    fail "sha256sum or shasum is needed to verify the download"
  fi
}

# $1 quoted for POSIX shells and fish: inside single quotes, with each
# single quote written as '\''. fish reads that the same way.
quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

install_binary() {
  local target archive url expected actual
  target="$(detect_target)"
  archive="rnm-$target.tar.gz"
  url="$(release_url)"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT

  say "Downloading $url/$archive"
  fetch "$url/$archive" "$tmp/$archive" ||
    fail "download failed; check that release '$version' exists at https://github.com/$REPO/releases"
  fetch "$url/SHA256SUMS" "$tmp/SHA256SUMS" || fail "cannot download SHA256SUMS from $url"

  expected="$(awk -v name="$archive" '$2 == name { print $1 }' "$tmp/SHA256SUMS")"
  [ -n "$expected" ] || fail "SHA256SUMS lists no checksum for $archive"
  actual="$(sha256 "$tmp/$archive")"
  [ "$expected" = "$actual" ] ||
    fail "checksum mismatch for $archive: expected $expected, got $actual; the download is corrupt or was tampered with"
  say "Checksum verified."

  tar -xzf "$tmp/$archive" -C "$tmp"
  mkdir -p "$install_dir"
  # Replaces a running rnm safely: the new file gets a new inode.
  cp "$tmp/rnm" "$install_dir/rnm.new"
  chmod 755 "$install_dir/rnm.new"
  mv -f "$install_dir/rnm.new" "$install_dir/rnm"
  say "Installed $("$install_dir/rnm" --version) to $install_dir/rnm"
}

# Appends $2 to the file $1 unless it already sets up rnm.
append_block() {
  local file="$1" block="$2"
  mkdir -p "$(dirname "$file")"
  if [ -f "$file" ] && grep -q 'rnm env' "$file"; then
    say "$file already sets up rnm; left unchanged."
    return
  fi
  printf '\n%s\n' "$block" >>"$file"
  say "Added rnm to $file:"
  printf '%s\n' "$block" | sed 's/^/    /'
}

setup_shell() {
  local shell_name dir env_flags file
  shell_name="$(basename "${SHELL:-bash}")"
  dir="$(quote "$install_dir")"
  env_flags=""
  if [ "$use_on_cd" = true ]; then env_flags=" --use-on-cd"; fi

  case "$shell_name" in
    bash | zsh)
      if [ "$shell_name" = zsh ]; then
        file="${ZDOTDIR:-$HOME}/.zshrc"
      elif [ "$(uname -s)" = Darwin ]; then
        # macOS terminals start login shells, which read ~/.bash_profile.
        file="$HOME/.bash_profile"
      else
        file="$HOME/.bashrc"
      fi
      append_block "$file" "$MARKER
if [ -x $dir/rnm ]; then
  export PATH=$dir:\"\$PATH\"
  eval \"\$(rnm env --shell $shell_name$env_flags)\"
fi"
      ;;
    fish)
      file="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/rnm.fish"
      append_block "$file" "$MARKER
if test -x $dir/rnm
  contains -- $dir \$PATH; or set -gx PATH $dir \$PATH
  rnm env --shell fish$env_flags | source
end"
      ;;
    nu)
      if PATH="$install_dir:$PATH" nu -c "rnm env --shell nu$env_flags | save -f (\$nu.data-dir | path join vendor/autoload/rnm.nu)"; then
        say "Saved rnm's Nushell setup to Nushell's vendor/autoload directory."
        say "Also add $install_dir to PATH in your Nushell env.nu."
      else
        say "Could not run nu; see https://github.com/$REPO/blob/main/docs/shell-setup.md#nushell"
      fi
      ;;
    *)
      say "Your shell ($shell_name) is not set up automatically."
      say "See https://github.com/$REPO/blob/main/docs/shell-setup.md"
      return
      ;;
  esac

  if [ -n "${file:-}" ] && grep -Eq 'nvm\.sh|fnm env' "$file" 2>/dev/null; then
    say "Note: $file also loads nvm or fnm. rnm comes later, so its node wins; remove the old lines when you no longer need them."
  fi
}

install_binary
if [ "$skip_shell" = true ]; then
  say "Skipped the shell setup (--skip-shell). Add rnm to your shell: https://github.com/$REPO/blob/main/docs/shell-setup.md"
else
  setup_shell
fi
say ""
say "Open a new terminal (or reload your shell startup file), then run:"
say "  rnm install --lts"
say "  rnm doctor"
