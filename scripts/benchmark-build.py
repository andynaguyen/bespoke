#!/usr/bin/env python3
"""Measure a clean dev build and a source-touch rebuild without shared artifacts."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--keep-target", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    builds = root / "target" / "hillclimb"
    builds.mkdir(parents=True, exist_ok=True)
    target = Path(tempfile.mkdtemp(prefix="build-", dir=builds))
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
        "manifest_sha256": hashlib.sha256((root / "Cargo.toml").read_bytes()).hexdigest(),
        "rustc": subprocess.check_output(["rustc", "-Vv"], text=True),
        "command": command,
        "target": str(target),
        "environment": {
            key: value for key, value in env.items()
            if key.startswith(("CARGO_", "RUST", "CMAKE_", "CC", "CXX", "SDKROOT", "SWIFTC", "MACOSX_"))
        },
        "samples": {},
    }
    try:
        for mode in ("clean", "source-touch"):
            if mode == "source-touch":
                os.utime(root / "src/main.rs", None)
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
            sample = {
                "seconds": elapsed, "exit_code": completed.returncode,
                "compiled_artifacts": len(compiled), "valid": valid,
            }
            result["samples"][mode] = sample
            (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps({mode: sample}), flush=True)
            if not valid:
                raise RuntimeError(f"{mode} did not compile the expected binary. See {output}")
            binary = Path(binaries[0]["executable"])
            smoke = subprocess.run(
                [str(binary), "--help"], cwd=root, env=env, capture_output=True,
                text=True, timeout=30,
            )
            (output / f"{mode}.help.txt").write_text(smoke.stdout + smoke.stderr)
            if smoke.returncode or "--transcribe-file" not in smoke.stdout:
                raise RuntimeError("The compiled binary failed the CLI smoke check")
            sample["binary_bytes"] = binary.stat().st_size
            sample["cli_smoke_passed"] = True
            shutil.copy2(target / "cargo-timings/cargo-timing.html", output / f"{mode}.timing.html")
            (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    finally:
        if not args.keep_target:
            shutil.rmtree(target)


if __name__ == "__main__":
    main()
