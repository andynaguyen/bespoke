#!/usr/bin/env python3
"""Report release job wall time, including cache and artifact overhead, from gh."""

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
    parser.add_argument("--max-seconds", type=int, default=480)
    args = parser.parse_args()
    command = ["gh", "run", "view", args.run_id, "--json", "jobs,url"]
    if args.attempt is not None:
        command.extend(["--attempt", str(args.attempt)])
    run = json.loads(subprocess.check_output(command, text=True))
    print(run["url"])
    passed = True
    releases = [job for job in run["jobs"] if job["name"].endswith("release")]
    jobs = [job for job in run["jobs"]
            if job["name"].endswith("release") or job["name"].startswith("Dependencies ")]
    if not releases:
        raise SystemExit("No release jobs found")
    starts, ends = [], []
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
        starts.append(job["startedAt"])
        ends.append(job["completedAt"])
        duration = seconds(job["startedAt"], job["completedAt"])
        within_budget = job["conclusion"] == "success" and duration < args.max_seconds
        passed &= within_budget
        print(f'{duration // 60}m{duration % 60:02}s, {job["conclusion"]}, '
              f'{"PASS" if within_budget else "FAIL"} <{args.max_seconds}s')
        print("\n| Step | Seconds |\n| --- | ---: |")
        for step in job["steps"]:
            if step["conclusion"] != "skipped":
                print(f'| {step["name"]} | {seconds(step["startedAt"], step["completedAt"])} |')
    if any(job["name"].startswith("Dependencies ") for job in jobs) and starts:
        duration = seconds(min(starts), max(ends))
        passed &= duration < args.max_seconds
        print(f'\nPipeline elapsed: {duration // 60}m{duration % 60:02}s, '
              f'{"PASS" if passed else "FAIL"} <{args.max_seconds}s')
    raise SystemExit(0 if passed else 1)


if __name__ == "__main__":
    main()
