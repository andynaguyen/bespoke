# Build time hillclimb

Metric: wall seconds for a clean default development build of the complete Bespoke binary. Target: at least 50% less time. Release build performance is outside this metric.

Original baseline: 97709e3ff075c86dce171d2bda7d8f78d2e19356. Harness commit: c6b831d35531370b68d6c895050649f4abd5acaf. All runs took place on 2026-10-20 on one Apple Silicon Mac with 12 logical CPUs with stable rustc 1.99.0.

Provenance: PROFILE main. Worker codex/gpt-6-luna, reasoningEffort=xhigh, serviceTier=priority. Two Opus 5.5 reviews shaped the second round.

## Outcome

Retained changes, all in `[profile.dev]` of `Cargo.toml`:

| Experiment | Setting | Role | Commit |
| --- | --- | --- | --- |
| E1 | `profile.dev.package."*".debug = 0` | Speed | fcaf213 |
| E7 | `profile.dev.debug = "line-tables-only"` | Speed | 2279f6a |
| E18 | `profile.dev.package.handy-keys.debug = 2` | Debugging safeguard, no claimed gain | f9bf5d2 |

Final confirmation ran three alternating original/retained pairs, each followed by a `src/startup.rs` touch rebuild:

| | Original | Retained | Change |
| --- | --- | --- | --- |
| Clean builds | 58.053s, 62.474s, 61.213s | 49.913s, 51.756s, 49.899s | Paired -14.02%, -17.16%, -18.48% |
| Clean median | 61.213s | 49.913s | -11.299s, -18.46% |
| Startup touch median | 3.384s | 3.110s | -8.09% |

The 50% target is not met. The median ratio is a measured estimate for this machine, not a universal guarantee. We added the third pair because the first two baseline timings differed by more than the 6% noise screen. No result was dropped. Every observation is in `build-times/resumed-summary.json`.

All six confirmation builds compile the same 838 package targets with identical features, optimization and assertion settings (`build-times/resumed-feature-comparison.json`). Application source, `build.rs`, `Cargo.lock`, dependency versions and features, vendor source and the release profile match the original.

Totals: 25 measured configurations. Two speed changes and one debugging safeguard were retained, and 22 configurations were discarded, including the failed linker and unity builds. Two more hypotheses were screened out before any change.

Debugging tradeoff: Bespoke backtraces keep file and line numbers. `CARGO_PROFILE_DEV_DEBUG=2 cargo build` restores Bespoke's local variables in a debugger. Dependency backtraces lose file and line numbers, except in `handy-keys`. Stepping through other dependency source or inspecting its local variables requires removing the `profile.dev.package."*"` section. README.md documents this.

Why the plateau: in retained final run 1, GPUI runs from 27.31s to 36.29s, gpui-component from 34.70s to 42.53s, and Bespoke from 42.53s to 49.87s. Native transcription finishes at 40.68s, before it can gate the app. Profile, debug, macro, native optimization, concurrency, linking and root-library variants all failed to shorten this chain cheaply (`build-times/retained-critical-path.json`). This is experimental evidence, not a proof that 50% is impossible.

Remaining possibilities need a different scope: a compatible upstream dependency release with narrower default macOS bindings, or first-party restructuring larger than the library split tested in E23. Neither is a measured gain. Local dependency patches, removal of used runtime features, added frameworks, and nightly toolchains or unstable flags are excluded. The user explicitly chose stable Rust, so no nightly toolchain was installed and `RUSTC_BOOTSTRAP` was never set.

## Protocol

Command: `python3 scripts/benchmark-build.py --output .audit/build-times/NAME`.

- Every sample builds in a new, empty target directory with `cargo build --locked --offline`. Registry and git downloads are already cached.
- The harness removes `RUSTC_WRAPPER` and `RUSTC_WORKSPACE_WRAPPER` so compiler caches cannot fake a faster build.
- All builds run serially.
- A sample is valid only if Cargo compiles the `bespoke` binary, the clean build compiles at least 100 artifacts, and the binary passes a `--help` smoke check.
- After the clean build, the harness touches one source file and times the rebuild. This measures warm compiler reuse, not the cost of every possible edit.
- Each `result.json` records the commit, tracked dirty state and source diff, the full `Cargo.toml` and its hash, the lockfile hash, any `.cargo/config.toml`, host, CPU count, `rustc -Vv`, relevant environment variables, the effective job count and per-sample timings.

Protocol changes during the run:

1. After round 1, the harness started recording dirty state, source diffs, the full manifest and the effective job count, and began requiring the smoke check before setting `valid`. The timed command did not change.
2. Before E23, harness commit 7418d4b moved the touched file from `src/main.rs` to `src/startup.rs`, so a thin `main.rs` cannot skip the app code on a warm rebuild. Earlier warm numbers stay labeled by protocol and are not used in the final summary. Clean builds did not change.

Acceptance rule: round 1 accepted a candidate that beat the baseline by more than 6%, the observed baseline range, with a repeat near the boundary. Starting with E7, a smaller gain could also be accepted through paired repeats. Both candidate runs had to beat both adjacent controls by more than the within-group variation, with a matching compile-stage mechanism and no failing checks. E7 qualified with a 4.75% reduction against about 1.6% within-group variation. The 50% target never changed.

## Round 1

Baseline: 59.536s and 56.316s, median 57.926s, a range of 5.56% of the median.

| Experiment | Hypothesis | Change | Before | After | Delta | Evidence | Decision and reason | Commit |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| baseline | Establish clean build noise | None | N/A | 59.536s, 56.316s. Median 57.926s | N/A | `build-times/baseline-1/`, `build-times/baseline-2/` | Baseline recorded. 838 compiled artifacts each. CLI smoke checks pass. Source-touch 3.312s and 3.326s | c6b831d |
| E1 | Dependency symbols cost compile and link time | `profile.dev.package."*".debug = 0` | 57.926s median | 53.678s, 52.040s. Median 52.859s | -5.067s, -8.75% | `build-times/e1-dependency-debug-1/`, `build-times/e1-dependency-debug-2/` | Keep. Repeated gain exceeds baseline noise. 838 artifacts and CLI smoke pass. Source-touch median 3.163s | fcaf213 |
| E2, screened | Unused macOS bindings cost type checking and codegen | Proposed explicit objc2 feature lists, not applied | 52.859s | Not measured | N/A | `vendor/handy-keys/Cargo.toml` also enables AppKit and Foundation defaults | Reject before build. Cargo feature unification keeps broad defaults. Fixing the transitive declaration would need a dependency patch | N/A |
| E3 | CMake unity builds reduce repeated C++ header parsing | `TRANSCRIBE_CMAKE_ARGS=-DCMAKE_UNITY_BUILD=ON` | 52.859s | Build failed after 39.271s | Invalid | `build-times/e3-unity-1/` | Revert. Metal includes Objective-C Foundation headers through a C unity unit, causing compile errors. No dependency patch attempted | N/A |
| E4 | More Cargo jobs start the native critical path earlier | Temporary `build.jobs = 24` | 52.859s | 59.366s | +6.507s, +12.31% | `build-times/e4-jobs24-1/` | Revert. Clean build regresses. Source-touch 2.934s does not justify the regression or a machine-specific setting | N/A |
| E5 | Fewer dependency LLVM units reduce compiler and linker overhead | Dependency `codegen-units = 16` | 52.859s | 54.074s | +1.215s, +2.30% | `build-times/e5-codegen16-1/` | Revert. No gain past noise. Source-touch 3.172s, critical path unchanged | N/A |
| E6 | Fewer jobs reduce native and Rust compiler contention | Temporary `build.jobs = 8` | 52.859s | 57.584s | +4.725s, +8.94% | `build-times/e6-jobs8-1/` | Revert. Clean build regresses. Source-touch 3.313s | N/A |
| Confirmation | Compare original and E1 under the same conditions | Alternate original and E1 manifests, B1/F1/B2/F2 | 58.210s, 57.373s. Median 57.792s | 52.126s, 52.215s. Median 52.171s | -5.621s, -9.73% | `build-times/confirm-*/`, `build-times/summary.json` | Keep E1. Both paired gains exceed the 6% threshold. Source-touch medians 3.183s and 3.187s, a noise-level difference | fcaf213 |

The confirmation builds alternate the original `Cargo.toml` bytes with the E1 bytes in one checkout. Manifest hashes in each result distinguish them even though HEAD stays on the accepted commit. All four builds compile the same 838 package targets and features (`build-times/feature-comparison.json`).

E1 checks: `cargo fmt --check`, `cargo clippy --locked --offline` and `cargo test --locked --offline` pass. Tests: 344 passed, 0 ignored. Clippy reports the 12 existing warnings (`build-times/checks-summary.json`).

## Round 2

The user asked to try the remaining cheap hypotheses after the first Opus review. The 50% target and all constraints stayed the same.

Current-best controls drifted. The first refreshed E1 run took 56.222s while XProtect was using CPU, and the repeat took 51.425s. Neither run redefines the original baseline. From here on, candidates are compared with adjacent control runs.

| Experiment | Hypothesis | Change | Before | After | Delta | Evidence | Decision and reason | Commit |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| E1 controls | Reestablish current-best timing | E1 unchanged | Earlier 52.171s | 56.222s, 51.425s | Background drift | `build-times/resume-best-1/`, `build-times/resume-best-2/` | Original baseline not redefined. Source-touch 3.383s and 3.143s | d5f3157 |
| E7 | Full root debug metadata costs time in the final Rust compile | Root `debug = "line-tables-only"` | Controls 51.425s and 52.218s, median 51.821s | 48.955s and 49.764s, median 49.359s | -2.462s, -4.75% | `build-times/e7-*/`, `build-times/resume-best-2/` | Keep. Both paired gains are about 4.7%, and controls and candidates each vary by about 1.6%. The root compile tail drops from 10.05s to 7.43s. App backtrace lines kept, local-variable debugging needs the documented opt-in. fmt, clippy and all 344 tests pass (`build-times/checks-summary.json`) | 2279f6a |
| E8 | Removing app line tables may cut more codegen | Root `debug = 0` | E7 median 49.359s | 51.121s | +1.762s, +3.57% | `build-times/e8-app-no-debug-1/` | Revert. No clean gain, and source-touch regresses to 3.566s. Not worth losing app backtrace lines | N/A |
| E9 | Dependency line tables may restore backtraces cheaply | Dependency `debug = "line-tables-only"`, root as E7 | 49.359s median | 53.349s | +3.990s, +8.08% | `build-times/e9-dependency-line-tables-1/` | Revert. Dependency line tables cost measurable clean build time. Source-touch 3.125s | N/A |
| E13 | Lower native optimization may reduce C++ contention | Temporary `TRANSCRIBE_CMAKE_ARGS` sets C and C++ Release flags to `-O1 -DNDEBUG` | 49.359s median | 48.609s | -0.750s, -1.52% | `build-times/e13-native-o1-1/`, `build-times/e13-native-o1-env.json` | Revert. Gain is within noise and does not justify a possible transcription slowdown | N/A |
| E10 | Root incremental bookkeeping slows clean builds | Root `incremental = false`, which also makes the default codegen units 16 | 49.359s clean, about 3.11s warm | 48.125s clean, 7.123s warm | Clean -2.50%, warm over twice as slow | `build-times/e10-no-incremental-1/` | Revert. Clean difference is within noise and warm rebuilds regress badly | N/A |
| E11 | Fewer codegen units reduce partition overhead with incremental kept | `profile.dev.codegen-units = 16` | 49.359s median | 53.691s | +4.331s, +8.77% | `build-times/e11-app-codegen16-1/` | Revert. The global setting produced 920 artifacts instead of 838, so it also changed host and target profile reuse. Warm 3.056s. E11b isolates the root crate | N/A |
| E12 | More dependency codegen units may parallelize heavy crates | Dependency `codegen-units = 256` | 49.359s median | 54.834s | +5.474s, +11.09% | `build-times/e12-dependency-codegen256-1/` | Revert. 838 artifacts, slower clean build, warm 3.281s | N/A |
| E11b | Isolate app codegen partitioning from dependency profiles | `package.bespoke.codegen-units = 16` | 49.359s median | 49.613s | +0.253s, +0.51% | `build-times/e11b-app-only-codegen16-1/` | Revert. 838 artifacts but no clean gain. Warm 2.997s does not justify it | N/A |
| E16 | 64 app units may balance partition overhead and parallelism | `package.bespoke.codegen-units = 64` | 49.359s median | 50.224s | +0.865s, +1.75% | `build-times/e16-app-only-codegen64-1/` | Revert. No clean gain, warm 3.016s. App settings 16, 64 and the default 256 are now covered | N/A |
| E17 | A dependency midpoint between 16 and 256 units may help | Dependency `codegen-units = 64` | 49.359s median | 54.612s | +5.252s, +10.64% | `build-times/e17-dependency-codegen64-1/` | Revert. No useful midpoint. 838 artifacts, warm 3.153s | N/A |
| E15 | Different debug-info placement might remove packaging overhead | `profile.dev.split-debuginfo = "off"` instead of the default `unpacked` | 49.359s median | 51.489s | +2.130s, +4.31% | `build-times/e15-split-debug-off-1/` | Revert. No clean gain. Warm 3.104s | N/A |
| E14a | Bundled LLD may reduce link time | Direct `CARGO_TARGET_*_LINKER=ld64.lld` plus toolchain `DYLD` path | 49.359s median | Build fails at 0.511s | Invalid | `build-times/e14-bundled-lld-1/` | Retry. rustc passed compiler-driver arguments to a bare linker, so this is a configuration failure, not a speed result | N/A |
| E14b | Retry bundled LLD through the compiler driver | Xcode clang with `-fuse-ld=` pointing at Rust's bundled ld64.lld, toolchain `DYLD` path | 49.359s median | 51.511s | +2.152s, +4.36% | `build-times/e14b-bundled-lld-driver-1/` | Revert. Build and smoke check pass, but clean and warm (3.647s) regress | N/A |
| E19, screened | Published upstream updates could narrow the binding graph without patches | Inspect published handy-keys and GPUI releases only | N/A | No compatible candidate | N/A | crates.io handy-keys 0.3.4, `vendor/handy-keys/HANDY_PATCH.md`, upstream source | Reject before any change. Published handy-keys lacks the Mouse6 to Mouse8 mappings Bespoke uses and still enables broad objc2 defaults. gpui is still 0.2.2. Newer gpui-component releases offer no build fix and bring broader API changes | N/A |
| E18, review safeguard | Keep full debugging of the vendored shortcut crate | `package.handy-keys.debug = 2` | Adjacent E7 control 50.271s | 51.928s first run, 50.016s adjacent repeat | Adjacent -0.255s, within noise | `build-times/e18-*/` | Keep as a debugging safeguard, not a speed gain. Warm control 3.200s, repeat 3.135s, 838 artifacts. fmt, clippy and all 344 tests pass (`build-times/e18-checks.json`) | f9bf5d2 |
| E20 | Optimized bindgen may shorten the build script that gates GPUI | `package.bindgen.opt-level = 3` | E18 repeat 50.016s | 53.634s | +3.619s, +7.24% | `build-times/e20-bindgen-opt3-1/` | Revert. bindgen compile grows from 2.38s to 17.62s. The gpui_media script shrinks from 5.32s to 3.73s but starts later. 838 artifacts, warm 3.136s | N/A |
| E21 | Faster proc macros and build scripts may shorten the frontend tail | `profile.dev.build-override.opt-level = 3` | E18 50.016s | 96.205s | +46.189s, +92.35% | `build-times/e21-build-opt3-1/` | Revert. Optimizing every build dependency and duplicating host and target variants costs far more than it saves. 920 artifacts, warm 3.111s. E21 already includes optimized bindgen | N/A |
| E20b | Lower bindgen optimization may balance compile and generation cost | `package.bindgen.opt-level = 1` | Adjacent controls 50.016s and 50.634s | 49.035s and 50.212s | Paired -1.96% and -0.83%, overlapping ranges | `build-times/e20b-*/`, `build-times/e18-handy-debug-2/` | Revert. The gain does not repeat past noise. GPUI still waits for its own build script after gpui_media finishes. Warm about 3.15s | N/A |
| E22 | Optimize only macros on the frontend critical path | GPUI, component, i18n, serde and clap macros at `opt-level = 3` | E18 50.016s | 53.156s | +3.140s, +6.28% | `build-times/e22-selected-macros-opt3-1/` | Revert. Narrowing avoids duplicate variants (838 artifacts), but optimizing these macros still costs more than it saves. Warm 3.083s | N/A |
| E22b | Cheaper macro optimization may keep the frontend savings | The same five macros at `opt-level = 1` | Startup-touch protocol control 49.086s, warm 3.367s | 52.155s clean, 3.115s warm | Clean +3.070s, +6.25% | `build-times/e22b-selected-macros-opt1-1/`, `build-times/e23-startup-touch-control-1/` | Revert. Clean build still regresses. No macro near miss exists to combine with the uncertain bindgen result | N/A |
| E23 | A library crate may let app compilation start on dependency metadata | Move the root into `lib.rs` with `pub fn run`, keep a thin `main.rs` with the Windows subsystem attribute | Controls 49.086s and 49.035s, median 49.060s | 48.086s and 51.063s, median 49.574s | +0.514s, +1.05% median | `build-times/e23-*/`, `build-times/e23-library.patch` | Revert in full. The first gain does not repeat. 839 artifacts (library plus binary). Startup touch recompiles both, about 3.265s against a 3.237s control median. Binary grows from about 173MB to 179MB | N/A |
| Final confirmation | Compare original and retained E1+E7+E18 | Three alternating original and retained pairs, startup touch | 61.213s median | 49.913s median | -11.299s, -18.46% | `build-times/resumed-summary.json`, `build-times/resumed-confirm-*/` | Retain two speed changes and one debugging safeguard. 50% target not met | E1 fcaf213, E7 2279f6a, E18 f9bf5d2 |

Review notes:

- E5 dependency `codegen-units = 16` mostly repeated Cargo's effective default for registry dependencies, so E12 and E17 tested 256 and 64.
- E11 changed the global profile and added artifact variants. E11b and E16 used named `bespoke` overrides and kept 838 artifacts.
- `lto = false` already matches the dev default at `opt-level = 0`, so it is not a distinct candidate.
- The second Opus review proposed optimizing bindgen and proc macros. E20 to E22b measured those ideas instead of assuming they work.
- `build.jobs = 16` was screened out without a run. The 12-core machine saturates early and leaves the serial tail underused, more job slots cannot shorten that tail, and both 8 and 24 regressed. This is a scheduling judgment, not proof of an optimum.
- The first review called the root library split expensive. A closer look found a small move plus a thin entry point, so E23 measured it twice.

Upstream screen references: https://crates.io/crates/handy-keys, https://github.com/handy-computer/handy-keys/blob/main/Cargo.toml, https://crates.io/crates/gpui, https://github.com/longbridge/gpui-kit/releases. The worker compared the published handy-keys 0.3.4 checksum against `vendor/handy-keys/HANDY_PATCH.md` and confirmed the extra button mappings are missing upstream.

## Verification and evidence

Final checks on E1+E7+E18: `cargo fmt --check`, `cargo clippy --locked --offline`, `cargo test --locked --offline`, `python3 -m py_compile scripts/benchmark-build.py` and `git diff --check` pass. Tests: 344 passed, 0 failed, 0 ignored. Clippy reports the 12 existing warnings (`build-times/resumed-final-checks.json`). Rejected experiments left no changes to application source, dependencies, build scripts, linker config or the release profile.

Git tracks each run's `result.json`, the check and summary JSON files, the E23 patch and this log. Raw compiler output, CLI output, clippy and test logs, Cargo artifact streams and timing HTML stay in the local `.audit/build-times/` directories, and `.audit/build-times/.gitignore` keeps them out of Git. The retained binary from `resumed-confirm-final-3` stays at the target path recorded in its `result.json`. Other benchmark target directories were deleted.
