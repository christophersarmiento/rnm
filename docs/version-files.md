# Version files and version specs

- [Where rnm looks](#where-rnm-looks)
- [`.nvmrc`](#nvmrc)
- [`.node-version`](#node-version)
- [`package.json`](#packagejson)
- [Version specs](#version-specs)
- [npm ranges](#npm-ranges)

## Where rnm looks

`rnm use`, `install`, `exec`, and `which` without a version, and the `--use-on-cd` hook, read the nearest version file:

1. Start in the current directory (the logical `$PWD`, so a symlinked directory is walked by its link path, as nvm does).
2. In each directory, check `.nvmrc`, then `.node-version`, then `package.json` (only if [turned on](#packagejson)). The first one found wins.
3. Otherwise move to the parent directory, up to and including `/`.

So a `.node-version` in a closer directory beats an `.nvmrc` further up, and `.nvmrc` beats `.node-version` in the same directory.

Files may be UTF-8 (with or without a byte order mark) or UTF-16 with a byte order mark, which is what `node -v > .nvmrc` writes in Windows PowerShell 5.1.

## `.nvmrc`

rnm parses `.nvmrc` exactly as nvm does:

- `#` starts a comment, anywhere on a line.
- Whitespace is trimmed, including the `\r` of Windows line endings. Empty lines are ignored.
- A line with `=` is a `key=value` option, which nvm reserves for future use; rnm ignores it. The key `node` is an error, and so is a repeated key.
- Exactly one other line must remain: the version, in the [version grammar](#version-specs).

```sh
# .nvmrc
lts/iron   # pinned for CI
```

`rnm use --save` and `rnm install --save` write the version to `./.nvmrc`.

## `.node-version`

The format nodenv, fnm, and others use: the first word of the first line that is not empty or a comment.

```text
20.11.1
```

## `package.json`

rnm can also read the Node.js version from `package.json`. This is **off by default**, because many projects set `engines` loosely for npm rather than to pick a development version. Turn it on with either:

- the environment variable `RNM_RESOLVE_ENGINES=1` (fnm's `FNM_RESOLVE_ENGINES` also works), or
- [`config.toml`](configuration.md):

  ```toml
  [version_files]
  package_json = true
  ```

rnm reads, in order:

1. [`devEngines.runtime`](https://docs.npmjs.com/cli/configuring-npm/package-json#devengines): the entry whose `name` is `node` (an object, or one entry of an array), and its `version`.
2. `engines.node`.

```json
{
  "devEngines": {
    "runtime": { "name": "node", "version": "^22.12" }
  },
  "engines": { "node": ">=20" }
}
```

A `package.json` with neither is skipped, and the search continues in the parent directory. The value is usually an [npm range](#npm-ranges); rnm picks the newest version that satisfies it, among installed versions for `use` and among the mirror's versions for `install`.

## Version specs

Everything nvm accepts:

| Spec | Means |
|---|---|
| `20`, `v20` | The newest `20.x.y`. |
| `20.11`, `v20.11` | The newest `20.11.y`. |
| `20.11.1`, `v20.11.1` | Exactly that version. Prereleases such as `v24.0.0-rc.1` work too. |
| `node`, `stable` | The newest release. |
| `lts/*` | The newest LTS release. |
| `lts/iron` | The newest release of one LTS line (lowercase). |
| `lts/-1` | The newest release of the LTS line before the newest one; `lts/-2` goes back two lines. |
| `system` | The Node.js on `PATH` outside rnm. |
| `current` | The active version (`which`, `ls`). |
| `iojs`, `iojs-v3.3.1` | Existing io.js installs (rnm does not install io.js). |
| `unstable` | The newest odd `0.x` line, for nvm compatibility. |
| any other name | An alias, such as `default` or one you created with `rnm alias`. |

Partial versions match whole components: `1` never matches `v18.0.0`. `latest` is not a keyword, as in nvm; it is an ordinary alias name.

For installed versions (`use`, `exec`, `which`), `lts/*` and `lts/<name>` are read from nvm's alias files, which `install` and `ls-remote` refresh from the mirror.

## npm ranges

A spec written like an [npm semver range](https://github.com/npm/node-semver#ranges) resolves to the newest matching version. rnm treats a spec as a range when it contains an operator (`<`, `>`, `=`, `^`, `~`, `||`), a space, or an `x`/`*` component:

| Range | Matches |
|---|---|
| `^20.1` | `>=20.1.0 <21.0.0` |
| `~18.2` | `>=18.2.0 <18.3.0` |
| `20.x`, `20.*` | any `20.y.z` |
| `>=18 <21` | 18, 19, or 20 |
| `18 - 20` | `>=18.0.0 <21.0.0` |
| `^18 \|\| ^20` | 18 or 20 |

Prereleases match only when the range names a prerelease of the same version, as in npm. Ranges work on the command line too (`rnm install '>=20 <22'`), but an alias cannot point at one, so nvm can read every alias file.
