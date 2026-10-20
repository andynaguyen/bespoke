# Build time hillclimb

Metric: wall seconds for a clean default development build of the complete Bespoke binary. Target: at least 50% less time. Release build performance is outside this metric.

PROFILE: main. Worker: codex/gpt-6-luna, reasoningEffort=xhigh, serviceTier=priority.

Original baseline: 97709e3ff075c86dce171d2bda7d8f78d2e19356. Harness commit: c6b831d35531370b68d6c895050649f4abd5acaf.

Protocol: `python3 scripts/benchmark-build.py --output .audit/build-times/NAME`. Every sample gets an empty target directory. Registry and git downloads are already cached. Cargo runs locked and offline, without compiler wrappers. All compilation runs serially. Each run must compile the binary and at least 100 artifacts, then pass `--help`. A subsequent source-touch rebuild measures warm compiler reuse, not the cost of every possible source edit. The script records Cargo timings and artifact events, then removes only its generated target directory unless `--keep-target` is set. No feature, dependency source, release profile, or application behavior may be removed to satisfy the target.

Baseline runs: 59.536s and 56.316s. Median 57.926s. Range 5.56% of median. Initial acceptance threshold: more than 6% improvement, repeated if near the noise boundary. The final baseline and final candidate will be rerun under the same conditions before reporting.

Current best: E1 dependency debuginfo override, median 52.859s, 8.75% below original baseline. Target: 28.963s. Next hypothesis: narrow macOS API bindings to APIs actually used.

| Timestamp | Experiment | Hypothesis | Change | Before | After | Delta | Evidence | Decision and reason | Commit SHA |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 2026-10-20 | baseline | Establish clean build noise | None | N/A | 59.536s, 56.316s. Median 57.926s | N/A | `build-times/baseline-1/`, `build-times/baseline-2/` | Baseline recorded. 838 compiled artifacts each. CLI smoke checks pass. Source-touch 3.312s and 3.326s | c6b831d |
| 2026-10-20 | E1 | Dependency symbols cost compile and link time | `profile.dev.package."*".debug = 0` | 57.926s median | 53.678s, 52.040s. Median 52.859s | -5.067s, -8.75% | `build-times/e1-dependency-debug-1/`, `build-times/e1-dependency-debug-2/` | Keep. Repeated gain exceeds baseline noise. App debug symbols retained. Dependency source debugging loses detail. 838 artifacts and CLI smoke pass. Source-touch median 3.163s | This commit, `perf(build): avoid dependency debug symbol generation` |

E1 verification: `cargo fmt --check` and `cargo clippy --locked --offline` pass. Clippy reports 12 existing code warnings. Logs are under `build-times/e1-*.txt`. Raw Cargo artifact JSON and timing HTML stay in the local audit directory but are ignored by Git. Result JSON, CLI output, compiler output, and this decision log are committed.
