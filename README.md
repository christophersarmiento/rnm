# rnm

[![CI](https://github.com/christophersarmiento/rnm/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/christophersarmiento/rnm/actions/workflows/ci.yml)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)

**A fast, lightweight Node.js version manager that is a drop-in replacement for nvm.**

rnm is one small binary written in Rust. It reads and writes nvm's own files (`$NVM_DIR`, aliases, `.nvmrc`), so you can switch from nvm without reinstalling a single version, and switch back at any time. It starts as fast as fnm, and it adds what neither tool has: safe credentials for private mirrors, and first-class Windows support.

```console
$ rnm install --lts
Downloading and installing node v24.11.0...
Checksums matched!
Now using node v24.11.0 (npm v11.6.1)
Creating default alias: default -> lts/* (-> v24.11.0)

$ cd my-project && cat .nvmrc
20
$ rnm use
Found '/home/me/my-project/.nvmrc' with version <20>
Now using node v20.19.5 (npm v10.8.2)
```

## Features

- **nvm-compatible.** Same install tree, alias files, `.nvmrc` rules, version grammar (`lts/*`, `lts/iron`, `lts/-1`, `node`, `system`, aliases), command names, flags, output formats, and exit codes. `NVM_BIN` and `NVM_INC` are set for the tools that read them.
- **Fast.** rnm adds about 1 ms (Linux) to 4 ms (macOS) to a shell's startup, and `use` costs about the same: as fast as fnm, and 25 to 360 times faster than nvm. See the [benchmarks](docs/benchmarks.md).
- **Five shells, three operating systems.** bash (3.2+), zsh, fish, Nushell, and PowerShell (7 and Windows PowerShell 5.1) on macOS, Linux, and Windows, including Windows on Arm and Git Bash.
- **Automatic switching** on `cd` from `.nvmrc`, `.node-version`, and (opt-in) `package.json` `devEngines` / `engines`, with npm ranges such as `^20.1` or `>=18 <21`.
- **Verified downloads.** Every archive is checked against `SHASUMS256.txt` while it streams, then unpacked and renamed into place, so a version is never half-installed.
- **Private mirrors done right.** Credentials are scoped to a URL prefix and checked again on every redirect, so they never leak to a CDN. Supports custom headers (GitLab's `PRIVATE-TOKEN`), netrc, credential helpers for short-lived tokens (AWS CodeArtifact), corporate CA files, and fallback mirrors.
- **The nvm workflow, complete.** `default-packages`, `--reinstall-packages-from`, `--latest-npm`, `install --offline`, `exec`, `which`, colors, and `NVM_SYMLINK_CURRENT`.
- **`rnm doctor`** checks your setup and tells you what to fix.

## Installation

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/christophersarmiento/rnm/main/install.sh | bash
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/christophersarmiento/rnm/main/install.ps1 | iex
```

The scripts download a release, verify its checksum, install the binary, and add rnm to your shell's startup file. Open a new terminal afterwards. See [Installation](docs/installation.md) for options, manual installs, building from source, upgrading, and uninstalling.

> rnm has not published its first release yet. Until then, build it from source with `cargo install --locked --git https://github.com/christophersarmiento/rnm`, then [set up your shell](docs/shell-setup.md).

### Shell setup

If you installed by hand, add one line to your shell's startup file:

| Shell | Startup file | Line |
|---|---|---|
| bash | `~/.bashrc` | `eval "$(rnm env --shell bash)"` |
| zsh | `~/.zshrc` | `eval "$(rnm env --shell zsh)"` |
| fish | `~/.config/fish/conf.d/rnm.fish` | `rnm env --shell fish \| source` |
| PowerShell | `$PROFILE` | `rnm env --shell powershell \| Out-String \| Invoke-Expression` |
| Nushell | (run once) | `rnm env --shell nu \| save -f ($nu.data-dir \| path join vendor/autoload/rnm.nu)` |

Add `--use-on-cd` to switch versions automatically when you change directories. Details in [Shell setup](docs/shell-setup.md).

## Usage

```sh
rnm install 22            # newest 22.x
rnm install --lts         # newest LTS
rnm install lts/iron      # newest of an LTS line
rnm use 20                # switch this shell
rnm use                   # use the version in the nearest .nvmrc or .node-version
rnm current               # v20.19.5
rnm ls                    # installed versions and aliases
rnm ls-remote --lts       # LTS versions on the mirror
rnm alias default 22      # the version new shells start with (also: rnm default 22)
rnm exec 18 node app.js   # run one command with another version
rnm which 20              # path to that version's node
rnm uninstall 18
rnm doctor                # check the setup
```

Every command and flag is described in [Commands](docs/commands.md).

## Coming from nvm or fnm

- **nvm:** remove the lines that load `nvm.sh` from your startup file and add rnm's. Your versions, aliases, `default-packages`, and `.nvmrc` files keep working, and nvm can still use the same tree.
- **fnm:** rnm accepts fnm's names too (`list`, `list-remote`, `--use-on-cd`, `--install-if-missing`, `exec --using`, `FNM_NODE_DIST_MIRROR`), but it keeps versions in nvm's tree, so install them again once.
- **nvm-windows:** `rnm import nvm-windows` links your existing versions without copying them.

See [Migrating](docs/migrating.md).

## How rnm compares

| | nvm | fnm | rnm |
|---|---|---|---|
| Implementation | ~5,600 lines of POSIX shell, sourced into every shell | Rust binary | Rust binary (~2.7 MB) |
| nvm's tree, aliases, and `.nvmrc` rules | yes | no (own tree; `lts/-1`, `node`, and `.nvmrc` comments fail) | yes |
| Windows | no (nvm-windows is a separate project) | yes | yes, including Windows PowerShell 5.1 and Windows on Arm |
| Nushell | no | no | yes |
| Checksum verification | SHA-256, skipped when no tool is found | none | SHA-256 always, while downloading |
| Mirror credentials | `NVM_AUTH_HEADER`, sent on every request | none | scoped per URL prefix; headers, netrc, helpers, CA files |
| Automatic switching on `cd` | README snippets | `--use-on-cd` | `--use-on-cd` (also `package.json`, opt-in) |
| `default-packages`, `--reinstall-packages-from` | yes | no | yes |

Measured with hyperfine on Linux (GitHub Actions, x64) and an Apple M3; lower is better. Details in [Benchmarks](docs/benchmarks.md).

| | rnm | fnm | nvm |
|---|--:|--:|--:|
| Shell startup, Linux / macOS | 1.0 / 3.6 ms | 1.1 / 3.9 ms | 114 / 130 ms |
| `use`, Linux / macOS | 0.8 / 3.5 ms | 1.0 / 3.5 ms | 168 / 194 ms |
| `cd` into a project with another `.nvmrc`, Linux / macOS | 0.8 / 3.5 ms | 0.9 / 3.6 ms | 299 / 359 ms |

## Documentation

- [Installation](docs/installation.md): install script, manual install, upgrade, uninstall
- [Shell setup](docs/shell-setup.md): every shell, `--use-on-cd`, completions
- [Commands](docs/commands.md): every command, flag, and exit code
- [Version files and specs](docs/version-files.md): `.nvmrc`, `.node-version`, `package.json`, the version grammar
- [Configuration](docs/configuration.md): `config.toml`, environment variables, mirrors, credentials, proxies
- [Windows](docs/windows.md): layout, PowerShell, Git Bash, importing from nvm-windows, `default --persist`
- [Migrating from nvm, fnm, and nvm-windows](docs/migrating.md)
- [Troubleshooting](docs/troubleshooting.md)
- [How rnm works](docs/how-it-works.md)
- [Benchmarks](docs/benchmarks.md)

## License

[GPL-3.0](LICENSE).
