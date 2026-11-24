mod support;
use serde_json::Value;
use sha2::{Digest, Sha256};
use support::{INPUT, Server};
use uuid::Uuid;

async fn accepted(response: reqwest::Response) -> Value {
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    response.json().await.unwrap()
}

#[tokio::test]
#[ignore = "Requires WIFI_TEST_ENGINE; exercises the real engine executable"]
async fn recovery_results_upload_identity_and_retries() {
    let mut server = Server::start().await;
    let key = Uuid::new_v4();
    let job = accepted(
        server
            .submit("synthetic-test-password", 0, 0, Some(key))
            .await,
    )
    .await;
    let id = job["id"].as_str().unwrap();
    assert_eq!(job["input_sha256"], hex::encode(Sha256::digest(INPUT)));
    assert_eq!(
        std::fs::read(
            server
                .data
                .path()
                .join("jobs")
                .join(id)
                .join("input.hc22000")
        )
        .unwrap(),
        INPUT
    );
    let finished = server.wait(id, &["recovered"]).await;
    assert_eq!(finished["progress"]["recovered_records"], 12);
    assert_eq!(finished["attempts"][0]["timing_complete"], true);
    let result = server.get(&format!("/v1/jobs/{id}/result")).await;
    assert_eq!(result["job_id"], id);
    assert_eq!(
        result["engine_result"]["matches"][0]["password_hex"],
        hex::encode("synthetic-test-password")
    );
    let (a, b) = tokio::join!(
        server.submit("synthetic-test-password", 0, 0, Some(key)),
        server.submit("synthetic-test-password", 0, 0, Some(key))
    );
    assert_eq!(accepted(a).await["id"], id);
    assert_eq!(accepted(b).await["id"], id);
    assert_eq!(
        server
            .submit("wrong-password", 0, 0, Some(key))
            .await
            .status(),
        409
    );
    let owned_id = id.to_owned();
    server.restart().await;
    assert_eq!(
        accepted(
            server
                .submit("synthetic-test-password", 0, 0, Some(key))
                .await
        )
        .await["id"],
        owned_id
    );
    assert_eq!(
        server
            .http
            .delete(format!("{}/v1/jobs/{owned_id}", server.url))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        server
            .submit("synthetic-test-password", 0, 0, Some(key))
            .await
            .status(),
        409
    );
}

#[tokio::test]
#[ignore = "Requires WIFI_TEST_ENGINE; real queue, stop, checkpoint and resume"]
async fn queue_stop_resume_limits_and_crash_ownership() {
    let mut server = Server::start().await;
    let running = accepted(server.submit("unknown?d?d?d?d?d?d?d?d", 0, 0, None).await).await;
    let id = running["id"].as_str().unwrap();
    server.wait(id, &["running"]).await;
    let queued = accepted(server.submit("synthetic-test-password", 0, 0, None).await).await;
    let queued_id = queued["id"].as_str().unwrap();
    let stopped = accepted(server.action(queued_id, "stop").await).await;
    assert_eq!(stopped["state"], "stopped");
    assert_eq!(stopped["resume_available"], true);
    assert!(stopped["attempts"].as_array().unwrap().is_empty());
    accepted(server.action(id, "stop").await).await;
    let stopped = server.wait(id, &["stopped"]).await;
    assert_eq!(stopped["progress"]["fresh"], false);
    assert_eq!(stopped["resume_available"], true);
    accepted(server.action(queued_id, "resume").await).await;
    server.wait(queued_id, &["recovered"]).await;
    accepted(server.action(id, "resume").await).await;
    server.wait(id, &["running"]).await;
    let owned_id = id.to_owned();
    server.restart().await;
    let interrupted = server.get(&format!("/v1/jobs/{owned_id}")).await;
    assert!(
        wifi_recovery_api::process::lock(&server.data.path().join("engine.lock")).is_ok(),
        "The old engine must have been reaped before readiness"
    );
    assert_eq!(interrupted["state"], "interrupted");
    assert_eq!(interrupted["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(interrupted["attempts"][1]["timing_complete"], false);
    let limited = accepted(server.submit("unknown?d?d?d?d", 1, 0, None).await).await;
    server
        .wait(limited["id"].as_str().unwrap(), &["limit_reached"])
        .await;
    let exhausted = accepted(server.submit("wrong-password", 0, 0, None).await).await;
    server
        .wait(exhausted["id"].as_str().unwrap(), &["exhausted"])
        .await;
}

#[tokio::test]
#[ignore = "Requires WIFI_TEST_ENGINE; malformed uploads and checkpoint handling"]
async fn rejects_invalid_uploads_and_corrupt_checkpoints() {
    let server = Server::start().await;
    let invalid = reqwest::multipart::Form::new()
        .text("metadata", "{\"pattern\":\"password\"}")
        .part("file", reqwest::multipart::Part::bytes(b"invalid".to_vec()));
    let response = server
        .http
        .post(format!("{}/v1/jobs", server.url))
        .multipart(invalid)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        std::fs::read_dir(server.data.path().join("staging"))
            .unwrap()
            .count(),
        0
    );
    let job = accepted(server.submit("unknown?d?d?d?d", 1, 0, None).await).await;
    let id = job["id"].as_str().unwrap();
    server.wait(id, &["limit_reached"]).await;
    std::fs::write(
        server.data.path().join("jobs").join(id).join("checkpoint"),
        "corrupt",
    )
    .unwrap();
    assert_eq!(server.action(id, "resume").await.status(), 409);
}

#[tokio::test]
#[ignore = "Requires WIFI_TEST_ENGINE; live multi-SSID throughput and runtime budgets"]
async fn live_rates_resources_and_runtime_resume() {
    use std::time::{Duration, Instant};
    let server = Server::start().await;
    let bytes = [INPUT, include_bytes!("fixtures/additional-ssids.hc22000")].concat();
    let job = accepted(
        server
            .submit_bytes("unknown?d?d?d?d?d?d?d?d", 0, 12, None, bytes)
            .await,
    )
    .await;
    let id = job["id"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let live = loop {
        let value = server.get(&format!("/v1/jobs/{id}")).await;
        if value["progress"]["kh_per_second"]
            .as_f64()
            .is_some_and(|n| n > 0.0)
            && value["progress"]["candidates_per_second"]
                .as_f64()
                .is_some_and(|n| n > 0.0)
        {
            break value;
        }
        assert!(
            Instant::now() < deadline && value["state"] != "failed",
            "No live sample: {value}"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    };
    let p = &live["progress"];
    assert!(
        p["kh_per_second"].as_f64().unwrap() * 1000.0
            > p["candidates_per_second"].as_f64().unwrap() * 1.1
    );
    assert!(p["candidate_hex"].as_str().is_some());
    assert!(p["estimated_full_seconds"].as_f64().unwrap() > 0.0);
    assert!(p["search_eta_seconds"].as_f64().unwrap() > 0.0);
    let resources = server.get("/v1/system").await;
    #[cfg(unix)]
    assert!(
        resources["resources"]["memory_total_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    if std::env::var_os("WIFI_TEST_HIP").is_some() {
        assert!(
            resources["resources"]["gpus"]
                .as_array()
                .unwrap()
                .iter()
                .any(|gpu| gpu["utilization_percent"].as_u64().unwrap_or(0) > 0)
        );
    }
    let limited = server.wait(id, &["limit_reached"]).await;
    let previous = limited["total_active_seconds"].as_f64().unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        server.get(&format!("/v1/jobs/{id}")).await["total_active_seconds"],
        previous
    );
    accepted(server.action(id, "resume").await).await;
    server.wait(id, &["running"]).await;
    accepted(server.action(id, "stop").await).await;
    let stopped = server.wait(id, &["stopped"]).await;
    assert_eq!(stopped["attempts"].as_array().unwrap().len(), 2);
    assert!(stopped["total_active_seconds"].as_f64().unwrap() >= previous);
}
