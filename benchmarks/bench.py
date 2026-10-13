#!/usr/bin/env python3
"""Helpers for benchmarks/run.sh: a local Node.js mirror, checksum checks,
and the Markdown report.

    bench.py serve DIR PORT_FILE
    bench.py verify MIRROR_DIR VERSION...
    bench.py report OUT_DIR META_JSON

Only the standard library is used, so it runs on any CI runner or Mac.
"""

import functools
import hashlib
import http.server
import json
import math
import os
import sys


def serve(directory, port_file):
    """Serves DIRECTORY on 127.0.0.1 on a free port and writes the port to
    PORT_FILE once the socket is listening."""

    class Handler(http.server.SimpleHTTPRequestHandler):
        # Keep-alive, as a real CDN allows.
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

    handler = functools.partial(Handler, directory=directory)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    tmp = port_file + ".tmp"
    with open(tmp, "w") as f:
        f.write(str(server.server_address[1]))
    os.replace(tmp, port_file)
    server.serve_forever()


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify(mirror_dir, versions):
    """Checks every archive in each version directory against that
    version's SHASUMS256.txt. A bad file is deleted so the next run fetches
    it again."""
    ok = True
    for version in versions:
        version_dir = os.path.join(mirror_dir, version)
        expected = {}
        with open(os.path.join(version_dir, "SHASUMS256.txt")) as f:
            for line in f:
                parts = line.split()
                if len(parts) == 2:
                    expected[parts[1].lstrip("*")] = parts[0]
        for name in sorted(os.listdir(version_dir)):
            if name == "SHASUMS256.txt":
                continue
            path = os.path.join(version_dir, name)
            if expected.get(name) != sha256_of(path):
                print(f"bench.py: checksum mismatch, deleted {path}", file=sys.stderr)
                os.remove(path)
                ok = False
    return 0 if ok else 1


# ---------------------------------------------------------------------------
# Report

TOOLS = ("rnm", "fnm", "nvm")
MS = 1000.0
# Derived per-call costs below this (in seconds) are reported as zero.
NOISE_FLOOR = 0.05 / MS


class Stat:
    """A mean and standard deviation in seconds."""

    def __init__(self, mean, stddev):
        self.mean = mean
        self.stddev = stddev or 0.0

    def minus(self, other, divisor=1):
        """(self - other) / divisor, with the deviations of two independent
        measurements combined in quadrature."""
        return Stat(
            (self.mean - other.mean) / divisor,
            math.hypot(self.stddev, other.stddev) / divisor,
        )


def load(out_dir, name):
    path = os.path.join(out_dir, name + ".json")
    if not os.path.exists(path):
        return None
    with open(path) as f:
        results = json.load(f)["results"]
    return dict(wall_clock(r) for r in results)


def wall_clock(result):
    """(command name, wall-clock Stat) from one hyperfine result. hyperfine
    1.x puts the name in `command` and the times at the top level; 2.x has
    `name` and a `summary` per metric."""
    if "summary" in result:
        time = result["summary"]["time_wall_clock"]
        return result["name"], Stat(time["mean"], time.get("stddev"))
    return result["command"], Stat(result["mean"], result.get("stddev"))


def fmt_ms(stat):
    precision = 2 if abs(stat.mean) * MS < 100 else 1
    return f"{stat.mean * MS:.{precision}f} ± {stat.stddev * MS:.{precision}f}"


def is_noise(stat):
    """True when a derived per-call cost cannot be told apart from zero."""
    return stat.mean < max(2 * stat.stddev, NOISE_FLOOR)


def table(rows, extra_header=None):
    """rows: (label, Stat or None, extra or None). Relative speed is against
    the fastest row. A row whose cost is within noise of zero is the
    fastest, and the other rows get no ratio."""
    stats = [s for _, s, _ in rows if s is not None]
    any_noise = any(is_noise(s) for s in stats)
    fastest = None if any_noise or not stats else min(s.mean for s in stats)
    header = "| Tool | Mean ± σ (ms) |"
    rule = "|---|--:|"
    if extra_header:
        header += f" {extra_header} |"
        rule += "--:|"
    header += " Relative |"
    rule += "--:|"
    lines = [header, rule]
    for label, stat, extra in rows:
        if stat is None:
            cells = ["n/a"] + (["n/a"] if extra_header else []) + ["n/a"]
        else:
            if is_noise(stat):
                value, relative = f"≈ 0 ({fmt_ms(stat)})", "**fastest**"
            elif fastest:
                ratio = stat.mean / fastest
                value = fmt_ms(stat)
                relative = "**1.00**" if ratio < 1.005 else f"{ratio:.2f}×"
            else:
                value, relative = fmt_ms(stat), "—"
            cells = [value] + ([extra or ""] if extra_header else []) + [relative]
        lines.append(f"| {label} | " + " | ".join(cells) + " |")
    return "\n".join(lines)


def per_call(results, tool, base_key, loop_key, calls):
    if results is None:
        return None
    base = results.get(f"{tool} {base_key}")
    loop = results.get(f"{tool} {loop_key}")
    if base is None or loop is None:
        return None
    return loop.minus(base, calls)


def report(out_dir, meta_path):
    with open(meta_path) as f:
        meta = json.load(f)
    calls = int(meta["calls"])
    out = []
    w = out.append

    w("# rnm, fnm and nvm benchmarks\n")
    w(f"- Date: {meta['date']}")
    w(f"- OS: {meta['os']}")
    w(f"- CPU: {meta['cpu']}")
    w(f"- Shell: {meta['bash']}")
    w(f"- hyperfine: {meta['hyperfine']}")
    w(f"- Tools: rnm {meta['versions']['rnm']}, fnm {meta['versions']['fnm']}, "
      f"nvm {meta['versions']['nvm']}")
    w(f"- Node.js: {', '.join(meta['node_versions'])} (`default` = {meta['node_versions'][-1]}), "
      "served from a local mirror on 127.0.0.1")
    w(f"- Runs: {meta['runs']} (+{meta['warmup']} warm-up) per command; "
      f"install: {meta['install_runs']} (+1 warm-up); {calls} calls per loop")
    w("")
    w("Mean ± standard deviation of wall-clock time. *Relative* is against the "
      "fastest tool in the table. Raw hyperfine JSON is next to this file.")
    w("")

    startup = load(out_dir, "startup")
    if startup is not None:
        w("## Shell startup\n")
        w("A non-interactive `bash` that runs what `~/.bashrc` would: "
          "`eval \"$(rnm env --shell bash)\"`, `eval \"$(fnm env --shell bash)\"`, "
          "or `. \"$NVM_DIR/nvm.sh\"` (which activates `default`). "
          "*Added* is the time over the bare-`bash` baseline.\n")
        baseline = startup.get("baseline")
        rows = []
        for tool in TOOLS:
            stat = startup.get(f"{tool} init")
            added = None
            if stat is not None and baseline is not None:
                added = f"{(stat.mean - baseline.mean) * MS:.2f}"
            rows.append((tool, stat, added))
        w(table(rows, "Added (ms)"))
        if baseline is not None:
            w(f"\nBaseline, `bash` running an empty script: {fmt_ms(baseline)} ms.")
        w("")

    calls_results = {tool: load(out_dir, f"calls-{tool}") for tool in TOOLS}
    if any(r is not None for r in calls_results.values()):
        w("## Commands in an initialised shell (per call)\n")
        w(f"Each run starts `bash`, initialises the tool as above, then makes {calls} "
          f"calls. The time of the initialisation alone (measured in the same "
          f"hyperfine invocation) is subtracted and the rest divided by {calls}. "
          "σ combines both measurements' deviations. Output goes to `/dev/null`. "
          "For nvm the commands are shell functions in the same process; "
          "for rnm and fnm each call starts a process.\n")
        sections = (
            ("use", f"`use`: alternating `use 20` and `use 22` (every call switches)"),
            ("current", "`current`"),
            ("ls", "`ls` (`nvm ls` also lists aliases)"),
        )
        for key, title in sections:
            w(f"### {title}\n")
            rows = [(tool, per_call(calls_results[tool], tool, "init", key, calls), None)
                    for tool in TOOLS]
            w(table(rows))
            w("")

    cd_results = {tool: load(out_dir, f"cd-{tool}") for tool in TOOLS}
    if any(r is not None for r in cd_results.values()):
        w("## cd hook (per `cd`)\n")
        w("rnm and fnm: `env --use-on-cd`. nvm has no built-in hook, so it uses the "
          "`cdnvm` recipe from nvm's README. Each hook replaces `cd` with an alias, "
          "which non-interactive bash expands only after `shopt -s expand_aliases`; "
          "every script sets it. After each `cd` the script runs "
          "`eval \"$PROMPT_COMMAND\"`, as an interactive prompt would (only rnm "
          f"sets it). Each run makes {calls} `cd` calls; the hook setup alone is "
          f"subtracted and the rest divided by {calls}.\n")
        sections = (
            ("cd-switch", "Between two projects whose `.nvmrc` files name different versions"),
            ("cd-plain", "Between two directories without a version file"),
        )
        for key, title in sections:
            w(f"### {title}\n")
            rows = [(tool if tool != "nvm" else "nvm (cdnvm)",
                     per_call(cd_results[tool], tool, "hook-init", key, calls), None)
                    for tool in TOOLS]
            w(table(rows))
            w("")
        w("Without a version file, rnm and `cdnvm` switch back to `default`, so they "
          "check on every `cd`; fnm's hook only acts when the new directory itself "
          "has a version file.\n")

    install = load(out_dir, "install")
    if install is not None:
        w("## Install\n")
        w(f"`install {meta['node_versions'][-1]}` into a tree without that version and with "
          "an empty download cache (both removed by hyperfine `--prepare` before every "
          "run): fetch, verify (rnm, nvm) and unpack the `.tar.xz` from the local mirror. "
          "nvm's time includes sourcing `nvm.sh --no-use`.\n")
        rows = [(tool, install.get(f"{tool} install"), None) for tool in TOOLS]
        w(table(rows))
        w("")

    text = "\n".join(out).rstrip() + "\n"
    with open(os.path.join(out_dir, "results.md"), "w") as f:
        f.write(text)
    sys.stdout.write(text)
    return 0


def main(argv):
    if len(argv) >= 3 and argv[0] == "serve":
        return serve(argv[1], argv[2])
    if len(argv) >= 3 and argv[0] == "verify":
        return verify(argv[1], argv[2:])
    if len(argv) == 3 and argv[0] == "report":
        return report(argv[1], argv[2])
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
