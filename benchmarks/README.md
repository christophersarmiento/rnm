# Benchmarks

`run.sh` times rnm, fnm and nvm end to end with
[hyperfine](https://github.com/sharkdp/hyperfine) and writes a Markdown report,
`results.md`, plus hyperfine's JSON and Markdown exports for every scenario.
The [Benchmarks workflow](../.github/workflows/benchmarks.yml) runs it on
Ubuntu and macOS (manually, or for pull requests that change these files) and
shows `results.md` in the job summary.

## Running locally

Prerequisites: `hyperfine`, `fnm`, `python3`, `curl`, `git` (to fetch nvm),
`xz` (optional; without it nvm falls back to `.tar.gz`), and a release build of
rnm:

```sh
cargo build --release
benchmarks/run.sh                       # results in benchmarks/results/
benchmarks/run.sh --runs 10 --only startup,calls
benchmarks/run.sh --help                # all options and their variables
```

The first run downloads about 150 MB into `benchmarks/.cache/` (two Node.js
versions in `.tar.xz` and `.tar.gz`, `index.json`, `index.tab`, the
`SHASUMS256.txt` files, and nvm itself). Later runs reuse it. The sandbox
holds six Node.js installs (about 1 GB) and is deleted at the end unless you
pass `--keep`. Delete `benchmarks/.cache/` to reclaim the space.

## Isolation

- Everything runs in a temporary sandbox: `HOME`, all `XDG_*` directories
  (including `XDG_RUNTIME_DIR`, where rnm and fnm keep per-shell links), and a
  separate install tree per tool (`RNM_DIR`, `FNM_DIR`, `NVM_DIR`). Your own
  setup is never read or changed.
- Every `RNM_*`, `NVM_*`, `FNM_*` and `NODE_*` variable is removed, as are
  `BASH_ENV`, `CDPATH` and `PROMPT_COMMAND`. `PATH` is reduced to the sandbox's
  `bin` (links to rnm, fnm and `xz`) and `/usr/bin:/bin:/usr/sbin:/sbin`, so no
  other node is visible and command lookup costs the same for every tool.
- Node.js is served from `benchmarks/.cache/mirror` by a local HTTP server on
  127.0.0.1 (`bench.py serve`), so network speed does not matter. Each tool is
  pointed at it with `RNM_NODE_MIRROR`, `FNM_NODE_DIST_MIRROR` or
  `NVM_NODEJS_ORG_MIRROR`. The archives are checked against the real
  `SHASUMS256.txt`.
- Each tool installs both versions and sets `default` to the newer one before
  timing starts. A check then confirms that each tool really puts the expected
  `node` on `PATH` after startup, `use` and the cd hook.

## Scenarios

Every command is a `bash` running a generated script (hyperfine `--shell=none`,
so no extra shell is timed). Output goes to `/dev/null`.

| Scenario | What is timed |
|---|---|
| Shell startup | What `~/.bashrc` does: `eval "$(rnm env --shell bash)"`, `eval "$(fnm env --shell bash)"`, or `. "$NVM_DIR/nvm.sh"` (which activates `default`). A `bash` running an empty script is the baseline. |
| `use`, `current`, `ls` | Per call, in an initialised shell. A script initialises the tool and makes N calls (`--calls`, default 10; `use` alternates between the two major versions, so every call switches). The time of initialisation alone, measured in the same hyperfine invocation, is subtracted and the rest divided by N. σ combines the two measurements in quadrature. |
| cd hook | Per `cd`, the same way. rnm and fnm use `env --use-on-cd`; nvm uses the `cdnvm` recipe from its README (`cdnvm.bash`, copied unchanged), since nvm has no built-in hook. Two cases: alternating between two projects whose `.nvmrc` files name different versions (every `cd` switches), and between two directories without a version file. |
| Install | `install <exact version>` with the version and the tool's download cache removed by hyperfine `--prepare` before every run, so each run downloads, (where the tool does) verifies, and unpacks. Fewer runs (`--install-runs`, default 5). nvm's time includes sourcing `nvm.sh --no-use`. |

The hooks replace `cd` with an alias. Non-interactive bash does not expand
aliases, so each hook script runs `shopt -s expand_aliases` first. An
interactive bash also runs `PROMPT_COMMAND` before each prompt; the scripts
do that too with `eval "${PROMPT_COMMAND:-}"` after every `cd` (only rnm sets
it, and it does nothing when the directory has not changed). Real interactive
shells (`bash -i`) were avoided because they need a terminal and read
startup files.

## Caveats

- Per-call costs are differences of two means; with few runs they are noisy,
  and a cost within noise of zero is shown as `≈ 0`.
- rnm and fnm start a process for every `use`, `current`, `ls` and hook call;
  nvm's commands are shell functions but run many utilities (`sed`, `ls`,
  `node --version`). Process start-up cost depends heavily on the OS.
- The tools do different amounts of work in some scenarios:
  - Install: fnm streams the archive straight into the extractor and checks
    no checksum, and with an exact version does not fetch the index. rnm and
    nvm fetch the index and `SHASUMS256.txt`, keep the archive in their cache
    (rnm also `fsync`s it), and verify it before unpacking.
  - cd hook without a version file: rnm and `cdnvm` switch back to `default`,
    so they do work on every `cd`. fnm's hook only looks for a version file in
    the new directory (its default `local` strategy) and starts no process
    otherwise.
  - `nvm ls` also prints aliases and resolves each of them; `nvm use`
    rewrites `PATH` in the shell instead of re-pointing a link.
- The bash that runs the benchmarks is `bash` on `PATH` (`--bash` to change).
  On macOS this is usually 3.2 unless Homebrew's bash comes first.
- Results from shared CI runners vary from run to run; compare tools within
  one report, not across reports.
