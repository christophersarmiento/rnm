# Benchmarks

rnm, fnm, and nvm, timed end to end with [hyperfine](https://github.com/sharkdp/hyperfine). The harness is in [`benchmarks/`](../benchmarks/README.md); anyone can run it.

## Summary

| | rnm | fnm | nvm |
|---|--:|--:|--:|
| **Linux** (GitHub Actions, Ubuntu 24.04, x64) | | | |
| Shell startup (added to `bash`) | **1.0 ms** | 1.1 ms | 114 ms |
| `use` (per call) | **0.8 ms** | 1.0 ms | 168 ms |
| `cd` into a project with a different `.nvmrc` | **0.8 ms** | 0.9 ms | 299 ms (`cdnvm` recipe) |
| `current` | **0.7 ms** | 0.8 ms | 7 ms |
| `ls` | 1.0 ms | **0.8 ms** | 610 ms |
| Install an exact version (local mirror) | **2.12 s** | 2.27 s | 2.47 s |
| **macOS** (Apple M3, macOS 27) | | | |
| Shell startup (added to `bash`) | **3.6 ms** | 3.9 ms | 130 ms |
| `use` (per call) | **3.5 ms** | 3.5 ms | 194 ms |
| `cd` into a project with a different `.nvmrc` | **3.5 ms** | 3.6 ms | 359 ms (`cdnvm` recipe) |
| `current` | **3.1 ms** | 3.5 ms | 23 ms |
| `ls` | 4.2 ms | **3.6 ms** | 750 ms |
| Install an exact version (local mirror) | 2.47 s | **2.28 s** | 3.55 s |

rnm 0.1.0, fnm 1.39.0, nvm 0.40.8, 13 October 2026. Lower is better. Linux: one run of the harness on a GitHub Actions runner, bash 5.2, 20 runs per command. macOS: bash 3.2, 10 runs per command. Process start-up is much cheaper on Linux, which is why every number there is smaller.

In short: rnm starts a shell and switches versions as fast as fnm, and 25 to 360 times faster than nvm, while keeping nvm's files and behaviour. Installs are on a par with fnm, although fnm skips checksum verification and the download cache.

## Method

- **Isolation.** Each run uses a temporary `HOME`, separate install trees per tool, and a `PATH` with nothing but the tools and `/usr/bin:/bin`. Every `RNM_*`, `NVM_*`, and `FNM_*` variable is removed.
- **Local mirror.** Node.js v20.20.2 and v22.23.3 are served from a local HTTP server, so network speed does not count. Each tool installs both and sets `default` to 22 before timing starts, and the harness checks that each tool really puts the expected `node` on `PATH`.
- **Shell startup** is a non-interactive `bash` running what `~/.bashrc` would: `eval "$(rnm env --shell bash)"`, `eval "$(fnm env --shell bash)"`, or `. "$NVM_DIR/nvm.sh"`. The table shows the time added to a bare `bash` (1.5 ms).
- **Per-call numbers** (`use`, `current`, `ls`, `cd`) come from a shell that initialises the tool and makes 10 calls; the initialisation alone is measured in the same hyperfine run, subtracted, and the rest divided by 10. `use` alternates between 20 and 22, so every call switches.
- **cd hook:** rnm and fnm use `env --use-on-cd`; nvm has no hook, so it uses the `cdnvm` recipe from nvm's README.
- **Install** removes the version and the tool's download cache before every run, so each run downloads, verifies (rnm, nvm), and unpacks the `.tar.xz`.

## Where the tools do different work

- **Install:** fnm streams the archive straight into the extractor; it checks no checksum and keeps no copy. rnm and nvm fetch `SHASUMS256.txt`, keep the archive in the shared download cache (so `install --offline` and reinstalls need no network), and verify it. rnm also flushes the archive to disk before using it.
- **`cd` into a directory without a version file:** rnm (and `cdnvm`) switch back to the `default` version, as nvm's README recipes do, so rnm runs once per directory change (about 3.5 ms). fnm's hook does nothing there and stays on the previous version.
- **`ls`:** nvm also resolves every alias; rnm lists aliases with their resolved versions too.

## Full results (macOS)

The Linux report has the same shape.

```text
Shell startup                    Mean ± σ (ms)   Relative
  rnm                             5.10 ± 0.15     1.00
  fnm                             5.39 ± 0.52     1.06×
  nvm                           131.0  ± 2.8     25.68×
  bash baseline                   1.49 ± 0.09

use (per call)
  rnm                             3.45 ± 0.13     1.00
  fnm                             3.54 ± 0.17     1.03×
  nvm                           194.1  ± 6.3     56.26×

current (per call)
  rnm                             3.14 ± 0.14     1.00
  fnm                             3.47 ± 0.13     1.10×
  nvm                            23.34 ± 5.47     7.43×

ls (per call)
  rnm                             4.19 ± 0.09     1.17×
  fnm                             3.58 ± 0.17     1.00
  nvm                           750.0  ± 29.7   209.38×

cd between projects with different .nvmrc files (per cd)
  rnm                             3.46 ± 0.15     1.00
  fnm                             3.63 ± 0.19     1.05×
  nvm (cdnvm)                   358.7  ± 8.6    103.59×

cd between directories without a version file (per cd)
  rnm                             3.46 ± 0.16
  fnm                             ≈ 0 (the hook does nothing)
  nvm (cdnvm)                    74.76 ± 1.62

install v22.23.3 (empty cache)
  rnm                          2473.9  ± 29.0     1.08×
  fnm                          2284.9  ± 56.1     1.00
  nvm                          3550.2  ± 94.0     1.55×
```

10 runs (+2 warm-up) per command, 5 (+1) per install, 10 calls per loop.

## Run them yourself

You need `hyperfine`, `fnm`, `python3`, `curl`, and `git`:

```sh
cargo build --release
benchmarks/run.sh
```

The report goes to `benchmarks/results/results.md`. See [`benchmarks/README.md`](../benchmarks/README.md) for options and caveats.
