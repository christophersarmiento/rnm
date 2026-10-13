# Windows

- [Install](#install)
- [Where rnm keeps things](#where-rnm-keeps-things)
- [Shells](#shells)
- [Windows on Arm](#windows-on-arm)
- [Import from nvm-windows](#import-from-nvm-windows)
- [Make the default version visible everywhere](#make-the-default-version-visible-everywhere)
- [Common problems](#common-problems)

rnm runs natively on Windows 10 and 11, x64 and Arm64. It needs no administrator rights, no Developer Mode, and no shims.

## Install

```powershell
irm https://raw.githubusercontent.com/christophersarmiento/rnm/main/install.ps1 | iex
```

See [Installation](installation.md#install-script-windows) for the options.

## Where rnm keeps things

| | Path |
|---|---|
| rnm itself | `%LOCALAPPDATA%\rnm\bin\rnm.exe` (install script default) |
| Versions, aliases, download cache | `%LOCALAPPDATA%\rnm`, or `RNM_DIR` |
| Per-shell links | `%LOCALAPPDATA%\rnm\multishells` |
| Settings | `%APPDATA%\rnm\config.toml` |

The tree has nvm's layout (`versions\node\vX.Y.Z`, `alias\`, `.cache\`), except that `node.exe`, `npm.cmd`, and global packages live in the version folder itself, as in Node.js's Windows zip. rnm never reads `NVM_DIR` or `NVM_HOME` on Windows; those belong to nvm (which does not run on Windows) and nvm-windows.

Each shell session gets its own **directory junction** to the active version. `rnm use` re-points only that junction, so other terminals are not affected, and nothing needs administrator rights. Junctions of closed shells are removed the next time a shell starts.

## Shells

- **PowerShell 7** and **Windows PowerShell 5.1** are both supported. Everything rnm prints for PowerShell is ASCII, because 5.1 reads program output in the console's code page. With `--use-on-cd`, PowerShell 7 uses `LocationChangedAction`; 5.1 and Constrained Language Mode (AppLocker, WDAC) use a `prompt` wrapper instead.
- **Git Bash** uses the bash setup. rnm writes paths in Git Bash's `/c/Users/...` form.
- **Nushell** works as on other systems; rnm treats `Path` as a list and matches its name in any case.
- **cmd.exe** is not supported yet. Use [`rnm default --persist`](#make-the-default-version-visible-everywhere) to give it (and every other program) your default version.

See [Shell setup](shell-setup.md).

### Execution policy

Windows PowerShell does not run your profile, and so does not set up rnm, under the `Restricted` (the default on Windows 10 and 11 clients) or `AllSigned` execution policies. `rnm doctor` checks this. To fix it:

```powershell
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
```

## Windows on Arm

rnm installs Arm64 Node.js on Arm64 PCs, even when an x64 build of rnm runs under emulation. Node.js publishes Arm64 Windows builds from 19.9.0; for older versions rnm installs the x64 build, which Windows runs under emulation.

## Import from nvm-windows

```powershell
rnm import nvm-windows
```

rnm finds nvm-windows's versions through its settings (the `InstallRoot` registry value of nvm-windows 2, `root:` in `%NVM_HOME%\settings.txt` of nvm-windows 1, or the default folders), and adds each `vX.Y.Z` folder to its own tree:

| Option | Effect |
|---|---|
| `--link` (default) | A junction to nvm-windows's folder. Nothing is copied, and nvm-windows keeps working. |
| `--copy` | A full copy. Slow (thousands of files), but independent of nvm-windows. |
| `--move` | Moves the folder (same drive only). Fast, but nvm-windows loses the version. |
| `--from DIR` | Read versions from `DIR` instead of searching. |

It also imports nvm-windows's aliases, and its active version as `default`, unless rnm already has an alias of that name. Versions rnm already has are skipped, so running it again is safe.

rnm never changes nvm-windows's settings, `PATH`, or files. `rnm uninstall` on a linked version removes only the junction.

Once rnm works, turn nvm-windows off (or uninstall it) so its `PATH` entry does not compete with rnm's; `rnm doctor` warns when nvm-windows is still active.

## Make the default version visible everywhere

Programs started from Explorer or the Start menu, such as editors, never run a shell profile, so they do not see rnm's per-shell link. To give them your default version:

```powershell
rnm default --persist
```

This:

1. creates the junction `%LOCALAPPDATA%\rnm\default`, pointing at the version your `default` alias names
2. adds that folder once to the front of your user `Path` (the raw registry value, so entries such as `%USERPROFILE%\bin` stay as they are)
3. tells running programs that the environment changed

Restart open editors and terminals to pick it up. From then on, every change of the `default` alias (`rnm default 22`, `rnm alias default 22`, `rnm install --default`) re-points the junction. Shells set up with `rnm env` still put their own link first, so `rnm use` works as usual there.

`rnm default --unpersist` removes the junction and the `Path` entry.

A `nodejs` folder on the **system** `Path` (from the official installer) comes before every user `Path` entry; uninstall that Node.js, or remove its entry, if programs still find the wrong one.

## Common problems

- **`rnm uninstall` says the version is in use.** A running program (node, npm, an editor's language server) has files open in that folder. Close it and try again; rnm does not leave a half-removed version behind.
- **`rnm use` has no effect in a new terminal.** The profile did not run: check the execution policy, and that the rnm block is in the profile of the PowerShell you use (Windows PowerShell 5.1 and PowerShell 7 have separate profiles).
- **`node` is still nvm-windows's.** nvm-windows's `NVM_SYMLINK` folder may be on the system `Path`, which comes before your user `Path`. `rnm doctor` warns when nvm-windows is active; turn it off or remove that entry.
