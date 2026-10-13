# Commands

rnm uses nvm's command and flag names, with fnm's names as aliases where they differ. `rnm help <command>` (or `rnm <command> --help`) shows the same information.

- Versions: [`install`](#install) · [`uninstall`](#uninstall) · [`use`](#use) · [`current`](#current) · [`ls`](#ls) · [`ls-remote`](#ls-remote) · [`which`](#which) · [`exec`](#exec)
- Aliases: [`alias`](#alias) · [`unalias`](#unalias) · [`default`](#default)
- npm packages: [`reinstall-packages`](#reinstall-packages) · [`install-latest-npm`](#install-latest-npm)
- Setup: [`env`](#env) · [`completions`](#completions) · [`doctor`](#doctor) · [`import nvm-windows`](#import-nvm-windows)
- [Exit codes](#exit-codes)

Wherever a command takes a `VERSION`, you can write anything from the [version grammar](version-files.md#version-specs): `20`, `v20.11`, `20.11.1`, `lts/*`, `lts/iron`, `lts/-1`, `node`, `system`, an alias, or an npm range such as `^20.1`.

## install

**Alias:** `i`

```sh
rnm install [VERSION] [options]
```

Downloads a version, verifies it against the mirror's `SHA256SUMS`, installs it, and switches this shell to it. Without a version, it installs the one in the nearest [version file](version-files.md). The first install also creates the `default` alias.

An exact version that is already installed needs no network.

| Option | Effect |
|---|---|
| `--lts[=NAME]` | The newest LTS release, or the newest of one LTS line (`--lts=iron`). |
| `--default` | Point the `default` alias at the requested version. |
| `--alias=NAME` | Point another alias at it. |
| `-w`, `--save` | Write the installed version to `./.nvmrc`. |
| `--no-use` | Install without switching this shell. |
| `--offline` | Use no network: pick among installed versions and archives in the download cache. |
| `--latest-npm` | Upgrade npm to the newest version that works with this Node.js. |
| `--skip-default-packages` | Do not install the packages from the [`default-packages`](configuration.md#default-packages) file. |
| `--reinstall-packages-from=VERSION` | Reinstall the global npm packages of another installed version (or `system`). Also `--copy-packages-from=`. |
| `--no-progress` | No progress bar. Also `NVM_NO_PROGRESS=1`. |

Like nvm, aliases keep what you typed: `rnm install 22 --default` stores `22`, so `default` follows new 22.x releases as you install them. A range from `package.json` is the exception: it is stored as the version it resolved to, because nvm cannot read ranges.

After the install, rnm runs these steps in nvm's order: switch this shell, `--latest-npm`, `default-packages`, `--reinstall-packages-from`. They also run when the version was already installed.

```sh
rnm install 22
rnm install --lts --default
rnm install 20 --reinstall-packages-from=18
rnm install --offline 20
```

## uninstall

```sh
rnm uninstall VERSION
rnm uninstall --lts[=NAME]
```

Removes an installed version and every alias that points at exactly that version. The downloaded archive stays in the cache. The active version cannot be removed; switch away first. On Windows, a version that a running program (node, npm, an editor) still uses is reported instead of half-removed.

A version imported from nvm-windows as a link is unlinked; nvm-windows's files stay.

## use

```sh
rnm use [VERSION] [options]
```

Switches this shell to an installed version. Without a version, it uses the nearest version file and prints which file it found.

| Option | Effect |
|---|---|
| `--lts[=NAME]` | The newest installed LTS, or of one LTS line. |
| `--silent` | Print nothing on success. |
| `--silent-if-unchanged` | Print nothing if this version is already active. |
| `-w`, `--save` | Write the version to `./.nvmrc`. |
| `--install-if-missing` | Install the version first if needed. |

`rnm use system` switches to the Node.js found on `PATH` outside rnm.

`use` needs the [shell setup](shell-setup.md); without it, rnm says how to add it.

## current

Prints the active version: `v20.11.1`, `system`, or `none`.

## ls

**Alias:** `list`

```sh
rnm ls [PATTERN] [--no-alias] [--no-colors]
```

Lists installed versions (the active one marked with `->`), then aliases. A pattern (`18`, `v20.1`, `iojs`, `system`, an alias, or a range) shows only the matching versions, and prints `N/A` with exit code 3 when nothing matches.

Colors follow nvm: `NVM_COLORS` (five letters, default `bygre`), `--no-colors`, and no colors when the output is not a terminal or `TERM=dumb`.

## ls-remote

**Alias:** `list-remote`

```sh
rnm ls-remote [PATTERN] [options]
```

Lists the versions on the mirror, oldest first, marking installed ones. It also refreshes nvm's LTS alias files (`lts/*`, `lts/<name>`), which `use lts/...` reads.

| Option | Effect |
|---|---|
| `--lts[=NAME]` | Only LTS releases, or one LTS line. |
| `--filter VERSION` | fnm's spelling of `PATTERN`. |
| `--sort asc\|desc` | Oldest first (default) or newest first. |
| `--latest` | Only the newest match. |
| `--no-colors` | Never color the output. |

The version list is cached in `$NVM_DIR/.cache/rnm` and revalidated with an `ETag`, so a repeat call downloads nothing when the list has not changed. If the mirror is unreachable, the cached list is used with a warning.

## which

```sh
rnm which [VERSION] [--silent]
```

Prints the path of a version's `node`. `current` means the active version and `system` the Node.js outside rnm. Without a version, it uses the nearest version file.

## exec

```sh
rnm exec [VERSION] COMMAND [ARGS...]
rnm exec --using VERSION COMMAND [ARGS...]
```

Runs one command with a version's `bin` directory first on `PATH` and `NVM_BIN`/`NVM_INC` set, without changing the shell. The first argument counts as the version only when it names an installed version and a command follows it, so `rnm exec node app.js` runs `node`. Without a version, the nearest version file decides. `--silent` hides the `Running node …` line. The exit code is the command's.

```sh
rnm exec 18 node --version
rnm exec --using lts/iron npm test
```

## alias

```sh
rnm alias                 # list every alias
rnm alias NAME            # aliases starting with NAME
rnm alias NAME VERSION    # create or change an alias
```

Aliases are files in `$NVM_DIR/alias`, shared with nvm. A target may be a version, an LTS name, or another alias; chains are followed and loops are reported. `rnm alias NAME ""` deletes the alias. Aliases cannot point at npm ranges.

## unalias

```sh
rnm unalias NAME
```

Deletes an alias and prints the command that restores it.

## default

```sh
rnm default               # show the default alias
rnm default VERSION       # the same as: rnm alias default VERSION
rnm default --persist     # Windows: make the default version visible to every program
rnm default --unpersist   # Windows: undo --persist
```

New shells start with the `default` version. On Windows, `--persist` adds a link to it to your user `Path`, so editors and other programs started from Explorer find `node` too. See [Windows](windows.md#make-the-default-version-visible-everywhere).

## reinstall-packages

**Alias:** `copy-packages`

```sh
rnm reinstall-packages VERSION
```

Installs the global npm packages of `VERSION` into the active version: registry packages as `name@version`, and `npm link` again for linked packages. npm and corepack are skipped, because they come with Node.js.

## install-latest-npm

Upgrades the active version's npm to the newest release that supports it, following nvm's compatibility table (for example npm 9 on Node.js 20.0–20.4, npm 10 on 22.0–22.8).

## env

```sh
rnm env [--shell SHELL] [--use-on-cd [--install-if-missing]] [--json]
```

Prints the shell code that sets up rnm. See [Shell setup](shell-setup.md). `--json` prints the variables instead, for tools that are not shells.

## completions

```sh
rnm completions [--shell SHELL]
```

Prints a completion script. See [Shell setup](shell-setup.md#completions).

## doctor

Checks the setup and says what to fix:

- the install directory and the installed versions
- this shell's setup, the active version, and whether rnm's link comes first on `PATH`
- the `default` alias
- npm's `prefix` setting (`.npmrc`, `NPM_CONFIG_PREFIX`), which breaks per-version global packages
- nvm, fnm, Volta, or nvm-windows active in the same shell
- the mirror: the settings, every credential source (never the values), fallback mirrors, and one request for the version list
- on Windows: the PowerShell execution policy

It exits with 1 when a check fails. Warnings alone exit with 0.

## import nvm-windows

```sh
rnm import nvm-windows [--link | --move | --copy] [--from DIR]
```

Adopts the versions nvm-windows installed, its aliases, and its active version (as `default`, if you have none). See [Windows](windows.md#import-from-nvm-windows).

## Exit codes

rnm keeps nvm's exit codes, so scripts that check them keep working.

| Code | Meaning |
|---|---|
| 0 | Success. |
| 1 | General error: a download, verification, file system, or settings problem. |
| 2 | Invalid command-line arguments. Also `reinstall-packages` from the active version. |
| 3 | The version is not installed, or does not exist on the mirror. |
| 4 | `--reinstall-packages-from` names the version being installed. |
| 5 | `--reinstall-packages-from` names a version that is not installed. |
| 8 | An alias chain loops. |
| 55 | An option that is not supported in this combination (`ls --no-alias` with a pattern). |
| 126 | `exec` could not start the command. |
| 127 | No version given and no version file found. |
