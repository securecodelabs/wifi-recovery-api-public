pub mod support;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use support::Server;

#[tokio::test]
#[ignore = "Dedicated AMD hardware acceptance: two 67-million-candidate API jobs, minimum 650 kH/s sustained"]
async fn sustained_api_pattern_throughput() {
    assert!(
        std::env::var_os("WIFI_TEST_HIP").is_some(),
        "Set WIFI_TEST_HIP=1 for this hardware test"
    );
    let server = Server::start().await;
    for round in 1..=2 {
        let response = server
            .submit_bytes(
                "?H?H?H?H?H?H?H?H?H?H",
                67_108_864,
                0,
                None,
                include_bytes!("fixtures/single-wpa2.hc22000").to_vec(),
            )
            .await;
        assert!(response.status().is_success());
        let job: Value = response.json().await.unwrap();
        assert_eq!(job["gpu_batch"], 262144);
        measure(&server, job["id"].as_str().unwrap(), round).await;
    }
}

async fn measure(server: &Server, id: &str, round: u32) {
    let clock = Instant::now();
    let mut first = None;
    let mut last = None;
    let mut peak_average = 0.0_f64;
    let mut previous_count = 0;
    loop {
        let job = server.get(&format!("/v1/jobs/{id}")).await;
        assert_ne!(job["state"], "failed", "{job}");
        let progress = &job["progress"];
        let count = progress["tested_candidates"].as_u64().unwrap_or(0);
        let elapsed = progress["search_elapsed_seconds"].as_f64().unwrap_or(0.0);
        peak_average = peak_average.max(progress["kh_per_second"].as_f64().unwrap_or(0.0));
        if count != previous_count && (13_421_773..=53_687_091).contains(&count) {
            let sample = (count, clock.elapsed().as_secs_f64(), elapsed);
            first.get_or_insert(sample);
            last = Some(sample);
        }
        previous_count = count;
        if job["state"] == "limit_reached" {
            let result = server.get(&format!("/v1/jobs/{id}/result")).await;
            let start = first.expect("Missing central-window start sample");
            let end = last.expect("Missing central-window end sample");
            let engine_khs = (end.0 - start.0) as f64 / (end.2 - start.2) / 1000.0;
            let observed_khs = (end.0 - start.0) as f64 / (end.1 - start.1) / 1000.0;
            let run_average = result["engine_result"]["telemetry"]["candidates_per_second"]
                .as_f64()
                .unwrap()
                / 1000.0;
            println!(
                "{}",
                json!({"round":round,"job":id,"candidates":count,"sustained_observer_khs":observed_khs,
                "sustained_engine_khs":engine_khs,"run_average_khs":run_average,"peak_live_average_khs":peak_average,
                "batch":262144,"window":"20-80 percent","record":"single exact WPA2 EAPOL v2","pattern":"ten uppercase hexadecimal bytes"})
            );
            assert!(
                observed_khs >= 650.0 && engine_khs >= 650.0,
                "650 kH/s sustained target not met"
            );
            return;
        }
        assert!(
            clock.elapsed() < Duration::from_secs(240),
            "Performance run timed out: {job}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
