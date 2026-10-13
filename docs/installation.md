# Installation

- [Install script (macOS and Linux)](#install-script-macos-and-linux)
- [Install script (Windows)](#install-script-windows)
- [Manual install](#manual-install)
- [Build from source](#build-from-source)
- [Check the installation](#check-the-installation)
- [Upgrade](#upgrade)
- [Uninstall](#uninstall)

rnm is a single binary. Release archives exist for:

| Platform | Archive |
|---|---|
| Linux x64 (any distribution, static musl build) | `rnm-x86_64-unknown-linux-musl.tar.gz` |
| Linux arm64 | `rnm-aarch64-unknown-linux-musl.tar.gz` |
| macOS Apple Silicon | `rnm-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `rnm-x86_64-apple-darwin.tar.gz` |
| Windows x64 | `rnm-x86_64-pc-windows-msvc.zip` |
| Windows on Arm | `rnm-aarch64-pc-windows-msvc.zip` |

Each release also has a `SHA256SUMS` file. A `nightly` prerelease is rebuilt from the `dev` branch when it changes.

> rnm has not published its first release yet. Until it does, [build from source](#build-from-source).

## Install script (macOS and Linux)

```sh
curl -fsSL https://raw.githubusercontent.com/christophersarmiento/rnm/main/install.sh | bash
```

The [script](../install.sh):

1. Picks the archive for your system. An x64 shell under Rosetta 2 gets the Apple Silicon build.
2. Downloads it and `SHA256SUMS` from the GitHub release, and stops if the checksum does not match.
3. Installs `rnm` to `~/.local/share/rnm/bin` (`$XDG_DATA_HOME/rnm/bin` if set).
4. Adds rnm to the startup file of your login shell (`$SHELL`):

   | Shell | File |
   |---|---|
   | bash on Linux | `~/.bashrc` |
   | bash on macOS | `~/.bash_profile` (macOS terminals start login shells) |
   | zsh | `${ZDOTDIR:-~}/.zshrc` |
   | fish | `~/.config/fish/conf.d/rnm.fish` |
   | Nushell | saves rnm's setup to Nushell's `vendor/autoload` directory |

   The block it adds starts with `# rnm`. A file that already runs `rnm env` is left alone, so running the script again is safe.

Pass options after `bash -s --`:

```sh
curl -fsSL https://raw.githubusercontent.com/christophersarmiento/rnm/main/install.sh | bash -s -- --use-on-cd
```

| Option | Effect |
|---|---|
| `--version <tag>` | Install a specific release, for example `v0.1.0`. Default: the latest release. |
| `--nightly` | Install the newest nightly build. |
| `--install-dir <dir>` | Put the binary in `<dir>` (also `RNM_INSTALL_DIR`). |
| `--skip-shell` | Do not change any startup file. |
| `--use-on-cd` | Set up automatic version switching on `cd`. |

`RNM_INSTALL_BASE_URL` downloads from another URL or a local directory (it must hold the archive and `SHA256SUMS`), for example an internal copy of the release.

The script needs `bash`, `curl` or `wget`, `tar`, and `sha256sum` or `shasum`.

## Install script (Windows)

In PowerShell 7 or Windows PowerShell 5.1:

```powershell
irm https://raw.githubusercontent.com/christophersarmiento/rnm/main/install.ps1 | iex
```

The [script](../install.ps1):

1. Picks the archive for the machine's native CPU, so an x64 PowerShell on a Windows on Arm PC still gets the Arm64 build.
2. Downloads it and `SHA256SUMS`, and stops if the checksum does not match.
3. Installs `rnm.exe` to `%LOCALAPPDATA%\rnm\bin`.
4. Adds that folder to your user `Path` (the raw registry value, so entries such as `%USERPROFILE%\bin` stay intact).
5. Adds rnm to the profiles of both Windows PowerShell 5.1 and PowerShell 7 (`Documents\WindowsPowerShell\profile.ps1` and `Documents\PowerShell\profile.ps1`).

If your execution policy is `Restricted` or `AllSigned`, PowerShell does not run profiles. The script tells you, and the fix is:

```powershell
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
```

To pass options, run the script as a script block:

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/christophersarmiento/rnm/main/install.ps1))) -Version v0.1.0 -UseOnCd
```

| Option | Effect |
|---|---|
| `-Version <tag>` | Install a specific release (also `RNM_INSTALL_VERSION`). |
| `-Nightly` | Install the newest nightly build. |
| `-InstallDir <dir>` | Put `rnm.exe` in `<dir>` (also `RNM_INSTALL_DIR`). |
| `-SkipShell` | Change neither your `Path` nor your profiles. |
| `-UseOnCd` | Set up automatic version switching on directory changes. |

Git Bash and Nushell on Windows are not set up by the script; see [Shell setup](shell-setup.md).

## Manual install

1. Download the archive for your platform and `SHA256SUMS` from the [releases page](https://github.com/christophersarmiento/rnm/releases).
2. Check it: `sha256sum --check --ignore-missing SHA256SUMS` (macOS: `shasum -a 256 --check --ignore-missing SHA256SUMS`; PowerShell: compare `Get-FileHash` with the line in `SHA256SUMS`).
3. Unpack it and put `rnm` (or `rnm.exe`) in a directory on your `PATH`.
4. [Set up your shell](shell-setup.md).

## Build from source

You need Rust 1.99 or later.

```sh
cargo install --locked --git https://github.com/christophersarmiento/rnm
```

This puts `rnm` in `~/.cargo/bin`. Then [set up your shell](shell-setup.md).

## Check the installation

Open a new terminal and run:

```sh
rnm --version
rnm doctor
```

`rnm doctor` checks the install directory, your shell setup and `PATH` order, the `default` alias, npm's prefix setting, other version managers, and the mirror. It exits with 1 when something needs fixing, and says what to do.

## Upgrade

Run the install script again. It replaces the binary and leaves your startup files alone. Add `--skip-shell` (`-SkipShell` on Windows) if you set up your shell by hand.

Upgrading rnm never touches your installed Node.js versions.

## Uninstall

1. Remove the rnm block (it starts with `# rnm`) from your shell startup file or PowerShell profiles.
2. Delete the binary: `~/.local/share/rnm/bin/rnm`, or `%LOCALAPPDATA%\rnm\bin\rnm.exe` (and remove that folder from your user `Path`).
3. On Windows, if you used `rnm default --persist`, run `rnm default --unpersist` first.
4. Optionally delete the data:
   - **macOS and Linux:** the per-shell links in `$XDG_RUNTIME_DIR/rnm` or `~/.local/state/rnm`, the settings in `~/.config/rnm`, and the cache in `$NVM_DIR/.cache/rnm`. Your Node.js versions live in nvm's directory (`~/.nvm` by default), which nvm can keep using; delete it only if you want them gone.
   - **Windows:** `%LOCALAPPDATA%\rnm` (versions, aliases, and links) and `%APPDATA%\rnm` (settings). Versions imported from nvm-windows with links stay in nvm-windows's folder.
