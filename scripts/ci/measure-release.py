#!/usr/bin/env python3
"""Report release job wall time, including setup and artifact overhead, from gh."""

import argparse
import datetime
import json
import subprocess


def seconds(start, end):
    return int(
        (datetime.datetime.fromisoformat(end.replace("Z", "+00:00"))
         - datetime.datetime.fromisoformat(start.replace("Z", "+00:00"))).total_seconds()
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_id")
    parser.add_argument("--attempt", type=int, help="Inspect an earlier run attempt")
    parser.add_argument("--max-seconds", type=int, help="Optional strict upper limit for each release job")
    args = parser.parse_args()
    if args.max_seconds is not None and args.max_seconds <= 0:
        parser.error('--max-seconds must be positive')
    command = ["gh", "run", "view", args.run_id, "--json", "jobs,url"]
    if args.attempt is not None:
        command.extend(["--attempt", str(args.attempt)])
    run = json.loads(subprocess.check_output(command, text=True))
    print(run["url"])
    passed = True
    jobs = [job for job in run["jobs"] if job["name"].endswith("release")]
    if not jobs:
        raise SystemExit("No release jobs found")
    for job in jobs:
        print(f'\n### {job["name"]}\n')
        if job["status"] != "completed":
            print("Still running")
            passed = False
            continue
        if not job["steps"]:
            print(f'{job["conclusion"]}, no runner steps executed, FAIL')
            passed = False
            continue
        duration = seconds(job["startedAt"], job["completedAt"])
        succeeded = job["conclusion"] == "success"
        passed &= succeeded
        print(f'{duration // 60}m{duration % 60:02}s, {job["conclusion"]}')
        if args.max_seconds is not None:
            within_budget = succeeded and duration < args.max_seconds
            passed &= within_budget
            print(f'{"PASS" if within_budget else "FAIL"} <{args.max_seconds}s')
        print("\n| Step | Seconds |\n| --- | ---: |")
        for step in job["steps"]:
            if step["conclusion"] != "skipped":
                print(f'| {step["name"]} | {seconds(step["startedAt"], step["completedAt"])} |')
    raise SystemExit(0 if passed else 1)


if __name__ == "__main__":
    main()
