#!/usr/bin/env bash
# End-to-end benchmarks of rnm, fnm and nvm with hyperfine.
# See benchmarks/README.md. Works with bash 3.2 (macOS's /bin/bash).
#
# Much of this file writes shell code for later, so `$` inside single quotes
# is intended.
# shellcheck disable=SC2016
set -euo pipefail

# Pinned inputs. Changing them changes the CI cache key (it hashes this file).
readonly NODE_OLD=v20.20.2
readonly NODE_NEW=v22.23.3
readonly NVM_TAG=v0.40.8
readonly NVM_REPO_URL=https://github.com/nvm-sh/nvm.git
readonly NODEJS_DIST=https://nodejs.org/dist

BENCH_DIR=$(cd "$(dirname "$0")" && pwd)
REPO_DIR=$(dirname "$BENCH_DIR")
readonly BENCH_DIR REPO_DIR
readonly ALL_SCENARIOS=startup,calls,cd,install

usage() {
  cat <<EOF
Usage: benchmarks/run.sh [options]

Options (environment variable in brackets):
  --rnm PATH            rnm binary [BENCH_RNM] (default: target/release/rnm)
  --fnm PATH            fnm binary [BENCH_FNM] (default: fnm on PATH)
  --nvm-sh PATH         nvm.sh [NVM_SH] (default: clone nvm $NVM_TAG into the cache)
  --bash PATH           bash used for every benchmark [BENCH_BASH] (default: bash on PATH)
  --runs N              runs per command [BENCH_RUNS] (default: 20)
  --warmup N            warm-up runs per command [BENCH_WARMUP] (default: 3)
  --install-runs N      runs per install command [BENCH_INSTALL_RUNS] (default: 5)
  --calls N             calls per loop in the per-call scenarios, even [BENCH_CALLS] (default: 10)
  --only LIST           comma-separated subset of: $ALL_SCENARIOS [BENCH_ONLY]
  --out DIR             output directory [BENCH_OUT] (default: benchmarks/results)
  --cache DIR           download cache [BENCH_CACHE] (default: benchmarks/.cache)
  --keep                keep the sandbox (it holds six Node.js installs)
  -h, --help            show this help
EOF
}

die() {
  printf 'run.sh: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '==> %s\n' "$*" >&2
}

rnm_bin=${BENCH_RNM:-$REPO_DIR/target/release/rnm}
fnm_bin=${BENCH_FNM:-}
nvm_sh=${NVM_SH:-}
bash_bin=${BENCH_BASH:-}
runs=${BENCH_RUNS:-20}
warmup=${BENCH_WARMUP:-3}
install_runs=${BENCH_INSTALL_RUNS:-5}
calls=${BENCH_CALLS:-10}
only=${BENCH_ONLY:-$ALL_SCENARIOS}
out_dir=${BENCH_OUT:-$BENCH_DIR/results}
cache_dir=${BENCH_CACHE:-$BENCH_DIR/.cache}
keep_sandbox=false

while [ $# -gt 0 ]; do
  case $1 in
    --rnm) rnm_bin=$2; shift 2 ;;
    --fnm) fnm_bin=$2; shift 2 ;;
    --nvm-sh) nvm_sh=$2; shift 2 ;;
    --bash) bash_bin=$2; shift 2 ;;
    --runs) runs=$2; shift 2 ;;
    --warmup) warmup=$2; shift 2 ;;
    --install-runs) install_runs=$2; shift 2 ;;
    --calls) calls=$2; shift 2 ;;
    --only) only=$2; shift 2 ;;
    --out) out_dir=$2; shift 2 ;;
    --cache) cache_dir=$2; shift 2 ;;
    --keep) keep_sandbox=true; shift ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; die "unknown option: $1" ;;
  esac
done

is_count() {
  case $1 in '' | *[!0-9]*) return 1 ;; *) [ "$1" -gt 0 ] ;; esac
}
for value in "$runs" "$warmup" "$install_runs" "$calls"; do
  is_count "$value" || die "run counts must be positive integers, got '$value'"
done
[ $((calls % 2)) -eq 0 ] || die "--calls must be even (calls alternate between two targets), got $calls"

wants() {
  case ",$only," in *",$1,"*) return 0 ;; *) return 1 ;; esac
}

# --- Prerequisites ---------------------------------------------------------

require() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is required but not on PATH. $2"
}

require hyperfine "Install it: brew install hyperfine / apt-get install hyperfine / https://github.com/sharkdp/hyperfine/releases"
require python3 "Install Python 3."
require curl "Install curl."
hyperfine_bin=$(command -v hyperfine)
python_bin=$(command -v python3)
curl_bin=$(command -v curl)

[ -n "$bash_bin" ] || bash_bin=$(command -v bash)
[ -x "$rnm_bin" ] || die "rnm not found at $rnm_bin. Build it with 'cargo build --release' or pass --rnm."
if [ -z "$fnm_bin" ]; then
  require fnm "Install it: https://github.com/Schniz/fnm#installation, or pass --fnm."
  fnm_bin=$(command -v fnm)
fi
[ -x "$fnm_bin" ] || die "fnm not found at $fnm_bin."

absolute() {
  (cd "$(dirname "$1")" && printf '%s/%s\n' "$(pwd -P)" "$(basename "$1")")
}
rnm_bin=$(absolute "$rnm_bin")
fnm_bin=$(absolute "$fnm_bin")
mkdir -p "$cache_dir" "$out_dir"
cache_dir=$(cd "$cache_dir" && pwd -P)
out_dir=$(cd "$out_dir" && pwd -P)

if [ -z "$nvm_sh" ]; then
  nvm_clone=$cache_dir/nvm-$NVM_TAG
  if [ ! -f "$nvm_clone/nvm.sh" ]; then
    require git "git is needed to fetch nvm; or pass --nvm-sh."
    log "Cloning nvm $NVM_TAG"
    rm -rf "$nvm_clone"
    git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$NVM_TAG" "$NVM_REPO_URL" "$nvm_clone"
  fi
  nvm_sh=$nvm_clone/nvm.sh
fi
[ -f "$nvm_sh" ] || die "nvm.sh not found at $nvm_sh."
nvm_sh=$(absolute "$nvm_sh")

case $(uname -s) in
  Darwin) node_os=darwin ;;
  Linux) node_os=linux ;;
  *) die "unsupported OS: $(uname -s)" ;;
esac
case $(uname -m) in
  arm64 | aarch64) node_arch=arm64 ;;
  x86_64 | amd64) node_arch=x64 ;;
  *) die "unsupported CPU: $(uname -m)" ;;
esac

# --- Local mirror ----------------------------------------------------------

mirror_dir=$cache_dir/mirror

fetch() {
  local url=$1 dest=$2
  [ -s "$dest" ] && return 0
  mkdir -p "$(dirname "$dest")"
  log "Downloading $url"
  "$curl_bin" -fsSL --retry 3 -o "$dest.part" "$url" || die "download failed: $url"
  mv "$dest.part" "$dest"
}

fetch "$NODEJS_DIST/index.json" "$mirror_dir/index.json"
fetch "$NODEJS_DIST/index.tab" "$mirror_dir/index.tab"
for version in "$NODE_OLD" "$NODE_NEW"; do
  fetch "$NODEJS_DIST/$version/SHASUMS256.txt" "$mirror_dir/$version/SHASUMS256.txt"
  for ext in tar.xz tar.gz; do
    file=node-$version-$node_os-$node_arch.$ext
    fetch "$NODEJS_DIST/$version/$file" "$mirror_dir/$version/$file"
  done
done
"$python_bin" "$BENCH_DIR/bench.py" verify "$mirror_dir" "$NODE_OLD" "$NODE_NEW" \
  || die "the mirror had corrupt files; run again to fetch them"

# --- Sandbox ---------------------------------------------------------------

sandbox=$(mktemp -d "${TMPDIR:-/tmp}/rnm-bench.XXXXXX")
sandbox=$(cd "$sandbox" && pwd -P)
server_pid=

cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  if [ "$keep_sandbox" = true ]; then
    log "Sandbox kept: $sandbox"
  else
    rm -rf "$sandbox"
  fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

port_file=$sandbox/port
"$python_bin" "$BENCH_DIR/bench.py" serve "$mirror_dir" "$port_file" &
server_pid=$!
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  [ -s "$port_file" ] && break
  sleep 0.25
done
[ -s "$port_file" ] || die "the local mirror did not start"
mirror_url=http://127.0.0.1:$(cat "$port_file")

# Nothing from the user's own setup may leak in: no tool variables, no
# startup files, and a short PATH without any node. A short PATH also keeps
# command lookup cheap and equal for every tool (nvm runs many utilities).
for name in $(compgen -e); do
  case $name in
    RNM_* | NVM_* | FNM_* | NODE_* | npm_config_* | BASH_ENV | ENV | CDPATH | PROMPT_COMMAND | XDG_*)
      unset "$name" ;;
  esac
done

bin_dir=$sandbox/bin
mkdir -p "$bin_dir" "$sandbox/home" "$sandbox/run" "$sandbox/work" \
  "$sandbox/rnm" "$sandbox/nvm" "$sandbox/fnm"
chmod 700 "$sandbox/run"
ln -s "$rnm_bin" "$bin_dir/rnm"
ln -s "$fnm_bin" "$bin_dir/fnm"
# All three tools prefer .tar.xz; nvm needs the xz program for that.
if xz_bin=$(command -v xz); then
  ln -s "$xz_bin" "$bin_dir/xz"
fi

export HOME=$sandbox/home
export PATH=$bin_dir:/usr/bin:/bin:/usr/sbin:/sbin
export XDG_CONFIG_HOME=$HOME/.config XDG_CACHE_HOME=$HOME/.cache \
  XDG_DATA_HOME=$HOME/.local/share XDG_STATE_HOME=$HOME/.local/state \
  XDG_RUNTIME_DIR=$sandbox/run
export RNM_DIR=$sandbox/rnm RNM_NODE_MIRROR=$mirror_url
export FNM_DIR=$sandbox/fnm FNM_NODE_DIST_MIRROR=$mirror_url
export NVM_DIR=$sandbox/nvm NVM_NODEJS_ORG_MIRROR=$mirror_url
# A real nvm install: nvm.sh and its helpers live in $NVM_DIR.
for helper in nvm.sh nvm-exec bash_completion; do
  if [ -f "$(dirname "$nvm_sh")/$helper" ]; then
    cp "$(dirname "$nvm_sh")/$helper" "$NVM_DIR/"
  fi
done

old_major=${NODE_OLD#v}
old_major=${old_major%%.*}
new_major=${NODE_NEW#v}
new_major=${new_major%%.*}

# Projects for the cd hook: two with an .nvmrc, two without.
proj=$sandbox/proj
mkdir -p "$proj/old" "$proj/new" "$proj/plain1" "$proj/plain2"
echo "$old_major" >"$proj/old/.nvmrc"
echo "$new_major" >"$proj/new/.nvmrc"

# --- Setup: install both versions with every tool ----------------------------

run_bash() {
  "$bash_bin" -c "$1" >"$sandbox/setup.log" 2>&1 || {
    cat "$sandbox/setup.log" >&2
    die "setup command failed: $1"
  }
}

log "Installing $NODE_OLD and $NODE_NEW with each tool from $mirror_url"
run_bash "rnm install $NODE_OLD && rnm install $NODE_NEW && rnm alias default $NODE_NEW"
run_bash "fnm install $NODE_OLD && fnm install $NODE_NEW && fnm default $NODE_NEW"
run_bash ". \"\$NVM_DIR/nvm.sh\" --no-use && nvm install $NODE_OLD && nvm install $NODE_NEW && nvm alias default $NODE_NEW"

# --- Scripts ---------------------------------------------------------------

scripts=$sandbox/scripts
mkdir -p "$scripts"

init_code() {
  case $1 in
    rnm) printf '%s\n' 'eval "$(rnm env --shell bash)"' ;;
    fnm) printf '%s\n' 'eval "$(fnm env --shell bash)"' ;;
    nvm) printf '%s\n' '. "$NVM_DIR/nvm.sh"' ;;
  esac
}

hook_init_code() {
  printf '%s\n' 'shopt -s expand_aliases'
  case $1 in
    rnm) printf '%s\n' 'eval "$(rnm env --shell bash --use-on-cd)"' ;;
    fnm) printf '%s\n' 'eval "$(fnm env --shell bash --use-on-cd)"' ;;
    nvm)
      printf '%s\n' '. "$NVM_DIR/nvm.sh"'
      cat "$BENCH_DIR/cdnvm.bash"
      ;;
  esac
}

# alternate COUNT LINE_A LINE_B: COUNT lines, alternating.
alternate() {
  local i=0
  while [ "$i" -lt "$1" ]; do
    if [ $((i % 2)) -eq 0 ]; then printf '%s\n' "$2"; else printf '%s\n' "$3"; fi
    i=$((i + 1))
  done
}

# cd_code DIR: a cd, then what an interactive prompt runs before showing.
cd_code() {
  printf 'cd %q\neval "${PROMPT_COMMAND:-}"\n' "$1"
}

: >"$scripts/baseline.sh"
for tool in rnm fnm nvm; do
  init_code "$tool" >"$scripts/$tool-init.sh"
  { init_code "$tool"; alternate "$calls" "$tool use $old_major >/dev/null 2>&1" "$tool use $new_major >/dev/null 2>&1"; } >"$scripts/$tool-use.sh"
  { init_code "$tool"; alternate "$calls" "$tool current >/dev/null 2>&1" "$tool current >/dev/null 2>&1"; } >"$scripts/$tool-current.sh"
  { init_code "$tool"; alternate "$calls" "$tool ls >/dev/null 2>&1" "$tool ls >/dev/null 2>&1"; } >"$scripts/$tool-ls.sh"
  hook_init_code "$tool" >"$scripts/$tool-hook-init.sh"
  { hook_init_code "$tool"; alternate "$calls" "$(cd_code "$proj/old")" "$(cd_code "$proj/new")"; } >"$scripts/$tool-cd-switch.sh"
  { hook_init_code "$tool"; alternate "$calls" "$(cd_code "$proj/plain1")" "$(cd_code "$proj/plain2")"; } >"$scripts/$tool-cd-plain.sh"
done
printf '%s\n' "rnm install $NODE_NEW >/dev/null 2>&1" >"$scripts/rnm-install.sh"
printf '%s\n' "fnm install $NODE_NEW >/dev/null 2>&1" >"$scripts/fnm-install.sh"
printf '%s\n' ". \"\$NVM_DIR/nvm.sh\" --no-use" "nvm install $NODE_NEW >/dev/null 2>&1" >"$scripts/nvm-install.sh"

# --- Sanity checks (not timed) ----------------------------------------------

# check TOOL LABEL EXPECTED CODE: run CODE in the sandbox's working directory
# and compare what it writes to file descriptor 3.
check() {
  local actual
  actual=$(cd "$sandbox/work" && "$bash_bin" -c "$4" 3>&1 >/dev/null 2>&1 | tr '\n' ' ') || true
  actual=${actual% }
  [ "$actual" = "$3" ] || die "$1: $2: expected '$3', got '$actual'"
}

log "Checking that every tool switches versions in the sandbox"
for tool in rnm fnm nvm; do
  check "$tool" "init + use" "$NODE_NEW $NODE_OLD" \
    "$(init_code "$tool"); node -v >&3; $tool use $old_major; node -v >&3"
  check "$tool" "cd hook" "$NODE_OLD $NODE_NEW" \
    "$(hook_init_code "$tool")
$(cd_code "$proj/old")
node -v >&3
$(cd_code "$proj/new")
node -v >&3"
done
for script in "$scripts"/*.sh; do
  case $script in *-install.sh) continue ;; esac
  (cd "$sandbox/work" && "$bash_bin" "$script" >/dev/null 2>&1) || die "$script failed"
done

# --- Benchmarks --------------------------------------------------------------

# bench NAME RUNS WARMUP [--prepare-for CMD_NAME CMD]... CMD_NAME SCRIPT ...
# Each command runs "bash SCRIPT" without an intermediate shell.
bench() {
  local name=$1 bench_runs=$2 bench_warmup=$3
  shift 3
  local args
  args=(--shell=none --runs "$bench_runs" --warmup "$bench_warmup"
    --export-json "$out_dir/$name.json" --export-markdown "$out_dir/$name.md")
  while [ $# -gt 0 ]; do
    if [ "$1" = --prepare ]; then
      args+=(--prepare "$2")
      shift 2
      continue
    fi
    args+=(--command-name "$1" "'$bash_bin' '$scripts/$2.sh'")
    shift 2
  done
  log "hyperfine: $name"
  (cd "$sandbox/work" && "$hyperfine_bin" "${args[@]}")
}

if wants startup; then
  bench startup "$runs" "$warmup" baseline baseline \
    "rnm init" rnm-init "fnm init" fnm-init "nvm init" nvm-init
fi

if wants calls; then
  for tool in rnm fnm nvm; do
    bench "calls-$tool" "$runs" "$warmup" \
      "$tool init" "$tool-init" "$tool use" "$tool-use" \
      "$tool current" "$tool-current" "$tool ls" "$tool-ls"
  done
fi

if wants cd; then
  for tool in rnm fnm nvm; do
    bench "cd-$tool" "$runs" "$warmup" \
      "$tool hook-init" "$tool-hook-init" "$tool cd-switch" "$tool-cd-switch" \
      "$tool cd-plain" "$tool-cd-plain"
  done
fi

if wants install; then
  # hyperfine applies the n-th --prepare to the n-th command.
  bench install "$install_runs" 1 \
    --prepare "rm -rf '$RNM_DIR/versions/node/$NODE_NEW' '$RNM_DIR/.cache'" \
    --prepare "rm -rf '$FNM_DIR/node-versions/$NODE_NEW' '$FNM_DIR/node-versions/.downloads'" \
    --prepare "rm -rf '$NVM_DIR/versions/node/$NODE_NEW' '$NVM_DIR/.cache'" \
    "rnm install" rnm-install "fnm install" fnm-install "nvm install" nvm-install
fi

# --- Report ------------------------------------------------------------------

first_line() {
  "$@" 2>/dev/null | head -n 1
}

case $node_os in
  darwin)
    os_name="macOS $(sw_vers -productVersion) ($(uname -m))"
    cpu_name="$(sysctl -n machdep.cpu.brand_string), $(sysctl -n hw.ncpu) cores"
    ;;
  linux)
    # shellcheck source=/dev/null
    os_name="$(. /etc/os-release && printf '%s' "$PRETTY_NAME") ($(uname -m))"
    cpu_name="$(grep -m1 '^model name' /proc/cpuinfo | sed 's/^[^:]*: *//'), $(nproc) cores"
    [ -n "${cpu_name%%,*}" ] || cpu_name="$(uname -m), $(nproc) cores"
    ;;
esac
nvm_version=$("$bash_bin" -c '. "$NVM_DIR/nvm.sh" --no-use && nvm --version')
rnm_version=$(first_line rnm --version)
fnm_version=$(first_line fnm --version)

BENCH_META_DATE=$(date -u '+%Y-%m-%d %H:%M UTC') \
  BENCH_META_OS=$os_name \
  BENCH_META_CPU=$cpu_name \
  BENCH_META_BASH="$(first_line "$bash_bin" --version)" \
  BENCH_META_HYPERFINE="$(first_line "$hyperfine_bin" --version)" \
  BENCH_META_RNM=${rnm_version#rnm } \
  BENCH_META_FNM=${fnm_version#fnm } \
  BENCH_META_NVM=$nvm_version \
  BENCH_META_NODE="$NODE_OLD $NODE_NEW" \
  BENCH_META_RUNS=$runs BENCH_META_WARMUP=$warmup \
  BENCH_META_INSTALL_RUNS=$install_runs BENCH_META_CALLS=$calls \
  "$python_bin" -c '
import json, os, sys
e = lambda k: os.environ["BENCH_META_" + k]
json.dump({
    "date": e("DATE"), "os": e("OS"), "cpu": e("CPU"), "bash": e("BASH"),
    "hyperfine": e("HYPERFINE"),
    "versions": {"rnm": e("RNM"), "fnm": e("FNM"), "nvm": e("NVM")},
    "node_versions": e("NODE").split(), "runs": e("RUNS"), "warmup": e("WARMUP"),
    "install_runs": e("INSTALL_RUNS"), "calls": e("CALLS"),
}, open(sys.argv[1], "w"), indent=2)
' "$out_dir/meta.json"

"$python_bin" "$BENCH_DIR/bench.py" report "$out_dir" "$out_dir/meta.json" >/dev/null
log "Results: $out_dir/results.md"
