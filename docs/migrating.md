# Migrating from nvm, fnm, and nvm-windows

- [From nvm](#from-nvm)
- [From fnm](#from-fnm)
- [From nvm-windows](#from-nvm-windows)
- [Differences from nvm](#differences-from-nvm)

## From nvm

There is nothing to migrate. rnm uses nvm's directory (`$NVM_DIR`, default `~/.nvm`) and reads and writes it exactly as nvm does: the same version folders, alias files, LTS aliases, download cache, install locks, and `default-packages`. Your `.nvmrc` files work unchanged.

1. [Install rnm](installation.md).
2. In your shell startup file, remove (or comment out) nvm's lines:

   ```sh
   export NVM_DIR="$HOME/.nvm"
   [ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"
   [ -s "$NVM_DIR/bash_completion" ] && \. "$NVM_DIR/bash_completion"
   ```

   Keep `export NVM_DIR=…` if your versions live somewhere other than `~/.nvm`.
3. If you used a `cdnvm` or `load-nvmrc` recipe from nvm's README, remove it and use `--use-on-cd` instead:

   ```sh
   eval "$(rnm env --shell bash --use-on-cd)"
   ```

4. Open a new terminal and run `rnm doctor`.

Most commands are the same: replace `nvm` with `rnm`.

| nvm | rnm |
|---|---|
| `nvm install 20`, `nvm install --lts`, `nvm install node` | same |
| `nvm use`, `nvm use 20 --silent`, `nvm use --lts` | same |
| `nvm alias default 20` | same, or `rnm default 20` |
| `nvm ls`, `nvm ls-remote --lts`, `nvm current`, `nvm which 20` | same |
| `nvm exec 20 node app.js` | same |
| `nvm run 20 app.js` | `rnm exec 20 node app.js` |
| `nvm reinstall-packages 18`, `nvm install-latest-npm` | same |
| `nvm install 20 --reinstall-packages-from=18 --latest-npm` | same |
| `nvm deactivate` | `rnm use system` |
| `nvm cache clear` | delete `$NVM_DIR/.cache/bin` |
| `nvm version-remote 20` | `rnm ls-remote 20 --latest` |
| `nvm unload` | remove the setup line and open a new shell |

You can switch back to nvm at any time: re-add its lines. Both tools can even share the tree at the same time, though in a shell where both are loaded, whichever changed `PATH` last wins.

## From fnm

rnm accepts fnm's spellings where they differ from nvm's:

| fnm | rnm |
|---|---|
| `fnm env --use-on-cd` | same |
| `fnm use --install-if-missing` | same |
| `fnm list`, `fnm list-remote` | same (also `ls`, `ls-remote`) |
| `fnm list-remote --filter 20 --sort desc --latest --lts` | same |
| `fnm exec --using 20 node app.js` | same |
| `fnm default 20` | same |
| `fnm completions --shell zsh` | same |
| `FNM_NODE_DIST_MIRROR` | read; `RNM_NODE_MIRROR` is rnm's own name |
| `FNM_RESOLVE_ENGINES` | read; turns on `package.json` (off by default in rnm) |
| `--version-file-strategy recursive` | always: rnm searches parent directories, as nvm does |

fnm keeps its versions in its own directory, which rnm does not read. Install the versions you need again with rnm (`rnm install 20`), then:

1. Replace `eval "$(fnm env …)"` in your startup file with `eval "$(rnm env --shell <shell> …)"`, keeping options such as `--use-on-cd`.
2. Open a new terminal and run `rnm doctor`.
3. When everything works, remove fnm and its directory (`~/.local/share/fnm`, `~/Library/Application Support/fnm`, or `%APPDATA%\fnm`).

Version files work the same: `.nvmrc` and `.node-version`. Unlike fnm, rnm also understands everything nvm allows in `.nvmrc`, including comments, `lts/-1`, `node`, and aliases.

## From nvm-windows

```powershell
rnm import nvm-windows
```

This links your existing versions into rnm's tree without copying them, and imports your aliases and active version. See [Windows](windows.md#import-from-nvm-windows). Then:

1. Turn nvm-windows off (`nvm off`) or uninstall it, so its `PATH` entry does not compete with rnm's.
2. If editors and other programs need `node` outside a terminal, run `rnm default --persist`.
3. Open a new terminal and run `rnm doctor`.

Linked versions keep living in nvm-windows's folder. If you plan to uninstall nvm-windows completely, import with `--copy` (or `--move`) instead, so the versions do not disappear with it.

## Differences from nvm

rnm aims to behave like nvm wherever scripts can see it. The intentional differences:

- **Speed and isolation.** `rnm use` re-points a per-shell link instead of rewriting `PATH`. nvm loaded in the same shell will not recognise rnm's `PATH` entry; it still works, but adds its own entry in front.
- **Checksums are always verified.** nvm skips the check when it finds no checksum tool; rnm refuses to install anything it cannot verify.
- **No building from source.** rnm installs prebuilt binaries only. For systems nodejs.org does not cover, use [unofficial builds](configuration.md#builds-for-musl-riscv64-and-loong64).
- **No io.js installs.** Existing io.js versions in the tree are still listed and usable.
- **`.node-version` is read too**, and `package.json` on request. nvm reads only `.nvmrc`.
- **npm ranges** such as `^20.1` are accepted as versions. nvm rejects them.
- **Credentials are scoped.** `NVM_AUTH_HEADER` is sent only to the configured mirror, never to hosts it redirects to. nvm strips characters it does not allow from the value; rnm rejects values that are not valid in a header, so typos are not silently changed.
