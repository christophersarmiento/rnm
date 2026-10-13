# Configuration

- [Where things live](#where-things-live)
- [`config.toml`](#configtoml)
- [Mirrors](#mirrors)
- [Credentials](#credentials)
- [Certificates and proxies](#certificates-and-proxies)
- [Builds for musl, riscv64, and loong64](#builds-for-musl-riscv64-and-loong64)
- [`default-packages`](#default-packages)
- [Environment variables](#environment-variables)

rnm needs no configuration. Everything here is optional.

Settings come from four places. Each one overrides the ones before it:

1. built-in defaults
2. `config.toml`
3. environment variables (`RNM_*` first, then nvm's `NVM_*` and fnm's `FNM_*` names)
4. command-line flags

## Where things live

| | macOS and Linux | Windows |
|---|---|---|
| Installed versions, aliases, download cache | nvm's directory: `$RNM_DIR`, else `$NVM_DIR`, else `$XDG_CONFIG_HOME/nvm`, else `~/.nvm` | `$RNM_DIR`, else `%LOCALAPPDATA%\rnm` |
| Per-shell links | `$XDG_RUNTIME_DIR/rnm/multishells`, else `$XDG_STATE_HOME/rnm/multishells`, else `~/.local/state/rnm/multishells` | `<install dir>\multishells` |
| Settings | `$XDG_CONFIG_HOME/rnm/config.toml`, else `~/.config/rnm/config.toml` | `%APPDATA%\rnm\config.toml` |
| Version-list cache | `<install dir>/.cache/rnm` | `<install dir>\.cache\rnm` |

Inside the install directory, rnm uses nvm's layout:

```text
versions/node/v20.11.1/   one directory per version
alias/default             one-line alias files
alias/lts/iron            LTS aliases, refreshed from the mirror
.cache/bin/<name>/        downloaded archives, shared with nvm
.cache/locks/             install locks, shared with nvm
default-packages          global npm packages for new versions
```

On Windows, `node.exe` sits in the version directory itself instead of in `bin\`, as in Node.js's Windows archives.

## `config.toml`

rnm reads one settings file, from your own configuration directory, never from a project: a cloned repository must not be able to change where rnm downloads from or capture your credentials. `RNM_CONFIG` names a different file.

Unknown keys are errors that name the line, so a typo cannot silently do nothing. A broken file makes every rnm command fail with that message.

A complete example:

```toml
[mirror]
url = "https://artifactory.corp/artifactory/nodejs-dist/"
fallback = ["https://nodejs.org/dist"]
unofficial_builds = false
# unofficial_url = "https://unofficial-builds.nodejs.org/download/release"

[[auth]]
prefix = "https://artifactory.corp/artifactory/"
value_env = "ARTIFACTORY_TOKEN"
scheme = "Bearer"

[http]
netrc = true
# netrc_file = "~/.netrc"
# ca_file = "/etc/ssl/corp-root.pem"
allow_insecure_http_auth = false

[version_files]
package_json = false
```

| Key | Default | Meaning |
|---|---|---|
| `mirror.url` | `https://nodejs.org/dist` | The mirror. It must be laid out like nodejs.org: `index.json`, and `vX.Y.Z/SHASUMS256.txt` plus the archives. |
| `mirror.fallback` | `[]` | Mirrors to try, in order, when one fails. See [Mirrors](#mirrors). |
| `mirror.unofficial_builds` | `false` | On musl, riscv64, and loong64 Linux, use unofficial-builds.nodejs.org instead of the default mirror. |
| `mirror.unofficial_url` | unofficial-builds.nodejs.org | Another mirror of the unofficial builds. |
| `[[auth]]` | none | Credentials for URL prefixes. See [Credentials](#credentials). |
| `http.netrc` | `true` | Read credentials from netrc. |
| `http.netrc_file` | `~/.netrc` (`%USERPROFILE%\_netrc` or `.netrc` on Windows) | The netrc file. |
| `http.ca_file` | the system's certificate store | PEM certificates to trust instead. |
| `http.allow_insecure_http_auth` | `false` | Send credentials over plain `http` to hosts other than localhost. |
| `version_files.package_json` | `false` | Read [`package.json`](version-files.md#packagejson). |

A leading `~` in a path means your home directory.

## Mirrors

The main mirror comes from the first of:

1. `RNM_NODE_MIRROR`, `NVM_NODEJS_ORG_MIRROR`, or `FNM_NODE_DIST_MIRROR`
2. `mirror.url`
3. unofficial-builds, when [turned on and needed](#builds-for-musl-riscv64-and-loong64)
4. `https://nodejs.org/dist`

Fallback mirrors come from `RNM_NODE_MIRROR_FALLBACK` (separated by commas or spaces), else `mirror.fallback`. rnm moves on to the next mirror when one:

- cannot be reached (network, DNS, TLS, or proxy failure)
- answers 404, 429, or 5xx
- serves something that is not a version list
- lacks the archive or its checksum

It prints `warning: …; trying the next mirror` each time. A **401 or 403 never falls back**: a credential problem needs fixing, and quietly downloading from somewhere else would hide it. Downloads start with the mirror that served the version list, and every archive is checked against the `SHASUMS256.txt` of the mirror it came from.

If no mirror can be reached at all, `install` and `ls-remote` use the last version list they saw, with a warning. An exact version that is already installed needs no network, and `install --offline` never uses it.

## Credentials

rnm sends a credential only to URLs under the prefix it belongs to. It follows redirects itself and checks every hop, so a mirror that redirects to a CDN (Artifactory does) never passes your token on. Credentials never travel over plain `http` except to localhost, and they never appear in output, errors, or `rnm env`.

When several credentials cover a URL, the first in this list wins:

1. **`RNM_AUTH_HEADER`** (or nvm's `NVM_AUTH_HEADER`, or fnm's `FNM_AUTH_HEADER`): a full `Authorization` value such as `Bearer abc123`, sent to the main mirror only.
2. **A user name and password in a mirror URL**, such as `https://me:secret@mirror.corp/node/`. rnm sends them as HTTP Basic and removes them from the URL.
3. **`[[auth]]` entries** in `config.toml`. The entry with the longest matching `prefix` wins.
4. **netrc** entries, by host (https only). The `default` entry is ignored, because it would send a password to every host. Turn netrc off with `http.netrc = false` or `RNM_NETRC=0`.

An `[[auth]]` entry has a `prefix`, optionally a `header` (default `Authorization`) and a `scheme` put in front of the value (`Bearer`), and exactly one source for the value:

```toml
# A token from an environment variable (preferred)
[[auth]]
prefix = "https://artifactory.corp/artifactory/"
value_env = "ARTIFACTORY_TOKEN"
scheme = "Bearer"

# A custom header (GitLab)
[[auth]]
prefix = "https://gitlab.corp/api/v4/projects/42/packages/generic/"
header = "PRIVATE-TOKEN"
command = ["glab", "auth", "token"]

# A credential helper for short-lived tokens (AWS CodeArtifact)
[[auth]]
prefix = "https://corp-123456789012.d.codeartifact.us-east-1.amazonaws.com/"
scheme = "Bearer"
command = ["aws", "codeartifact", "get-authorization-token", "--domain", "corp",
           "--query", "authorizationToken", "--output", "text"]

# HTTP Basic (Nexus)
[[auth]]
prefix = "https://nexus.corp/repository/nodejs/"
username = "builder"
password_env = "NEXUS_PASSWORD"

# A literal value; rnm warns if other users can read config.toml
[[auth]]
prefix = "https://mirror.corp/"
value = "Bearer abc123"
```

| Key | Meaning |
|---|---|
| `prefix` | An `https` URL; the entry covers every URL under it. |
| `header` | The header name. Default: `Authorization`. |
| `scheme` | Put in front of the value with a space. |
| `value` | The value itself. |
| `value_env` | An environment variable that holds the value. |
| `command` | A program and its arguments that print the value. It runs at most once per rnm command, and only when a request under `prefix` needs it. |
| `username` with `password` or `password_env` | Sent as HTTP Basic. |

When a mirror answers 401 or 403, the error says which credential was sent, by its source (never its value), or that none was.

`rnm doctor` lists every credential source and makes one request to the mirror, so it is a quick way to check a new setup.

## Certificates and proxies

rnm checks certificates against the operating system's certificate store, so corporate root certificates that IT installed work without settings. To trust a specific file instead (as curl's `--cacert` does), set `http.ca_file` or `RNM_CA_FILE` to a PEM file.

Proxies come from `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, and `NO_PROXY` (upper or lower case). Proxy credentials go in the proxy URL.

## Builds for musl, riscv64, and loong64

nodejs.org publishes no binaries for Linux with musl libc (Alpine) on most versions, or for riscv64 and loong64. [unofficial-builds.nodejs.org](https://unofficial-builds.nodejs.org) does. To use it automatically on those systems:

```toml
[mirror]
unofficial_builds = true
```

or `RNM_UNOFFICIAL_BUILDS=1`. It only replaces the default mirror; a mirror you set yourself always wins. Without it, rnm explains the switch when a version has no binary for your system.

rnm detects musl from `/etc/alpine-release` or musl's loader in `/lib`.

## `default-packages`

`<install dir>/default-packages` lists global npm packages to install into every new version, as in nvm:

```text
# ~/.nvm/default-packages
typescript
@antfu/ni
npm-check-updates@17
```

- One package spec per line; anything `npm install` accepts.
- Lines starting with `#` and empty lines are skipped.
- A line with a space in it is an error, as in nvm (so no comments after a package name).

`rnm install --skip-default-packages` skips the file.

## Environment variables

| Variable | Meaning |
|---|---|
| `RNM_DIR` | The install directory. Wins over `NVM_DIR`. |
| `NVM_DIR` | nvm's install directory (macOS and Linux only). |
| `RNM_CONFIG` | Read this settings file instead of the default one. |
| `RNM_NODE_MIRROR`, `NVM_NODEJS_ORG_MIRROR`, `FNM_NODE_DIST_MIRROR` | The main mirror. |
| `RNM_NODE_MIRROR_FALLBACK` | Fallback mirrors, separated by commas or spaces. |
| `RNM_AUTH_HEADER`, `NVM_AUTH_HEADER`, `FNM_AUTH_HEADER` | The `Authorization` value for the main mirror. |
| `RNM_UNOFFICIAL_BUILDS` | `1` or `0`: use unofficial-builds on musl, riscv64, and loong64. |
| `RNM_NETRC` | `0` turns netrc off. |
| `NETRC` | The netrc file. |
| `RNM_CA_FILE` | Trust the certificates in this PEM file instead of the system's. |
| `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY` | Proxies. |
| `RNM_RESOLVE_ENGINES`, `FNM_RESOLVE_ENGINES` | `1`: read `package.json` as a version file. |
| `NVM_SYMLINK_CURRENT` | `true`: keep `<install dir>/current` pointing at the active version (macOS and Linux), as nvm does. |
| `NVM_INSTALL_LOCK_TIMEOUT` | Seconds to wait for another install of the same version. Default: 600. |
| `NVM_INSTALL_LOCK_STALE` | Minutes after which an abandoned install lock is taken over. Default: never. |
| `NVM_NO_PROGRESS` | Any value: no download progress bar. |
| `NVM_COLORS` | Five color letters for `ls`, as in nvm (default `bygre`). |
| `TERM=dumb` | No colors. |

`rnm env` sets these in your shell:

| Variable | Meaning |
|---|---|
| `RNM_MULTISHELL_PATH` | This shell's link. |
| `NVM_BIN` | The active version's `bin` directory (through the link). |
| `NVM_INC` | The active version's headers, `include/node` (through the link). |
