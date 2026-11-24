# WiFi recovery API

A small Rust/Axum HTTP service for the public WiFi GPU 0.17.2 engine. It accepts HC22000 input and a password pattern, stores durable jobs in SQLite, runs one search at a time, and reports progress and Linux/AMD resources. There is no authentication or authorization.

The product is one executable, `wifi-recovery-api`. The engine and its AMD/HIP runtime are separate prerequisites. No Docker files, containers, installers or service definitions are provided. Download the Linux amd64 executable and checksum from [GitHub Releases](https://github.com/securecodelabs/wifi-recovery-api-public/releases).

## Run

```sh
wifi-recovery-api --listen 127.0.0.1:8080 \
  --data-dir ./data --engine /path/to/wifi-gpu \
  --hip-device 0 --gpu-pci 0000:01:00.0
```

The API targets Linux amd64. `--cpu` selects the engine's CPU backend for integration testing. `--gpu-pci` associates telemetry with a PCI device explicitly; without it, automatic association is reported only when exactly one AMD device is visible. `--help`, `--version` and `--licenses` work without starting the service. All application files are placed in `--data-dir`, which is private to its owning user on Unix.

GPU batching is selected when accepting each job and retained across resumes: 262144 for one exact-nonce WPA2 EAPOL v2 record, which supports the engine's fast GPU verification path; 4096 for other inputs to keep mixed-record searches responsive. `--gpu-batch` overrides this choice (1-262144). Larger batches can improve throughput but delay progress/checkpoints on CPU-heavy verification workloads. CPU testing retains the engine's CPU batch default.

For the authorized remote test environment, binaries, runtime libraries, caches, data and logs belong under the WiFi Suite home directory, `/home/wifi-service/wifi-suite`. Tests launch executables directly; no global installation is required.

## Submit and control jobs

Use the companion `wifi-recovery-client`, or send multipart HTTP requests:

```sh
curl --fail http://127.0.0.1:8080/v1/jobs \
  -H "Idempotency-Key: $(uuidgen)" \
  -F 'metadata={"pattern":"Home?d?d?d?d","runtime_seconds":3600,"candidate_limit":0}' \
  -F 'file=@input.hc22000'
```

The returned UUID identifies the same job across stops, resumes and API restarts. Input bytes and search settings are immutable. A resume creates another attempt and gives that attempt a fresh runtime/candidate budget. Resumed jobs join the end of the queue. Zero limits mean unlimited.

| Endpoint | Method | Meaning |
| --- | --- | --- |
| `/healthz`, `/readyz` | GET | Liveness; version and startup engine readiness |
| `/v1/system` | GET | Cached server CPU/RAM and AMD GPU readings |
| `/v1/patterns/preview` | POST | JSON `{ "pattern": "Home?d?d?d?d" }` validation and examples |
| `/v1/jobs` | POST / GET | Submit; list with `limit=1..100` and `offset` |
| `/v1/jobs/{id}` | GET / DELETE | Status; delete an inactive job |
| `/v1/jobs/{id}/stop` | POST | Stop queued work or checkpoint running work |
| `/v1/jobs/{id}/resume` | POST | Requeue stopped/interrupted/limited work |
| `/v1/jobs/{id}/result` | GET | Verified results for inactive work |

See [the contract](docs/contract.md) for states, timing, limits and errors.

## Build and check

```sh
cargo +1.93.0 fmt --all -- --check
cargo +1.93.0 clippy --locked --all-targets --all-features -- -D warnings
cargo +1.93.0 test --locked --all-targets --all-features
cargo +1.93.0 build --release --locked
WIFI_TEST_ENGINE=/path/to/wifi-gpu WIFI_TEST_API="$PWD/target/release/wifi-recovery-api" \
  cargo +1.93.0 test --locked --test engine_integration -- --ignored --test-threads=1
```

Real-engine tests are explicitly ignored by the ordinary test command and explicitly run in Linux CI using the checksum-pinned public engine. Set `WIFI_TEST_HIP=1` to exercise actual AMD hardware. All fixtures are synthetic. The additional SSID fixture and converter fixture originate from the public WiFi GPU test suite; application code and test harnesses are fresh implementations.

The separate hardware performance gate runs two 67,108,864-candidate searches through the API and requires at least 650 kH/s sustained: `WIFI_TEST_HIP=1 WIFI_TEST_ENGINE=/path/to/wifi-gpu cargo test --release --test performance -- --ignored --nocapture`. It uses one exact-nonce WPA2 record and a ten-byte uppercase hexadecimal pattern. See [measured validation](docs/validation.md); performance depends on the input, runtime and hardware.

[Quality requirements](docs/quality.md) describe local security, license and complexity checks. [Dependency notices](THIRD_PARTY_NOTICES.md) are embedded in the executable. Code is licensed under Apache-2.0.

## Releases

Release publication requires an explicit user request. The manual release workflow requires an existing matching version tag, runs the checks, validates the executable version, and publishes only the executable and its SHA-256 checksum. The first release, v0.1.0, uses the locally built executable covered by the Radeon validation report; subsequent CI builds may have different binary hashes. Release notes identify the source revision and the published asset checksum.
