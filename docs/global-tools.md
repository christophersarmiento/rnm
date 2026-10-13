# Global tools

Command-line tools written for Node.js (TypeScript's `tsc`, `eslint`, coding agents such as Pi) run on a `node`. Which one depends on how the tool was installed.

## Tools installed with `npm install -g`

npm installs global packages into the **active version's own folder**, for example `~/.nvm/versions/node/v22.12.0/lib/node_modules`. This is how nvm works too:

- The tool runs on the version it was installed into.
- After `rnm use 20` it is gone, and it comes back when you switch back to 22.
- A newly installed version starts without it.

To carry tools across versions:

- List them in [`default-packages`](configuration.md#default-packages), so every `rnm install` adds them:

  ```text
  # ~/.nvm/default-packages
  typescript
  eslint
  ```

- Copy them from another version when you install a new one:

  ```sh
  rnm install 24 --reinstall-packages-from=22
  ```

- Or copy them into the active version later: `rnm reinstall-packages 22`.

If global packages land somewhere else, npm's `prefix` setting is set; `rnm doctor` warns about it. See [Troubleshooting](troubleshooting.md#global-packages-disappear-after-rnm-use).

## Tools installed outside the version folders

Some tools install themselves somewhere else:

- their own installer (Pi's `curl -fsSL https://pi.dev/install.sh | sh` puts Pi in `~/.pi/agent`)
- `pnpm add -g` or `bun add -g`

These **survive `rnm use`**: you install them once, not per version. But most of them start with `#!/usr/bin/env node`, so they run on **whichever `node` is active**. That is usually what you want. It breaks when a project pins a Node.js older than the tool supports: Pi needs Node.js 22.19 or newer, so inside a project whose `.nvmrc` says `18`, `pi` fails.

### Run a tool on a fixed version

`rnm exec` runs one command with another version's `node` first on `PATH`, without changing the shell. Wrap the tool in an alias or function, and it always runs on that version, whatever the shell or the project uses:

| Shell | Startup file line |
|---|---|
| bash, zsh | `alias pi='rnm exec --silent --using 22 pi'` |
| fish | `alias pi 'rnm exec --silent --using 22 pi'` |
| PowerShell | `function pi { rnm exec --silent --using 22 pi @args }` |
| Nushell | `alias pi = rnm exec --silent --using 22 pi` |

`--using` takes any [version spec](version-files.md#version-specs): `22`, `lts/*`, `lts/jod`, or an alias. An alias of your own keeps every pinned tool on one version that you can move later:

```sh
rnm alias tools 22
alias pi='rnm exec --silent --using tools pi'
```

The cost is one extra rnm process per start of the tool, about 1 to 3 ms.

The tool must still be installed outside the version folders (or in the pinned version itself): `rnm exec` puts the pinned version first on `PATH` and keeps the rest of your `PATH`, so it finds the tool where it is.
