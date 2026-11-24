use serde_json::json;
use wifi_recovery_api::{
    engine,
    model::{Progress, Search, State},
    telemetry::{Observation, age_estimates, stop_live},
};

#[test]
fn shared_job_fixture_round_trips_without_contract_changes() {
    let value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/job-v1.json")).unwrap();
    let job: wifi_recovery_api::model::Job = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(job).unwrap(), value);
}

#[test]
fn engine_exits_must_agree_with_verified_results() {
    let mut result = json!({"schema":1,"version":"0.17.2","status":"exhausted","total_records":1,"recovered":0,"matches":[]});
    assert_eq!(engine::outcome(Some(2), &result).unwrap(), State::Exhausted);
    assert!(engine::outcome(Some(0), &result).is_err());
    result["status"] = "recovered".into();
    assert!(engine::outcome(Some(0), &result).is_err());
    result["recovered"] = 1.into();
    result["matches"] = json!([{"verified":true,"password_hex":hex::encode("password")}]);
    assert_eq!(engine::outcome(Some(0), &result).unwrap(), State::Recovered);
    result["matches"][0]["verified"] = false.into();
    assert!(engine::outcome(Some(0), &result).is_err());
}

#[test]
fn attempt_budget_and_rates_have_independent_freshness() {
    let search = Search {
        pattern: "password?d".into(),
        candidate_limit: 10,
        runtime_seconds: 30,
    };
    let mut sample = Observation {
        start_offset: 50,
        ..Observation::default()
    };
    sample.throughput("status=running derivations_per_second=4000");
    sample.snapshot(
        &json!({"next_candidate":54,"tested_candidates":54,"total_candidates":100,
        "telemetry":{"run_started_at_ms":12,"elapsed_seconds":2,"candidates_per_second":2000,
        "search_eta_seconds":23,"runtime_remaining_seconds":28}}),
        &search,
    );
    assert_eq!(sample.progress.candidate_limit_remaining, Some(6));
    assert_eq!(sample.progress.kh_per_second, Some(4.0));
    assert_eq!(sample.progress.candidates_per_second, Some(2000.0));
    let mut p = Progress {
        sample_at_ms: Some(10000),
        rate_sample_at_ms: Some(1),
        estimate_sample_at_ms: Some(9000),
        kh_per_second: Some(4.0),
        search_eta_seconds: Some(23.0),
        ..Progress::default()
    };
    age_estimates(&mut p, 10000);
    assert_eq!(p.kh_per_second, None);
    assert_eq!(p.search_eta_seconds, Some(22.0));
    age_estimates(&mut p, 16000);
    assert_eq!(p.search_eta_seconds, None);
    stop_live(&mut sample.progress);
    assert_eq!(sample.progress.kh_per_second, None);
    assert_eq!(sample.progress.candidates_per_second, None);
}

#[test]
fn checkpoint_checksum_and_identity_are_checked() {
    use sha2::{Digest, Sha256};
    let directory = tempfile::tempdir().unwrap();
    let payload = format!("WIFI-GPU-SESSION\t1\njob\t{}\nnext\t5\n", "ab".repeat(32));
    let content = format!(
        "{payload}sha256\t{}\n",
        hex::encode(Sha256::digest(payload.as_bytes()))
    );
    let path = directory.path().join("checkpoint");
    std::fs::write(&path, &content).unwrap();
    assert!(engine::checkpoint_ready(directory.path()).is_ok());
    std::fs::write(&path, content.replace("next\t5", "next\t6")).unwrap();
    assert!(engine::checkpoint_ready(directory.path()).is_err());
}
