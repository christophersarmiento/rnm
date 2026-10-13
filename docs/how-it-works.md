# How rnm works

## Switching versions: one link per shell

nvm rewrites `PATH` in every `nvm use`, which is why it must be a shell function sourced into every shell (about 5,600 lines of shell script). rnm uses the design fnm introduced instead:

1. At startup, `rnm env` creates a link for the shell session and puts `<link>/bin` at the front of `PATH`, once.
2. `rnm use` re-points the link. The shell's environment never changes again, so `rnm` can be an ordinary program, and the `cd` hook is a single command.

```text
PATH=~/.local/state/rnm/multishells/4242_1712345678/bin:/usr/bin:…
                                    │
                                    └─▶ ~/.nvm/versions/node/v20.11.1
```

What rnm does differently from fnm:

- On unix, the new link is created next to the old one and renamed over it, so another process never sees a missing link.
- On Windows, the link is a directory junction, which needs no administrator rights or Developer Mode.
- Links are named after the shell's process ID and creation time, and links of shells that have exited are removed when a shell starts. fnm never removes them.
- Re-running the setup in the same shell reuses the link and does not add a second `PATH` entry.
- Every value in the generated shell code goes through that shell's own quoting, so paths with spaces, quotes, `$`, or non-ASCII characters are safe. CI runs the code in real shells on paths like that.
- `NVM_BIN` and `NVM_INC` point through the link, so they stay correct after every `use`.

## Startup cost

A shell startup runs `rnm env` once: no subshells, no `ps`, no network. The calling shell is found by reading the process table directly (`/proc` on Linux, `proc_pidinfo` on macOS, one Toolhelp snapshot on Windows). With `--use-on-cd`, the version file at startup is handled in the same process.

`rnm use`, `env`, and the `cd` hook never touch the network. `install` with an exact version that is already installed does not either.

## Installing a version

1. Resolve the spec against the mirror's `index.json`, which is cached and revalidated with an `ETag`, so a repeat costs one small request.
2. Take nvm's per-version install lock (`.cache/locks/vX.Y.Z`), so a concurrent `rnm install` or `nvm install` of the same version waits instead of clashing.
3. Download `SHASUMS256.txt` and pick the best archive it lists: `.tar.xz` (about half the size of `.tar.gz`), else `.tar.gz`; `.zip` on Windows. An archive without a checksum is never installed.
4. Stream the archive into nvm's cache (`.cache/bin/<name>/`) while hashing it. A cached archive with the right hash is reused.
5. Unpack into a hidden folder next to the final one, check that it contains a `node` binary, and rename it into place. Readers never see a half-installed version.

On Apple Silicon, versions before 16 (which have no arm64 build) install the x64 build for Rosetta 2, as nvm does. An x64 rnm running under Rosetta 2 or Windows on Arm emulation still installs native arm64 builds.

## Credentials

rnm follows HTTP redirects itself instead of letting the HTTP library do it. On every hop it picks the credential whose scope (origin and path prefix) covers the new URL, so a token for `https://mirror.corp/nodejs/` is never sent to the CDN the mirror redirects to, to another port, or over plain http. Credential values are marked sensitive and never formatted into output or errors. See [Configuration](configuration.md#credentials).

## Size

rnm is about 2.7 MB with no runtime dependencies. It uses `ureq` with rustls (no async runtime), pure-Rust xz and gzip decoders, and the zip reader only on Windows. Release builds use fat LTO and abort on panic.
