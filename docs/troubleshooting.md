# Troubleshooting

Start with:

```sh
rnm doctor
```

It checks the install directory, your shell setup and `PATH` order, the `default` alias, npm's prefix, other version managers in the same shell, and the mirror (one request), and says what to fix.

## "rnm is not set up in this shell"

`rnm use` needs the line from [Shell setup](shell-setup.md) in your startup file. Add it, then open a new terminal. On macOS, bash login shells read `~/.bash_profile`, not `~/.bashrc`. On Windows, check the [execution policy](windows.md#execution-policy).

## `node` is not the version `rnm current` shows

Something later in your startup file puts another `node` first on `PATH`: nvm, fnm, Volta, Homebrew's `node`, or a `PATH` line added after rnm's. Move the rnm line to the end of the file. `rnm doctor` names the other version managers it sees.

## `N/A: version "20" is not yet installed`

`use` only switches to installed versions. Run `rnm install 20`, or `rnm use --install-if-missing 20`. With `--use-on-cd`, add `--install-if-missing` to the setup line to install versions from version files automatically.

## A version file is rejected

`rnm use` prints why, for example `file has more than one version line`. `.nvmrc` follows nvm's rules: one version line, `#` comments, optional `key=value` lines. See [Version files](version-files.md).

## Downloads fail

- **`401 Unauthorized` or `403 Forbidden`**: the error says which credential was sent (by its source, such as `RNM_AUTH_HEADER`), or that none was. See [Credentials](configuration.md#credentials).
- **Certificate errors behind a corporate proxy**: rnm uses the system's certificate store. If your company's root certificate is not installed there, set `http.ca_file` or `RNM_CA_FILE` to a PEM file. See [Certificates and proxies](configuration.md#certificates-and-proxies).
- **`has no prebuilt binary for this platform (linux-x64-musl)`**: on Alpine and other musl systems, turn on [unofficial builds](configuration.md#builds-for-musl-riscv64-and-loong64).
- **`checksum mismatch`**: the download was corrupted or changed on the way. Try again; if it persists, the mirror serves a broken file.
- **No network**: `rnm install --offline 20` installs from the download cache, which nvm and rnm share.

## Global packages disappear after `rnm use`

That is by design: each version has its own global packages, as in nvm. Copy them with `rnm install 22 --reinstall-packages-from=20` or `rnm reinstall-packages 20`, and list packages for every new version in [`default-packages`](configuration.md#default-packages).

If global packages land outside the version folder, npm's `prefix` is set (in `~/.npmrc` or `NPM_CONFIG_PREFIX`); `rnm doctor` warns about it. Remove the setting.

## Too many links in the multishell directory

rnm removes the links of shells that have exited every time a shell starts. Links of running shells stay. The directory is `$XDG_RUNTIME_DIR/rnm/multishells` or `~/.local/state/rnm/multishells` (`%LOCALAPPDATA%\rnm\multishells` on Windows), and deleting it while no shell uses rnm is safe.

## Reporting a bug

Open an issue at <https://github.com/christophersarmiento/rnm/issues> with the output of `rnm --version` and `rnm doctor`, your shell and its version, and the commands that fail. Remove any credentials from the output first (rnm never prints them, but your shell history might contain them).
