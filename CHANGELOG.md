# Changelog

## 0.1.1 — 2026-11-24

- Reject runtime limits above the engine's supported 86400 seconds before accepting a job; zero remains unlimited.
- Return structured JSON errors for unsupported HTTP methods and malformed UUID/UTF-8 job paths.
- Exercise the exact packaged executable with the real CPU engine before release publication.
- Add admission/error regressions and document three review rounds, CPU/HIP integration and Radeon throughput checks.

The HTTP contract remains v1. Native WiFi GPU 0.17.2 is still required. Releases contain one Linux amd64 executable and its SHA-256 checksum.

## 0.1.0 — 2026-11-24

Initial public release: HC22000 pattern searches, persistent jobs, queue/stop/resume, progress/timing, verified results and Linux/AMD resource monitoring.
