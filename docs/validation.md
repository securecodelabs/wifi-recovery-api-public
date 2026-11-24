# Local validation — 2026-11-24

Implementation source: `26d612f5cbb12eb68e8bf56996d860c20cad3965`. Subsequent documentation commits do not change the executable. Version: 0.1.0; Rust: 1.93.0; engine: public WiFi GPU 0.17.2. This report records local validation before repository publication. The v0.1.0 release uses the exact Linux executable identified below; release notes identify the final source revision and hosted CI run.

## Exact executable

The Linux amd64 executable was built locally in Ubuntu on WSL with `cargo +1.93.0 build --release --locked`, copied to `dist/wifi-recovery-api`, and tested again after packaging. Its SHA-256 is:

```text
6cb8298b18e3d8bdf6002e05d1899794f2d67709a5ef2b0bcfb9437459f69f8e
```

The identical bytes ran on the remote Radeon server. The four real-engine integration tests passed against this packaged executable on both local CPU (15.74 seconds) and remote HIP (15.34 seconds). Both performance rounds below also used these exact bytes. Version, help, embedded licenses and startup were checked. `dist` contains the executable and checksum and is excluded from Git.

## Checks and functional coverage

Formatting and all-target/all-feature Clippy passed on Windows and Linux, with warnings denied and complexity limits of 25 cognitive complexity, 100 function lines, four nesting levels and seven arguments. No handwritten complexity suppression or increased limit was used.

The ordinary suite passed 13 tests on Windows and 14 on Linux. Four real-engine integration tests and one hardware performance test are intentionally excluded from ordinary tests and were run explicitly. The real-engine cases cover exact input retention, verified recovery, concurrent idempotent submission, restart and deletion tombstones, FIFO queueing, queued/running stops, resume with fresh attempt budgets, limits, exhaustion, corrupt checkpoints, abrupt API termination, prevention of overlapping engines, multiple SSIDs, progress/rate separation and live resource readings. Protocol/storage/resource tests cover result/exit disagreement, timing and stale observations, checkpoint integrity, persistent ownership/history and missing or ambiguous telemetry.

Both applications passed actionlint 1.7.12, zizmor 1.32.0, cargo-audit 0.22.2, cargo-deny 0.22.0 and Gitleaks 8.30.1 locally, plus generated-license notice verification. The API audit covered 228 locked dependencies with no advisories reported on this date. Gitleaks checked both the working tree and local Git history. Hosted CI was pending at the time of this local validation; consult the release notes for its subsequent result.

## Radeon performance acceptance

Test hardware: Radeon RX 9060 XT, gfx1200, 16 GiB VRAM, PCI `0000:01:00.0`; Ubuntu 26.04.1, Linux 7.0. The existing GPU memory/power profile was left unchanged. Native HIP reports runtime 7.2.53211 using the existing 7.2.3 runtime/compiler files copied under the test account's home directory. Remaining HSA dependencies resolve from the existing SDK copy there. No global runtime installation or container was used.

Each round was submitted through the API and searched 67,108,864 candidates from `?H?H?H?H?H?H?H?H?H?H`, using the public synthetic `single-wpa2.hc22000` fixture: one exact-nonce WPA2 EAPOL v2 record and one SSID. The API automatically chose batch 262144; engine defaults retained group 64 and SHA1 chunk 256. With this single SSID, candidate throughput and PBKDF2 derivation throughput have the same count basis.

| Final packaged API | Full-search average kH/s | Sustained engine-clock kH/s | Sustained observer-clock kH/s |
| --- | ---: | ---: | ---: |
| Round 1 | 660.566 | 663.084 | 664.492 |
| Round 2 | 659.369 | 662.094 | 661.157 |

The sustained window spans the central 20–80% of each search. The observer measures elapsed monotonic time between first observations of distinct progress counts; repeated cached counts do not move the sample timestamps. The engine-clock measurement independently uses reported search elapsed time. Both must reach 650 kH/s for the test to pass. Full-search averages include search work but exclude API acceptance, GPU initialization and other preparation. The two-round test passed in 203.86 seconds.

Earlier testing with the available HIP 7.14 runtime averaged about 648.5 kH/s and did not satisfy the acceptance gate. Selecting the existing native 7.2 runtime and using the large batch on the engine's fast verification path achieved the measurements above; no engine source changes or GPU clock/power changes were needed. Separate derivation-only benchmark results are not used as search acceptance evidence.

These measurements do not establish 650 kH/s for mixed record types, multiple SSIDs or CPU-heavy verification. Those inputs use batch 4096 by default for responsive progress and stop/resume. Jobs preserve their accepted batch setting across resumes. The mixed-record integration cases passed separately.

## Reproduce the hardware gate

On the test host, every executable, library, cache, temporary file, job and log is under `/home/wifi-service/wifi-suite`. The following environment describes the tested native runtime; it is not an installer:

```sh
root=/home/wifi-service/wifi-suite
export LD_LIBRARY_PATH="$root/runtime/rocm723/lib:$root/runtime/rocm/lib"
export ROCM_PATH="$root/runtime/rocm723" HIP_PATH="$root/runtime/rocm723"
export XDG_CACHE_HOME="$root/runtime/cache723"
export AMD_COMGR_CACHE_DIR="$root/runtime/cache723/comgr"
export TMPDIR="$root/runtime/tmp"
export WIFI_TEST_ENGINE="$root/bin/wifi-gpu"
export WIFI_TEST_API="$root/bin/wifi-recovery-api" WIFI_TEST_HIP=1
"$root/tests/engine-integration" --ignored --nocapture --test-threads=1
"$root/tests/performance" --ignored --nocapture --test-threads=1
```

The two test executables are compiled Rust integration-test harnesses, not product deliverables. Their sources are `tests/engine_integration.rs` and `tests/performance.rs`. Raw final logs remain in `logs/hip723-integration.log` and `logs/performance-api723.log` under that home directory. Ordinary hosted CI exercises the CPU adapter; it does not claim Radeon performance coverage.
