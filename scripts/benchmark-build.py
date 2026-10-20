#!/usr/bin/env python3
"""Measure a clean dev build and a rebuild after touching the startup module."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import time


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output", type=Path, required=True,
        help="new directory for result.json, timings and compiler output",
    )
    parser.add_argument(
        "--keep-target", action="store_true",
        help="keep the generated target directory after the run",
    )
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    # Recorded results touch this module. Keep it so new runs stay comparable.
    touch_source = root / "src/startup.rs"
    if not touch_source.is_file():
        parser.error(f"missing source file: {touch_source}")
    output = args.output.resolve()
    if output.exists():
        parser.error(f"output already exists: {output}")
    output.mkdir(parents=True)
    builds = root / "target" / "hillclimb"
    builds.mkdir(parents=True, exist_ok=True)
    target = Path(tempfile.mkdtemp(prefix="build-", dir=builds))
    cargo_config = root / ".cargo/config.toml"
    env = os.environ.copy()
    # Cache hits in a compiler wrapper must not masquerade as faster compilation.
    for key in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
        env.pop(key, None)
    env["CARGO_TARGET_DIR"] = str(target)
    env["CMAKE_POLICY_VERSION_MINIMUM"] = "3.5"
    command = [
        "cargo", "build", "--locked", "--offline", "--bin", "bespoke",
        "--timings", "--message-format=json-render-diagnostics",
    ]
    result = {
        "commit": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=root, text=True
        ).strip(),
        "worktree_status": subprocess.check_output(
            ["git", "status", "--porcelain", "--untracked-files=no"], cwd=root, text=True
        ),
        "worktree_diff": subprocess.check_output(
            ["git", "diff", "HEAD", "--", "Cargo.toml", "Cargo.lock", ".cargo", "build.rs", "src"],
            cwd=root, text=True,
        ),
        "manifest_sha256": sha256(root / "Cargo.toml"),
        "manifest": (root / "Cargo.toml").read_text(),
        "source_touch_file": str(touch_source.relative_to(root)),
        "lockfile_sha256": sha256(root / "Cargo.lock"),
        "cargo_config": cargo_config.read_text() if cargo_config.exists() else None,
        "host": platform.platform(),
        "logical_cpus": os.cpu_count(),
        "rustc": subprocess.check_output(["rustc", "-Vv"], text=True),
        "command": command,
        "target": str(target),
        "environment": {
            key: value for key, value in env.items()
            if key.startswith((
                "CARGO_", "RUST", "CMAKE_", "CC", "CXX", "SDKROOT", "SWIFTC",
                "MACOSX_", "TRANSCRIBE_", "DYLD_",
            ))
        },
        "samples": {},
    }

    def write_result():
        (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")

    try:
        for mode in ("clean", "source-touch"):
            if mode == "source-touch":
                os.utime(touch_source, None)
            start = time.monotonic()
            with (output / f"{mode}.jsonl").open("w") as stdout, (
                output / f"{mode}.stderr.txt"
            ).open("w") as stderr:
                completed = subprocess.run(command, cwd=root, env=env, stdout=stdout, stderr=stderr)
            elapsed = time.monotonic() - start
            messages = [
                json.loads(line) for line in (output / f"{mode}.jsonl").read_text().splitlines()
                if line.startswith("{")
            ]
            artifacts = [m for m in messages if m.get("reason") == "compiler-artifact"]
            compiled = [m for m in artifacts if not m["fresh"]]
            binaries = [
                m for m in compiled
                if m["target"]["name"] == "bespoke" and m.get("executable")
            ]
            valid = (
                completed.returncode == 0
                and any(m.get("reason") == "build-finished" and m["success"] for m in messages)
                and len(binaries) == 1
                and (mode != "clean" or len(compiled) >= 100)
            )
            # valid stays false until the binary also passes the CLI smoke check.
            sample = {
                "seconds": elapsed,
                "exit_code": completed.returncode,
                "compiled_artifacts": len(compiled),
                "valid": False,
                "compilation_valid": valid,
            }
            timing_path = target / "cargo-timings/cargo-timing.html"
            if timing_path.exists():
                timing = timing_path.read_text()
                jobs = re.search(r"\bjobs=(\d+)\b", timing)
                sample["effective_jobs"] = int(jobs.group(1)) if jobs else None
                shutil.copy2(timing_path, output / f"{mode}.timing.html")
            result["samples"][mode] = sample
            write_result()
            if not valid:
                sys.exit(f"{mode} build did not compile the bespoke binary. See {output}")
            binary = Path(binaries[0]["executable"])
            smoke = subprocess.run(
                [str(binary), "--help"], cwd=root, env=env, capture_output=True,
                text=True, timeout=30,
            )
            (output / f"{mode}.help.txt").write_text(smoke.stdout + smoke.stderr)
            if smoke.returncode or "--transcribe-file" not in smoke.stdout:
                sys.exit(f"{mode} binary failed the --help smoke check. See {output}")
            sample["binary_bytes"] = binary.stat().st_size
            sample["cli_smoke_passed"] = True
            sample["valid"] = True
            write_result()
            print(json.dumps({mode: sample}), flush=True)
    finally:
        if not args.keep_target:
            shutil.rmtree(target)


if __name__ == "__main__":
    main()
