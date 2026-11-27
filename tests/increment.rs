mod support;
use support::{INPUT, Server};
use uuid::Uuid;

#[tokio::test]
#[ignore = "Requires the updated real WIFI_TEST_ENGINE; verifies range boundaries and restart/resume"]
async fn incremental_job_preserves_range_budget_identity_and_checkpoint() {
    let mut server = Server::start().await;
    let legacy = server.submit("wrongpass", 0, 0, None).await;
    assert_eq!(legacy.status(), 201);
    let key = Uuid::new_v4();
    // One literal candidate at length 22, then 26 at length 23. The known
    // synthetic password has length 23 and ends in 'd'.
    let search = serde_json::json!({"pattern":"synthetic-test-passwor?l", "increment":true,
        "increment_min":22, "increment_max":23, "candidate_limit":1});
    let response = server
        .submit_search(search.clone(), Some(key), INPUT.to_vec())
        .await;
    assert_eq!(response.status(), 201);
    let job: serde_json::Value = response.json().await.unwrap();
    let id = job["id"].as_str().unwrap();
    server.wait(id, &["limit_reached"]).await;
    let detail = server.get(&format!("/v1/jobs/{id}")).await;
    assert_eq!(detail["progress"]["total_candidates"], 27);
    assert_eq!(detail["progress"]["tested_candidates"], 1);
    server.restart().await;
    let detail = server.get(&format!("/v1/jobs/{id}")).await;
    assert_eq!(detail["search"]["increment_min"], 22);
    let retry = server
        .submit_search(search.clone(), Some(key), INPUT.to_vec())
        .await;
    assert_eq!(retry.status(), 200);
    assert_eq!(retry.json::<serde_json::Value>().await.unwrap()["id"], id);
    let mut changed = search.clone();
    changed["increment_min"] = 23.into();
    assert_eq!(
        server
            .submit_search(changed, Some(key), INPUT.to_vec())
            .await
            .status(),
        409
    );
    for attempt in 2..=5 {
        assert!(server.action(id, "resume").await.status().is_success());
        let expected = if attempt == 5 {
            "recovered"
        } else {
            "limit_reached"
        };
        let detail = server.wait(id, &[expected]).await;
        assert_eq!(detail["attempts"].as_array().unwrap().len(), attempt);
    }
    let result = server.get(&format!("/v1/jobs/{id}/result")).await;
    assert_eq!(
        result["engine_result"]["matches"][0]["password_hex"],
        hex::encode("synthetic-test-password")
    );
}

#[tokio::test]
#[ignore = "Requires the updated real WIFI_TEST_ENGINE; validates HTTP range requests"]
async fn incremental_http_preview_defaults_and_rejections() {
    let server = Server::start().await;
    let preview = server
        .http
        .post(format!("{}/v1/patterns/preview", server.url))
        .json(&serde_json::json!({"pattern":"Home?d?d?d?d?d?d", "increment":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(preview.status(), 200);
    let preview: serde_json::Value = preview.json().await.unwrap();
    assert_eq!(preview["candidate_count"], 1_110_000);
    assert_eq!(preview["lengths"].as_array().unwrap().len(), 3);
    for invalid in [
        serde_json::json!({"pattern":"password", "increment_min":8}),
        serde_json::json!({"pattern":"password", "increment":true, "increment_min":9}),
        serde_json::json!({"pattern":"password", "increment":true, "increment_max":64}),
        serde_json::json!({"pattern":"password", "increment":true, "unknown":1}),
    ] {
        assert_eq!(
            server
                .submit_search(invalid, None, INPUT.to_vec())
                .await
                .status(),
            400
        );
    }
    let legacy = server.submit("wrongpass", 0, 0, None).await;
    assert_eq!(legacy.status(), 201);
    let key = Uuid::new_v4();
    let default = serde_json::json!({"pattern":"wrongpass", "increment":true});
    let response = server
        .submit_search(default, Some(key), INPUT.to_vec())
        .await;
    assert_eq!(response.status(), 201);
    let explicit = serde_json::json!({"pattern":"wrongpass", "increment":true, "increment_min":8, "increment_max":63});
    assert_eq!(
        server
            .submit_search(explicit, Some(key), INPUT.to_vec())
            .await
            .status(),
        200
    );
}
