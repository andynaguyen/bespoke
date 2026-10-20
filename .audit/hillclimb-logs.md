# Build time hillclimb

Metric: wall seconds for a clean default development build of the complete Bespoke binary. Target: at least 50% less time. Release build performance is outside this metric.

PROFILE: main. Worker: codex/gpt-6-luna, reasoningEffort=xhigh, serviceTier=priority.

Template: `/Users/karat-01/.codex/skills/hillclimb/references/log-template.md`.

Original baseline: 97709e3ff075c86dce171d2bda7d8f78d2e19356. Harness commit: c6b831d35531370b68d6c895050649f4abd5acaf.

Protocol: `python3 scripts/benchmark-build.py --output .audit/build-times/NAME`. Every sample gets an empty target directory. Registry and git downloads are already cached. Cargo runs locked and offline, without compiler wrappers. All compilation runs serially. Each run must compile the binary and at least 100 artifacts, then pass `--help`. A subsequent source-touch rebuild measures warm compiler reuse, not the cost of every possible source edit. The script records Cargo timings and artifact events, then removes only its generated target directory unless `--keep-target` is set. No feature, dependency source, release profile, or application behavior may be removed to satisfy the target.

Baseline runs: 59.536s and 56.316s. Median 57.926s. Range 5.56% of median. Initial acceptance threshold: more than 6% improvement, repeated if near the noise boundary. The final baseline and final candidate will be rerun under the same conditions before reporting.

Current best: E1 dependency debuginfo override. Final confirmation: baseline median 57.792s, final median 52.171s, 9.73% less time. The 50% target remains unmet. Stop: diminishing returns after four rejected build experiments and one screened hypothesis. No runtime features, dependency sources or release settings changed.


| Timestamp | Experiment | Hypothesis | Change | Before | After | Delta | Evidence | Decision and reason | Commit SHA |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 2026-10-20 | baseline | Establish clean build noise | None | N/A | 59.536s, 56.316s. Median 57.926s | N/A | `build-times/baseline-1/`, `build-times/baseline-2/` | Baseline recorded. 838 compiled artifacts each. CLI smoke checks pass. Source-touch 3.312s and 3.326s | c6b831d |
| 2026-10-20 | E1 | Dependency symbols cost compile and link time | `profile.dev.package."*".debug = 0` | 57.926s median | 53.678s, 52.040s. Median 52.859s | -5.067s, -8.75% | `build-times/e1-dependency-debug-1/`, `build-times/e1-dependency-debug-2/` | Keep. Repeated gain exceeds baseline noise. App debug symbols retained. Dependency source debugging loses detail. 838 artifacts and CLI smoke pass. Source-touch median 3.163s | fcaf213 |
| 2026-10-20 | E2, screened | Unused macOS bindings cost type checking and codegen | Proposed explicit objc2 feature lists, not applied | 52.859s | Not measured | N/A | `vendor/handy-keys/Cargo.toml` also enables AppKit and Foundation defaults | Reject before build. Cargo unification keeps broad defaults. Fixing the transitive declaration would violate no dependency patches | N/A |
| 2026-10-20 | E3 | CMake unity reduces repeated C++ header parsing | `TRANSCRIBE_CMAKE_ARGS=-DCMAKE_UNITY_BUILD=ON` | 52.859s | Build failed after 39.271s | Invalid, no gain | `build-times/e3-unity-1/` | Revert. Metal includes Objective-C Foundation headers through a C unity unit, causing compile errors. No dependency patch attempted | N/A |
| 2026-10-20 | E4 | More Cargo jobs start the native critical path earlier | Temporary `build.jobs = 24` | 52.859s | 59.366s | +6.507s, +12.31% | `build-times/e4-jobs24-1/` | Revert. Clean build regresses. Source-touch 2.934s does not justify primary regression or machine-specific setting | N/A |
| 2026-10-20 | E5 | Fewer dependency LLVM units reduce compiler and linker overhead | Dependency `codegen-units = 16` | 52.859s | 54.074s | +1.215s, +2.30% | `build-times/e5-codegen16-1/` | Revert. No gain past noise. Source-touch 3.172s, critical path unchanged | N/A |
| 2026-10-20 | E6 | Fewer jobs reduce native and Rust compiler contention | Temporary `build.jobs = 8` | 52.859s | 57.584s | +4.725s, +8.94% | `build-times/e6-jobs8-1/` | Revert. Clean build regresses. Source-touch 3.313s | N/A |
| 2026-10-20T09:08:28+00:00 | Final confirmation | Verify original baseline versus retained E1 under the same conditions | Alternate original manifest and E1, B1/F1/B2/F2 | 58.210s, 57.373s. Median 57.792s | 52.126s, 52.215s. Median 52.171s | -5.621s, -9.73% | `build-times/confirm-*/`, `build-times/summary.json` | Keep E1. Both paired gains exceed the original 6% noise threshold. 50% target not met | fcaf213 |

E1 verification: `cargo fmt --check` and `cargo clippy --locked --offline` pass. Clippy reports 12 existing code warnings. Logs are under `build-times/e1-*.txt`. Raw Cargo artifact JSON and timing HTML stay in the local audit directory but are ignored by Git. Result JSON, CLI output, compiler output, and this decision log are committed.

E1 tests: `cargo test --locked --offline` passes all 344 tests, 0 ignored. Evidence: `build-times/e1-tests.txt`.


Final confirmation uses the original Cargo.toml bytes for baseline runs and E1 bytes for final runs, alternating B1, F1, B2, F2 in the same checkout. Result manifest hashes distinguish them even though HEAD stays on the accepted commit. The harness now also records the lockfile hash, repository Cargo config, operating system and CPU count. Its timed command and validation are unchanged.

The baseline and accepted candidate have identical package targets and enabled feature sets across all 838 artifacts. Evidence: `build-times/feature-comparison.json`.

Final source-touch medians: baseline 3.183s, final 3.187s. The 0.004s difference is noise. All confirmation builds compiled 838 artifacts and passed the CLI smoke check. There were five measured candidates: one kept, four reverted, plus one screened hypothesis that was never applied.

Remaining hypotheses: an upstream handy-keys release that narrows the objc2 feature declarations could reduce compilation of unused macOS bindings without a local dependency patch. Upstream Objective-C support in CMake unity builds could make the rejected native-build experiment viable. A separate incremental-build study could examine moving stable backend modules into a library crate, but that does not promise a faster clean build. Native artifact caching helps repeated builds but does not meet this clean-compilation metric. None of these unmeasured ideas count toward the gain.

The retained change omits debug information only from development dependencies. Bespoke keeps full debug information. Developers inspecting dependency local variables or stepping through dependency sources can remove the package debug override, as documented in README.md. The release profile and Cargo.lock are unchanged.

Final verification: `cargo fmt --check`, `cargo clippy --locked --offline`, Python byte compilation and `git diff --check` pass. Clippy retains the existing 12 source warnings. The 344-test run validates the retained application configuration. Redundant experiment build directories were removed; `confirm-final-2` retains its binary and artifacts.

## Resumed run

The user requested exhaustion of remaining cheap hypotheses after the Opus 5.5 review. The original 50% reduction predicate and no dependency patches, no used feature removal, no new dubious tooling constraints remain. Earlier stop is superseded. PROFILE remains main, Luna xhigh with priority service.

The harness now records tracked dirty-tree status, source/config diffs, full Cargo.toml and the effective job count from Cargo timings. CLI smoke success is required before marking a sample valid. Timed commands are unchanged. New raw text outputs remain local and ignored; result JSON and this log preserve conclusions. Existing historical evidence remains available.

First refreshed current-best run: 56.222s clean and 3.383s source-touch, 838 compiled artifacts. XProtect was consuming CPU during the run, so a second baseline is required and the earlier 52.171s median is not used alone to accept candidates. Evidence: `build-times/resume-best-1/`.

Queue: app line-table debug info, app no debug info, dependency line-table info, root incremental/codegen settings, supported linking settings and grounded first-party changes. Each candidate is measured alone against the retained configuration. An independent read-only worker is checking which choices are distinct from Cargo defaults and identifying other low-cost options.

Resumed baseline repeat: 51.425s clean, 3.143s source-touch. Background-load drift is too large to use the first refreshed value alone. Candidate acceptance uses adjacent runs and repeats for apparent gains. Previous 6% screening threshold remains, and smaller apparent gains require repeated paired evidence rather than accepting a single noisy sample.

| Timestamp | Experiment | Hypothesis | Change | Before | After | Delta | Evidence | Decision and reason | Commit SHA |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 2026-10-20 | Resumed baseline | Reestablish current-best timing | E1 unchanged | Earlier 52.171s | 56.222s, 51.425s | Background drift | `build-times/resume-best-1/`, `build-times/resume-best-2/` | Original baseline not redefined. These are current-best E1 control runs | d5f3157 |
| 2026-10-20 | E7 | Root full debug metadata costs time at the final Rust compile step | Root dev debug=line-tables-only | Controls 51.425s and 52.218s, median 51.821s | 48.955s and 49.764s, median 49.359s | -2.462s, -4.75% | `build-times/e7-*/`, `build-times/resume-best-2/` | Keep after checks. Both paired gains are about 4.7%, controls and candidates each vary by only about 1.6%. Root compile tail drops from 10.05s to 7.43s. App backtrace lines retained, local-variable debugging requires documented opt-in. fmt, clippy and all 344 tests pass | This commit, E7 |
