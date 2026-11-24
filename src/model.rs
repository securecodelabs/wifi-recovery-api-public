use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CONTRACT: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Search {
    pub pattern: String,
    #[serde(default)]
    pub runtime_seconds: u64,
    #[serde(default)]
    pub candidate_limit: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    Running,
    Stopping,
    Stopped,
    Interrupted,
    LimitReached,
    Recovered,
    Exhausted,
    Failed,
}

impl State {
    pub fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Stopping)
    }

    pub fn resumable(self) -> bool {
        matches!(self, Self::Stopped | Self::Interrupted | Self::LimitReached)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Progress {
    pub stage: String,
    pub sample_at_ms: Option<u64>,
    #[serde(default)]
    pub rate_sample_at_ms: Option<u64>,
    #[serde(default)]
    pub estimate_sample_at_ms: Option<u64>,
    pub fresh: bool,
    pub tested_candidates: Option<u64>,
    pub total_candidates: Option<u64>,
    pub fraction: Option<f64>,
    pub recovered_records: Option<u64>,
    pub total_records: Option<u64>,
    pub candidate_hex: Option<String>,
    pub candidate_text: Option<String>,
    pub candidates_per_second: Option<f64>,
    pub kh_per_second: Option<f64>,
    pub speed_basis: String,
    pub search_elapsed_seconds: Option<f64>,
    pub estimated_full_seconds: Option<f64>,
    pub search_eta_seconds: Option<f64>,
    pub estimated_finish_at_ms: Option<u64>,
    pub runtime_remaining_seconds: Option<f64>,
    pub candidate_limit_remaining: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Attempt {
    pub number: u32,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub search_started_at_ms: Option<u64>,
    pub elapsed_seconds: f64,
    pub timing_complete: bool,
    pub outcome: State,
    pub engine_run_at_ms: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Job {
    pub id: Uuid,
    pub state: State,
    pub search: Search,
    #[serde(default = "default_gpu_batch")]
    pub gpu_batch: u32,
    pub filename: String,
    pub input_sha256: String,
    pub input_bytes: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub first_started_at_ms: Option<u64>,
    pub total_active_seconds: f64,
    pub attempts: Vec<Attempt>,
    pub progress: Progress,
    pub error: Option<String>,
    pub resume_available: bool,
    pub resume_reason: Option<String>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn default_gpu_batch() -> u32 {
    4096
}

pub fn display_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| {
            std::ascii::escape_default(*byte)
                .map(char::from)
                .collect::<String>()
        })
        .collect()
}
