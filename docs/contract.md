# HTTP contract v1

`/v1` identifies the JSON contract. Readiness, resources and result envelopes also report `contract: 1`. Timestamps are unsigned Unix milliseconds; durations and rates are nonnegative seconds and counts per second. Unavailable measurements are JSON `null`, not zero. Job responses use the same model for list, detail and mutations.

## Input and idempotency

Submission contains exactly one multipart `metadata` JSON field and one `file` field. Metadata is at most 4096 bytes. Uploads take at most 60 seconds; four may be validated concurrently. Files are limited to 8 MiB by default, reducible with `--max-upload-mib` (1-8). Engine inspection has a 15-second deadline and bounded output. Accepted files contain 1-10,000 distinct records. Engine results/checkpoints are bounded to 32 MiB, consistent with these input limits. At most 64 jobs are stored by default (`--max-jobs`, 1-10,000), including completed jobs. Delete inactive jobs to free admission capacity.

Search fields are `pattern`, `runtime_seconds` and `candidate_limit`; omitted limits become zero. Runtime is 0-86400 seconds, matching WiFi GPU 0.17.2; zero is unlimited. Larger values are rejected before a job is accepted. Unknown fields are rejected. Patterns contain at most 256 UTF-8 bytes and produce 8-63 password bytes. Supported tokens: `?d`, `?l`, `?u`, `?h`, `?H`, `?s`, `?a`, `?b`, `??`. Literal control characters, unknown tokens and a candidate space exceeding u64 are rejected. The rightmost position changes fastest. Preview returns an exact candidate count and up to three escaped/hex samples.

The job's `gpu_batch` is frozen on acceptance. Automatic selection uses 262144 for one exact WPA2 EAPOL v2 record and 4096 otherwise. A startup `--gpu-batch` override applies only to new jobs. An idempotent retry returns its original job and effective settings, including after server defaults change.

`Idempotency-Key` is an optional UUID bound to the exact uploaded SHA-256 and normalized settings. An identical retry returns HTTP 200 and the same UUID; a fresh submission returns 201 with `Location`. Different content conflicts (409). Bindings survive restarts and job deletion indefinitely; retrying a deleted binding also returns 409 and never recreates the work. Use a new key for deliberately new work. Filenames are bounded display metadata, never paths.

Input and its containing directory are synchronized before the SQLite transaction is acknowledged. SQLite uses WAL and FULL synchronization. Startup removes abandoned staging files and job directories that have no committed database entry. Deletion retains the idempotency tombstone.

## Incremental patterns

Both multipart search metadata and the JSON preview request accept `increment`
(default `false`), `increment_min` and `increment_max`. Bounds require
`increment: true`, are whole numbers within 8–63, and must be ordered. The
minimum defaults to 8; the maximum defaults to the supplied pattern length and
is capped at that length. A pattern shorter than the minimum is rejected.

For example, `{"pattern":"Home?d?d?d?d?d?d","increment":true,"increment_min":8,"increment_max":10}`
searches `Home` plus four, five, then six digits. Each length uses a prefix of
the supplied pattern's byte positions, including literals; no positions are
appended. UTF-8 literals may be split at a byte boundary. Within each length,
the rightmost position changes fastest. The combined count must fit u64.

Preview includes `min_password_length_bytes`, `max_password_length_bytes` and
`lengths`, an array of `{password_length_bytes, candidate_count}`. Its
`candidate_count` is the combined count, and samples follow the combined search
order. `password_length_bytes` is a number for a single length and `null` for a
range. The example above contains 1,110,000 candidates across three lengths.

An incremental search is one job. Its runtime/candidate budget covers the whole
range per attempt; resuming grants a new attempt budget and continues from the
combined checkpoint. Effective bounds are stored in the immutable search and
normalized for idempotency, so omitted defaults and equivalent explicit bounds
are identical. Fixed-length settings omit the new fields when serialized,
preserving existing saved jobs and idempotency keys.

Incremental submissions require an engine built with `--increment` support.
The API probes this capability and rejects unsupported submissions with 400
before creating a job. Fixed-length jobs continue to work with published WiFi
GPU v0.17.2. Incremental use requires updated API, engine and client builds;
the existing published artifacts do not include this feature.

## States and attempts

States: `queued`, `running`, `stopping`, `stopped`, `interrupted`, `limit_reached`, `recovered`, `exhausted`, `failed`. `running` includes preparation and device initialization. Stage and state are separate fields. Exhaustion and limits do not mean recovery.

Stop is idempotent: queued work becomes stopped; running work becomes stopping; inactive work is returned unchanged. Resume is idempotent while queued. Only stopped, interrupted and limit-reached work is otherwise eligible. Never-executed jobs resume from the beginning; previously attempted jobs require a checkpoint. Its format, checksum and known engine identity are checked before enqueue; the engine independently validates original inputs and recovered matches. An engine rejection becomes a failed attempt, never an automatic restart from zero. Deleting active work conflicts; deleting a missing UUID returns 404.

The current executable also serves as an internal process guardian. It owns a separate engine lock, monitors the API's parent pipe and requests SIGTERM when the API disappears. It reaps the engine, forcing termination after 15 seconds if needed. Startup waits up to 25 seconds for this ownership lock before reconciling interrupted attempts. Ordinary stop waits for an activity event before signalling, preventing a race with installation of the engine's signal handlers; preparation that never reports activity has a bounded fallback. Failed forced stops remain distinguishable from checkpointed stops.

Attempts preserve start, search-start, end, duration, engine-run marker, outcome and error. Total active duration excludes waiting and stopped time. Monotonic durations are saved periodically and at completion. Unexpected API death marks the in-flight attempt interrupted and its timing incomplete; the saved duration is last-known. Startup does not automatically resume interrupted work. A directory lock prevents two API instances from owning one queue.

Results return `{contract, job_id, state, engine_result}`. The UUID and the engine's 64-digit input fingerprint are different identities. Results are checked for schema/version, matching recovered counts, valid password bytes, and verified flags. Partial verified matches remain available after stopped, limited, interrupted or failed work if a valid engine result exists. A resumed attempt updates the cumulative result; attempt records retain their timing/outcomes.

## Monitoring semantics

`progress` contains stage, tested/total candidates, fraction, recovered/total records, latest candidate text/hex, candidates/s, kH/s, search elapsed, full-search estimate, search ETA, estimated finish, runtime remaining and candidate budget remaining. The latest candidate is a completed batch observation, not a claim about every GPU lane.

`kh_per_second` is the engine run-average PBKDF2 derivation rate divided by 1000, parsed from its bounded stderr status record. Multiple SSIDs can require multiple derivations per candidate. `speed_basis` records this definition. Search estimates use candidate throughput independently of execution limits. Rate and estimate timestamps are separate; values older than five seconds become unknown. Estimated remaining durations decrease with sample age. Inactive jobs clear live rates and ETA; `estimated_full_seconds` is retained as the last historical estimate. Resume starts with a fresh progress object and ignores the previous engine-run snapshot.

Resources are sampled once per second, independently of requests. CPU uses deltas of Linux `/proc/stat` server-wide counters; the first sample and counter resets are unknown. RAM comes from `MemTotal` and `MemAvailable`. AMD sysfs supplies PCI identity, busy percentage and VRAM used/total. Multi-device mapping stays unknown unless configured. These measurements cover the whole server/device, including other workloads. Every response reports the resource sample timestamp and age.

Errors have `{ "error": { "code": HTTP_STATUS, "message": "..." } }`. Validation uses 400, missing resources 404, state/idempotency/checkpoint conflicts 409, upload timeout 408, admission/concurrency limits 429 and internal failures 500. Readiness describes startup validation; individual device execution failures are reported on their jobs. Polling is the supported monitoring transport.
