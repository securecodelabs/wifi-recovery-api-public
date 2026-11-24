use crate::model::{Progress, Search, display_bytes, now_ms};
use serde_json::Value;

#[derive(Default)]
pub struct Observation {
    pub progress: Progress,
    pub engine_run: Option<u64>,
    pub search_started: Option<u64>,
    pub previous_run: Option<u64>,
    pub start_offset: u64,
}

impl Observation {
    pub fn activity(&mut self, line: &str) {
        let Some(json) = line.strip_prefix("WIFI_ACTIVITY ") else {
            return;
        };
        let Ok(value) = serde_json::from_str::<Value>(json) else {
            return;
        };
        let stage = value["stage"].as_str().unwrap_or("");
        if !matches!(
            stage,
            "loading_sources" | "restoring_position" | "initializing_device" | "recovering"
        ) {
            return;
        }
        self.progress.stage = stage.into();
        if stage == "recovering" {
            self.search_started.get_or_insert(now_ms());
        }
        candidate(&mut self.progress, value["latest_candidate_hex"].as_str());
        self.progress.sample_at_ms = Some(now_ms());
    }

    pub fn throughput(&mut self, line: &str) {
        if !line.starts_with("status=") {
            return;
        }
        let value = line
            .split_whitespace()
            .find_map(|part| part.strip_prefix("derivations_per_second="));
        self.progress.kh_per_second = value
            .and_then(|text| text.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| value / 1000.0);
        self.progress.speed_basis = "engine_run_average_pbkdf2_derivations".into();
        self.progress.rate_sample_at_ms = Some(now_ms());
    }

    pub fn snapshot(&mut self, value: &Value, search: &Search) {
        let telemetry = &value["telemetry"];
        let marker = telemetry["run_started_at_ms"].as_u64();
        if marker.is_none() || marker == self.previous_run {
            return;
        }
        self.engine_run = marker;
        let p = &mut self.progress;
        p.sample_at_ms = Some(now_ms());
        p.estimate_sample_at_ms = p.sample_at_ms;
        p.tested_candidates = value["tested_candidates"].as_u64();
        p.total_candidates = value["total_candidates"].as_u64();
        p.recovered_records = value["recovered"].as_u64();
        p.total_records = value["total_records"].as_u64();
        p.candidates_per_second = number(&telemetry["candidates_per_second"]);
        p.search_elapsed_seconds = number(&telemetry["elapsed_seconds"]);
        p.search_eta_seconds = number(&telemetry["search_eta_seconds"]);
        p.runtime_remaining_seconds = number(&telemetry["runtime_remaining_seconds"]);
        p.fraction = p
            .tested_candidates
            .zip(p.total_candidates)
            .filter(|(_, total)| *total > 0)
            .map(|(tested, total)| (tested as f64 / total as f64).min(1.0));
        p.estimated_full_seconds = p
            .total_candidates
            .zip(p.candidates_per_second)
            .filter(|(_, rate)| *rate > 0.0)
            .map(|(total, rate)| total as f64 / rate);
        p.estimated_finish_at_ms = p
            .search_eta_seconds
            .map(|eta| now_ms().saturating_add((eta * 1000.0) as u64));
        p.candidate_limit_remaining = (search.candidate_limit > 0).then(|| {
            search.candidate_limit.saturating_sub(
                value["next_candidate"]
                    .as_u64()
                    .unwrap_or(self.start_offset)
                    .saturating_sub(self.start_offset),
            )
        });
        candidate(p, value["latest_candidate_hex"].as_str());
    }
}

pub fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|number| number.is_finite() && *number >= 0.0)
}

fn candidate(progress: &mut Progress, value: Option<&str>) {
    let Some(value) = value.filter(|text| (16..=126).contains(&text.len())) else {
        return;
    };
    let Ok(bytes) = hex::decode(value) else {
        return;
    };
    progress.candidate_text = Some(display_bytes(&bytes));
    progress.candidate_hex = Some(value.to_ascii_lowercase());
}

pub fn stop_live(progress: &mut Progress) {
    progress.fresh = false;
    progress.candidates_per_second = None;
    progress.kh_per_second = None;
    progress.search_eta_seconds = None;
    progress.estimated_finish_at_ms = None;
    progress.runtime_remaining_seconds = None;
}

pub fn age_estimates(progress: &mut Progress, now: u64) {
    if progress
        .rate_sample_at_ms
        .is_none_or(|at| now.saturating_sub(at) > 5000)
    {
        progress.kh_per_second = None;
    }
    let age = progress
        .estimate_sample_at_ms
        .map(|at| now.saturating_sub(at));
    if age.is_none_or(|age| age > 5000) {
        progress.candidates_per_second = None;
        progress.search_eta_seconds = None;
        progress.estimated_finish_at_ms = None;
        progress.runtime_remaining_seconds = None;
        return;
    }
    let elapsed = age.unwrap_or(0) as f64 / 1000.0;
    progress.search_eta_seconds = progress
        .search_eta_seconds
        .map(|seconds| (seconds - elapsed).max(0.0));
    progress.runtime_remaining_seconds = progress
        .runtime_remaining_seconds
        .map(|seconds| (seconds - elapsed).max(0.0));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_old_attempts_and_invalid_rates() {
        let mut sample = Observation {
            previous_run: Some(12),
            ..Observation::default()
        };
        sample.snapshot(
            &json!({"telemetry":{"run_started_at_ms":12},"tested_candidates":99}),
            &Search {
                pattern: "password".into(),
                runtime_seconds: 0,
                candidate_limit: 0,
            },
        );
        assert_eq!(sample.progress.tested_candidates, None);
        sample.throughput("status=running derivations_per_second=12345");
        assert_eq!(sample.progress.kh_per_second, Some(12.345));
        sample.throughput("status=running derivations_per_second=NaN");
        assert_eq!(sample.progress.kh_per_second, None);
        sample.activity("WIFI_ACTIVITY {\"stage\":\"recovering\",\"latest_candidate_hex\":\"70617373776f7264\"}");
        assert_eq!(sample.progress.candidate_text.as_deref(), Some("password"));
    }
}
