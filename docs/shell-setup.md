# Shell setup

- [What the setup does](#what-the-setup-does)
- [bash](#bash) · [zsh](#zsh) · [fish](#fish) · [PowerShell](#powershell) · [Nushell](#nushell) · [Git Bash on Windows](#git-bash-on-windows)
- [Automatic switching on `cd`](#automatic-switching-on-cd)
- [Completions](#completions)
- [Minimum shell versions](#minimum-shell-versions)

The [install scripts](installation.md) do this for you. Set it up by hand if you installed rnm manually, use another shell, or want to change the setup.

## What the setup does

`rnm env` prints a few lines of shell code. When your shell runs them at startup:

1. rnm creates a link for this shell session, pointing at your `default` version (or at the system Node.js when there is no `default`). Links of shells that have exited are removed.
2. The link's `bin` directory goes to the front of `PATH`, once.
3. `NVM_BIN` and `NVM_INC` point into the link, so tools that read them keep working after every `rnm use`.
4. `RNM_MULTISHELL_PATH` names the link, so later `rnm use` calls know which one to re-point.

From then on, `rnm use` only re-points the link; it never has to change your shell's environment. Running the setup again in the same shell (for example after `source ~/.bashrc`) reuses the link and does not add `PATH` again.

`rnm env` detects the calling shell from the process tree. Pass `--shell` when it runs from somewhere else, such as a script.

## bash

In `~/.bashrc` (on macOS, the login shell reads `~/.bash_profile`):

```sh
eval "$(rnm env --shell bash)"
```

## zsh

In `~/.zshrc`:

```sh
eval "$(rnm env --shell zsh)"
```

## fish

In `~/.config/fish/conf.d/rnm.fish` (or `~/.config/fish/config.fish`):

```fish
rnm env --shell fish | source
```

## PowerShell

PowerShell 7 on any system, and Windows PowerShell 5.1. In your profile (`$PROFILE`, or `$PROFILE.CurrentUserAllHosts` for every host):

```powershell
rnm env --shell powershell | Out-String | Invoke-Expression
```

On Windows, PowerShell runs profiles only when the execution policy allows it. If `rnm doctor` reports `Restricted` or `AllSigned`, run `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`. See [Windows](windows.md).

## Nushell

Nushell cannot evaluate generated code at startup, so save rnm's setup once into Nushell's autoload directory:

```nu
rnm env --shell nu | save -f ($nu.data-dir | path join vendor/autoload/rnm.nu)
```

The saved file runs `rnm env --json | from json | load-env` at every startup and puts the link on `PATH` (`Path` on Windows). Run the command again after changing options such as `--use-on-cd`. `rnm` itself must be on `PATH` in your `env.nu`.

## Git Bash on Windows

Git Bash (MSYS2) uses the bash setup; rnm translates Windows paths to the `/c/Users/...` form:

```sh
eval "$(rnm env --shell bash)"
```

Put it in `~/.bashrc`. Make sure the folder with `rnm.exe` is on `PATH` (the Windows install script adds it to your user `Path`, which Git Bash inherits).

## Automatic switching on `cd`

Add `--use-on-cd` to the setup line:

```sh
eval "$(rnm env --shell bash --use-on-cd)"
```

It follows the recipes in nvm's README:

- At startup and after every directory change, rnm uses the version from the nearest version file in the directory or any parent (`.nvmrc`, `.node-version`, and `package.json` if you [turned it on](version-files.md#packagejson)).
- In a directory without one, it switches back to the `default` alias.
- A manual `rnm use` lasts until the next directory change.
- A version that is not installed is reported, not installed. Add `--install-if-missing` to install it instead:

  ```sh
  eval "$(rnm env --shell zsh --use-on-cd --install-if-missing)"
  ```

- It prints `Now using node …` only when the version actually changes.

How each shell runs the hook:

| Shell | Hook |
|---|---|
| bash | a `cd` alias plus a `PROMPT_COMMAND` check, so `pushd`, `popd`, and other directory changes are caught at the next prompt |
| zsh | `add-zsh-hook chpwd` |
| fish | a function on `--on-variable PWD` |
| Nushell | a tagged closure in `$env.config.hooks.env_change.PWD` |
| PowerShell 7 | `LocationChangedAction`, which covers `Set-Location`, `cd`, `pushd`, and `popd` |
| Windows PowerShell 5.1 and Constrained Language Mode | a wrapper around `prompt` that checks whether the directory changed |

Running the setup again replaces the hook instead of adding a second one. The hook never contacts the network unless you pass `--install-if-missing`.

## Completions

`rnm completions` prints a completion script for rnm's commands and flags. Load it from your startup file, after the `rnm env` line:

| Shell | Line |
|---|---|
| bash | `eval "$(rnm completions --shell bash)"` |
| zsh | `eval "$(rnm completions --shell zsh)"` (after `compinit`) |
| fish | `rnm completions --shell fish \| source` |
| PowerShell | `rnm completions --shell powershell \| Out-String \| Invoke-Expression` |
| Nushell | `rnm completions --shell nu \| save -f ($nu.data-dir \| path join vendor/autoload/rnm-completions.nu)` (once) |

## Minimum shell versions

| Shell | Oldest supported | Tested in CI |
|---|---|---|
| bash | 3.2 (macOS's `/bin/bash`) | 3.2 and current |
| zsh | 5.0 | 5.0.8 and current |
| fish | 3.1 | 3.1.2 and current |
| Nushell | 0.104 | 0.104.0 and current |
| PowerShell | 7.2, and Windows PowerShell 5.1 | 7.2.24, 5.1, and current |

Other shells (POSIX `sh`, dash, ksh, tcsh, xonsh, `cmd.exe`) are not supported yet. Use `rnm exec` from them, or, on Windows, [`rnm default --persist`](windows.md#make-the-default-version-visible-everywhere).
