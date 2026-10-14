#!/usr/bin/env python3
"""Run the CONTRIBUTING.md gate sequence and leave a receipt (issue #80).

Usage (from anywhere in the repository):
  scripts/proof.py                    run every gate in order, stop at the first failure
  scripts/proof.py --chromium PATH    run the browser suite against a system Chromium
  scripts/proof.py --list             print the parsed sequence and exit
  scripts/proof.py --self-test        prove fail-fast, receipt retention, and parsing
Exit: 0 every gate passed, 1 a gate failed or the run was interrupted,
      2 usage or setup error (nothing was run).

Single source: the sequence is not written here. It is read from the first
```sh block under "## Build and verification" in CONTRIBUTING.md; each
non-blank, non-comment line is one step, run with `sh -c` from the repository
root. With --chromium (or CONSOLEBOOK_E2E_CHROMIUM in the environment), the
one step running `npm run e2e` is replaced by the system-browser line from the
block in that section that sets CONSOLEBOOK_E2E_CHROMIUM=/path/to/chromium.
Anything else about those blocks changing makes this script fail closed.

Receipts: each run writes a new directory under .proof/ (ignored, never
deleted or overwritten here) holding receipt.json, receipt.md, one log per
step, and the web asset manifest. Failed and interrupted runs keep theirs.
The receipt is local evidence only; hosted pr-gate results are linked, not
copied. Depends only on git and the Python 3 standard library.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import platform
import shlex
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ME = "scripts/proof.py"
SCHEMA = "consolebook-proof-receipt/1"
SECTION = "## Build and verification"
E2E_MARKER = "npm run e2e"
BROWSER_VAR = "CONSOLEBOOK_E2E_CHROMIUM"
BROWSER_PLACEHOLDER = "/path/to/chromium"
RECEIPT_ROOT = ".proof"
BINARY = "target/debug/consolebook-server"
WEB_BUILD = "web/build"
# An artifact is hashed only when a step containing its producer passed in
# this run; otherwise a leftover from an earlier build would pass as evidence.
BINARY_PRODUCER = "cargo build -p consolebook-server"
WEB_BUILD_PRODUCER = "npm run build"


class SetupError(Exception):
    """A precondition failed before any step ran (exit 2)."""


# --- Sequence source ---------------------------------------------------------


def parse_sequence(text: str) -> tuple[list[str], str | None]:
    """Return (gate steps, system-browser line or None) from CONTRIBUTING.md text."""
    lines = text.splitlines()
    try:
        start = lines.index(SECTION)
    except ValueError:
        raise SetupError(f"CONTRIBUTING.md has no '{SECTION}' heading") from None
    end = next(
        (i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")),
        len(lines),
    )

    blocks: list[list[str]] = []
    current: list[str] | None = None
    for line in lines[start + 1 : end]:
        stripped = line.strip()
        if current is None:
            if stripped == "```sh":
                current = []
        elif stripped == "```":
            blocks.append(current)
            current = None
        else:
            current.append(stripped)
    if current is not None:
        raise SetupError(f"CONTRIBUTING.md: unterminated ```sh block under '{SECTION}'")
    if not blocks:
        raise SetupError(f"CONTRIBUTING.md: no ```sh block under '{SECTION}'")

    steps = [s for s in blocks[0] if s and not s.startswith("#")]
    if not steps:
        raise SetupError("CONTRIBUTING.md: the gate block has no commands")
    # A receipt that says "passed" must include the browser suite exactly once.
    e2e = sum(E2E_MARKER in s for s in steps)
    if e2e != 1:
        raise SetupError(
            f"CONTRIBUTING.md: expected one gate step running '{E2E_MARKER}', found {e2e}"
        )

    browser_lines = [
        s for block in blocks[1:] for s in block if f"{BROWSER_VAR}=" in s
    ]
    if len(browser_lines) > 1:
        raise SetupError(
            f"CONTRIBUTING.md: expected one {BROWSER_VAR}= line, found {len(browser_lines)}"
        )
    if browser_lines and E2E_MARKER not in browser_lines[0]:
        raise SetupError(
            f"CONTRIBUTING.md: the {BROWSER_VAR}= line must run '{E2E_MARKER}'"
        )
    return steps, (browser_lines[0] if browser_lines else None)


def with_system_browser(steps: list[str], browser_line: str | None, chromium: str) -> list[str]:
    """Swap the browser step for CONTRIBUTING.md's system-browser line."""
    if browser_line is None:
        raise SetupError(f"CONTRIBUTING.md documents no {BROWSER_VAR}= line to use")
    if browser_line.count(BROWSER_PLACEHOLDER) != 1:
        raise SetupError(
            f"CONTRIBUTING.md: the {BROWSER_VAR}= line must contain '{BROWSER_PLACEHOLDER}' once"
        )
    hits = [i for i, s in enumerate(steps) if E2E_MARKER in s]
    if len(hits) != 1:
        raise SetupError(
            f"CONTRIBUTING.md: expected one gate step running '{E2E_MARKER}', found {len(hits)}"
        )
    out = list(steps)
    out[hits[0]] = browser_line.replace(BROWSER_PLACEHOLDER, shlex.quote(chromium))
    return out


# --- Evidence helpers --------------------------------------------------------


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def git(root: Path, *args: str) -> bytes:
    return subprocess.run(
        ["git", *args], cwd=root, check=True, capture_output=True
    ).stdout


def candidate_state(root: Path) -> dict:
    """Revision, tree, and dirty state: what the receipt's results are about."""
    status = git(root, "status", "--porcelain=v1", "--untracked-files=all").decode()
    untracked = {}
    for line in status.splitlines():
        if line.startswith("?? "):
            rel = line[3:]
            p = root / rel
            untracked[rel] = sha256_file(p) if p.is_file() else None
    return {
        "head": git(root, "rev-parse", "HEAD").decode().strip(),
        "tree": git(root, "rev-parse", "HEAD^{tree}").decode().strip(),
        "dirty": bool(status.strip()),
        "status": status.splitlines(),
        "tracked_diff_sha256": hashlib.sha256(git(root, "diff", "HEAD", "--binary")).hexdigest(),
        "untracked_sha256": untracked,
    }


def tool_version(argv: list[str], cwd: Path) -> str:
    try:
        r = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.TimeoutExpired) as e:
        return f"unavailable ({e.__class__.__name__})"
    out = (r.stdout or r.stderr).strip().splitlines()
    return out[0] if r.returncode == 0 and out else f"unavailable (exit {r.returncode})"


def browser_identity(root: Path, chromium: str | None) -> dict:
    if chromium:
        return {
            "kind": "system",
            "path": chromium,
            "version": tool_version([chromium, "--version"], root),
        }
    resolved = tool_version(
        ["node", "-e", "process.stdout.write(require('@playwright/test').chromium.executablePath())"],
        root / "web",
    )
    return {
        "kind": "playwright-managed",
        "path": resolved,
        "version": tool_version([resolved, "--version"], root)
        if resolved.startswith("/")
        else "unresolved",
        "note": "path as resolved by Playwright; headless runs may use its headless shell",
    }


def privilege() -> dict:
    """Whether this process can bypass file permissions, as the permission tests care.

    On Linux that is CAP_DAC_OVERRIDE or CAP_DAC_READ_SEARCH in the effective
    set, so a root process with those dropped counts as unprivileged; elsewhere
    it falls back to an effective uid of 0.
    """
    euid = os.geteuid() if hasattr(os, "geteuid") else None
    try:
        for line in Path("/proc/self/status").read_text().splitlines():
            if line.startswith("CapEff:"):
                bypass = bool(int(line.split()[1], 16) & 0b110)
                return {"privileged": bypass, "euid": euid, "privilege_source": "CapEff"}
    except (OSError, ValueError):
        pass
    return {"privileged": euid == 0, "euid": euid, "privilege_source": "euid"}


def web_manifest(root: Path, out_dir: Path) -> dict | None:
    build = root / WEB_BUILD
    if not build.is_dir():
        return None
    rows = sorted(
        f"{sha256_file(p)}  {p.relative_to(root).as_posix()}"
        for p in build.rglob("*")
        if p.is_file()
    )
    manifest = out_dir / "web-build.sha256"
    manifest.write_text("\n".join(rows) + "\n")
    return {"path": manifest.name, "files": len(rows), "sha256": sha256_file(manifest)}


# --- Runner ------------------------------------------------------------------


def new_receipt_dir(base: Path, head: str, dirty: bool) -> Path:
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    name = f"{stamp}-{head[:12]}{'-dirty' if dirty else ''}"
    base.mkdir(parents=True, exist_ok=True)
    for n in range(100):
        d = base / (name if n == 0 else f"{name}-{n}")
        try:
            d.mkdir()
            return d
        except FileExistsError:
            continue
    raise SetupError(f"could not create a fresh receipt directory under {base}")


def run_step(cmd: str, root: Path, env: dict, log_path: Path) -> int:
    with log_path.open("wb") as log:
        proc = subprocess.Popen(
            ["sh", "-c", cmd], cwd=root, env=env,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        try:
            assert proc.stdout is not None
            for chunk in iter(lambda: proc.stdout.read1(1 << 16), b""):
                sys.stdout.buffer.write(chunk)
                sys.stdout.buffer.flush()
                log.write(chunk)
            return proc.wait()
        except KeyboardInterrupt:
            proc.terminate()
            proc.wait()
            raise


def run_sequence(
    steps: list[str],
    root: Path,
    receipts: Path,
    *,
    chromium: str | None,
    invocation: dict,
    artifacts: bool = True,
) -> tuple[dict, Path]:
    env = dict(os.environ)
    if chromium:
        env[BROWSER_VAR] = chromium
    before = candidate_state(root)
    out_dir = new_receipt_dir(receipts, before["head"], before["dirty"])
    contributing = root / "CONTRIBUTING.md"

    receipt: dict = {
        "schema": SCHEMA,
        "scope": "local",
        "scope_note": "local run; hosted pr-gate results are separate evidence, linked not copied",
        "invocation": invocation,
        "started_at": now(),
        "candidate": before,
        "sequence_source": {
            "path": "CONTRIBUTING.md",
            "sha256": sha256_file(contributing) if contributing.is_file() else None,
        },
        "host": {"platform": platform.platform(), **privilege()},
        "toolchain": {
            "rustc": tool_version(["rustc", "--version"], root),
            "node": tool_version(["node", "--version"], root),
            "npm": tool_version(["npm", "--version"], root),
        },
        "browser_requested": "system" if chromium else "playwright-managed",
        "steps": [{"index": i + 1, "command": s, "cwd": ".", "status": "not-run"} for i, s in enumerate(steps)],
    }
    if receipt["host"]["privileged"]:
        print(f"{ME}: warning: this process can bypass file permissions; CONTRIBUTING.md "
              "asks for an unprivileged user so permission tests are meaningful", file=sys.stderr)
    print(f"{ME}: browser for '{E2E_MARKER}': "
          f"{'system Chromium at ' + chromium if chromium else 'Playwright-managed Chromium'}")
    print(f"{ME}: receipt directory: {out_dir}")

    t0 = time.monotonic()
    # "passed" is set only when every step ran and exited 0; any other end,
    # including an unexpected exception, leaves a non-passing receipt.
    result, failed = "error", None
    try:
        for step in receipt["steps"]:
            i, n = step["index"], len(steps)
            print(f"\n==> [{i}/{n}] {step['command']}", flush=True)
            log = out_dir / f"{i:02d}.log"
            step["started_at"] = now()
            s0 = time.monotonic()
            try:
                code = run_step(step["command"], root, env, log)
            except BaseException as e:
                interrupted = isinstance(e, KeyboardInterrupt)
                step.update(status="interrupted" if interrupted else "error", exit_status=None)
                failed = i
                raise
            finally:
                step["duration_s"] = round(time.monotonic() - s0, 3)
                if log.is_file():
                    step["log"] = log.name
                    step["log_sha256"] = sha256_file(log)
            step["exit_status"] = code
            step["status"] = "passed" if code == 0 else "failed"
            print(f"<== [{i}/{n}] exit {code} in {step['duration_s']:.1f}s", flush=True)
            if code != 0:
                result, failed = "failed", i
                break
        else:
            result = "passed"
    except KeyboardInterrupt:
        result = "interrupted"
        print(f"\n{ME}: interrupted; writing the receipt", file=sys.stderr)
    finally:
        receipt["finished_at"] = now()
        receipt["duration_s"] = round(time.monotonic() - t0, 3)
        receipt["result"] = result
        receipt["failed_step"] = failed
        after = candidate_state(root)
        receipt["candidate_unchanged_during_run"] = after == before
        if after != before:
            receipt["candidate_after"] = after
        receipt["browser"] = browser_identity(root, chromium) if artifacts else None
        receipt["artifacts"] = collect_artifacts(root, out_dir, receipt["steps"]) if artifacts else {}
        write_receipt(receipt, out_dir)
    return receipt, out_dir


def produced(steps: list[dict], producer: str) -> bool:
    return any(producer in s["command"] and s["status"] == "passed" for s in steps)


def collect_artifacts(root: Path, out_dir: Path, steps: list[dict]) -> dict:
    binary = root / BINARY
    return {
        "binary": {"path": BINARY, "sha256": sha256_file(binary)}
        if produced(steps, BINARY_PRODUCER) and binary.is_file()
        else None,
        "web_build_manifest": web_manifest(root, out_dir)
        if produced(steps, WEB_BUILD_PRODUCER)
        else None,
    }


def write_receipt(receipt: dict, out_dir: Path) -> None:
    (out_dir / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    (out_dir / "receipt.md").write_text(render_markdown(receipt))


def render_markdown(r: dict) -> str:
    c = r["candidate"]
    b = r.get("browser") or {}
    rows = "\n".join(
        f"| {s['index']} | `{s['command']}` | {s['status']} | "
        f"{s.get('exit_status', '—')} | {s.get('duration_s', '—')} | "
        f"{s['log']} `{s['log_sha256'][:16]}` |" if "log" in s else
        f"| {s['index']} | `{s['command']}` | {s['status']} | "
        f"{s.get('exit_status', '—')} | {s.get('duration_s', '—')} | no log |"
        for s in r["steps"]
    )
    arts = r.get("artifacts") or {}
    binary = arts.get("binary")
    manifest = arts.get("web_build_manifest")
    return f"""### Local proof receipt — {r['result']}

- Candidate: `{c['head']}` tree `{c['tree']}`{' (dirty)' if c['dirty'] else ''}; unchanged during run: {r['candidate_unchanged_during_run']}
- Scope: {r['scope']} ({r['scope_note']})
- Command: `{r['invocation']['command']}` in `{r['invocation']['cwd']}`
- Started {r['started_at']}, finished {r['finished_at']}, {r['duration_s']:.1f}s
- Sequence source: CONTRIBUTING.md sha256 `{r['sequence_source']['sha256']}`
- Browser: {b.get('kind', 'not recorded')} `{b.get('path', '')}` ({b.get('version', '')})
- Host: {r['host']['platform']}; euid {r['host']['euid']}; can bypass file permissions: {r['host']['privileged']} (from {r['host']['privilege_source']})
- Toolchain: {r['toolchain']['rustc']}; node {r['toolchain']['node']}; npm {r['toolchain']['npm']}
- Binary: {f"`{binary['path']}` sha256 `{binary['sha256']}`" if binary else 'not built by this run'}
- Web build manifest: {f"{manifest['files']} files, sha256 `{manifest['sha256']}`" if manifest else 'not built by this run'}

| # | Command | Status | Exit | Seconds | Log sha256 (prefix) |
| --- | --- | --- | --- | --- | --- |
{rows}
"""


# --- Self-test ---------------------------------------------------------------


def self_test(root: Path) -> int:
    """Positive and negative controls for the runner and the parser."""
    failures: list[str] = []

    def check(cond: bool, what: str) -> None:
        print(f"  {'ok  ' if cond else 'FAIL'} {what}")
        if not cond:
            failures.append(what)

    inv = {"command": f"{ME} --self-test", "cwd": str(root)}
    with tempfile.TemporaryDirectory() as tmp:
        base = Path(tmp)
        print("negative control: a failing step stops the chain and keeps its receipt")
        r, d = run_sequence(["echo one", "echo two >&2; exit 3", "echo three"], root, base,
                            chromium=None, invocation=inv, artifacts=False)
        saved = json.loads((d / "receipt.json").read_text())
        check(r["result"] == "failed" and r["failed_step"] == 2, "result failed at step 2")
        check([s["status"] for s in saved["steps"]] == ["passed", "failed", "not-run"],
              "statuses passed, failed, not-run")
        check(saved["steps"][1]["exit_status"] == 3, "exit status 3 recorded")
        check(not (d / "03.log").exists(), "step 3 never started")
        check((d / "02.log").read_bytes() == b"two\n", "stderr captured in the step log")
        check(saved["steps"][1]["log_sha256"] == sha256_file(d / "02.log"), "log hash matches")

        print("positive control: a passing chain records passed")
        r, d2 = run_sequence(["true"], root, base, chromium=None, invocation=inv, artifacts=False)
        check(r["result"] == "passed" and r["failed_step"] is None, "result passed")
        check(d2 != d and (d / "receipt.json").is_file(), "earlier failed receipt kept")

    print("error control: an unexpected exception records an error, never a pass")
    global run_step
    real_step = run_step
    with tempfile.TemporaryDirectory() as tmp:
        base = Path(tmp)

        def broken(*_a, **_k):
            raise OSError("injected")

        run_step = broken
        try:
            run_sequence(["true", "true"], root, base, chromium=None, invocation=inv, artifacts=False)
            check(False, "the exception propagates")
        except OSError:
            check(True, "the exception propagates")
        finally:
            run_step = real_step
        (d,) = list(base.iterdir())
        saved = json.loads((d / "receipt.json").read_text())
        check(saved["result"] == "error" and saved["failed_step"] == 1, "result error at step 1")
        check([s["status"] for s in saved["steps"]] == ["error", "not-run"], "statuses error, not-run")

    print("artifacts: hashed only when their producing step passed in this run")
    step = {"command": f"x && {BINARY_PRODUCER}", "status": "not-run"}
    check(not produced([step], BINARY_PRODUCER), "a not-run build yields no binary hash")
    check(produced([dict(step, status="passed")], BINARY_PRODUCER), "a passed build does")

    print("parser: reads CONTRIBUTING.md and fails closed on drift")
    steps, alt = parse_sequence((root / "CONTRIBUTING.md").read_text())
    check(sum(E2E_MARKER in s for s in steps) == 1, "one browser step in the gate block")
    swapped = with_system_browser(steps, alt, "/opt/x y/chromium")
    check(any(f"{BROWSER_VAR}='/opt/x y/chromium'" in s for s in swapped)
          and not any(BROWSER_PLACEHOLDER in s for s in swapped),
          "system browser path substituted and quoted")
    for label, text in [
        ("missing section", "# Contributing\n"),
        ("missing block", f"{SECTION}\n\nno commands\n"),
        ("unterminated block", f"{SECTION}\n```sh\ncargo test\n"),
        ("empty block", f"{SECTION}\n```sh\n\n```\n"),
        ("a gate block without the browser suite", f"{SECTION}\n```sh\ncargo test\n```\n"),
        ("a duplicated browser suite",
         f"{SECTION}\n```sh\n(cd web && {E2E_MARKER})\n(cd web && {E2E_MARKER})\n```\n"),
        ("a system-browser line that skips the suite",
         f"{SECTION}\n```sh\n(cd web && {E2E_MARKER})\n```\n\n```sh\n"
         f"echo {BROWSER_VAR}={BROWSER_PLACEHOLDER}\n```\n"),
    ]:
        try:
            parse_sequence(text)
            check(False, f"rejects {label}")
        except SetupError:
            check(True, f"rejects {label}")
    try:
        with_system_browser(["cargo test"], f"(cd web && {BROWSER_VAR}={BROWSER_PLACEHOLDER} {E2E_MARKER})", "/c")
        check(False, "rejects a sequence with no browser step")
    except SetupError:
        check(True, "rejects a sequence with no browser step")

    print(f"{ME}: self-test {'passed' if not failures else f'FAILED ({len(failures)})'}")
    return 0 if not failures else 1


# --- Entry -------------------------------------------------------------------


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(prog=ME, description=__doc__.split("\n\n")[0])
    ap.add_argument("--chromium", metavar="PATH",
                    help=f"system Chromium for the browser suite (default: ${BROWSER_VAR} if set)")
    mode = ap.add_mutually_exclusive_group()
    mode.add_argument("--list", action="store_true", help="print the parsed sequence and exit")
    mode.add_argument("--self-test", action="store_true", help="run the runner's own controls")
    args = ap.parse_args(argv)

    try:
        root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel").decode().strip())
    except (OSError, subprocess.CalledProcessError):
        print(f"{ME}: not inside a git repository", file=sys.stderr)
        return 2

    try:
        if args.self_test:
            return self_test(root)
        steps, alt = parse_sequence((root / "CONTRIBUTING.md").read_text())
        chromium = args.chromium or os.environ.get(BROWSER_VAR) or None
        if chromium:
            if not (os.path.isfile(chromium) and os.access(chromium, os.X_OK)):
                raise SetupError(f"--chromium is not an executable file: {chromium}")
            chromium = os.path.abspath(chromium)
            steps = with_system_browser(steps, alt, chromium)
        if args.list:
            for i, s in enumerate(steps, 1):
                print(f"{i}. {s}")
            return 0
        invocation = {"command": shlex.join([ME, *argv]), "cwd": os.getcwd()}
        receipt, out_dir = run_sequence(steps, root, root / RECEIPT_ROOT,
                                        chromium=chromium, invocation=invocation)
    except SetupError as e:
        print(f"{ME}: {e}", file=sys.stderr)
        return 2

    print(f"\n{render_markdown(receipt)}")
    print(f"{ME}: {receipt['result']}; receipt {out_dir / 'receipt.json'} "
          f"sha256 {sha256_file(out_dir / 'receipt.json')}")
    if not receipt["candidate_unchanged_during_run"]:
        print(f"{ME}: warning: the candidate changed during the run; this receipt is stale",
              file=sys.stderr)
    return 0 if receipt["result"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
