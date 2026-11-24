use reqwest::{Client, Response, multipart};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;

pub const INPUT: &[u8] = include_bytes!("../fixtures/synthetic.hc22000");
pub struct Server {
    pub child: Child,
    pub data: tempfile::TempDir,
    pub url: String,
    pub http: Client,
}

impl Server {
    pub async fn start() -> Self {
        let data = tempfile::tempdir().unwrap();
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        drop(socket);
        let url = format!("http://{address}");
        let child = launch(data.path(), &address.to_string());
        let server = Self {
            child,
            data,
            url,
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap(),
        };
        server.ready().await;
        server
    }

    pub async fn restart(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        self.child = launch(self.data.path(), self.url.trim_start_matches("http://"));
        self.ready().await;
    }

    async fn ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            if self
                .http
                .get(format!("{}/readyz", self.url))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "API failed to start; see {}",
                self.data.path().display()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn submit(
        &self,
        pattern: &str,
        limit: u64,
        runtime: u64,
        key: Option<Uuid>,
    ) -> Response {
        self.submit_bytes(pattern, limit, runtime, key, INPUT.to_vec())
            .await
    }

    pub async fn submit_bytes(
        &self,
        pattern: &str,
        limit: u64,
        runtime: u64,
        key: Option<Uuid>,
        bytes: Vec<u8>,
    ) -> Response {
        let form = multipart::Form::new()
            .text(
                "metadata",
                json!({"pattern":pattern,"candidate_limit":limit,"runtime_seconds":runtime})
                    .to_string(),
            )
            .part(
                "file",
                multipart::Part::bytes(bytes).file_name("synthetic.hc22000"),
            );
        let mut request = self
            .http
            .post(format!("{}/v1/jobs", self.url))
            .multipart(form);
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key.to_string());
        }
        request.send().await.unwrap()
    }

    pub async fn get(&self, path: &str) -> Value {
        self.http
            .get(format!("{}{path}", self.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    pub async fn action(&self, id: &str, action: &str) -> Response {
        self.http
            .post(format!("{}/v1/jobs/{id}/{action}", self.url))
            .send()
            .await
            .unwrap()
    }

    pub async fn wait(&self, id: &str, expected: &[&str]) -> Value {
        let deadline = Instant::now() + Duration::from_secs(75);
        loop {
            let job = self.get(&format!("/v1/jobs/{id}")).await;
            let state = job["state"].as_str().unwrap();
            if expected.contains(&state) {
                return job;
            }
            assert!(state != "failed", "Job failed: {job}");
            assert!(Instant::now() < deadline, "Timed out: {job}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn launch(data: &std::path::Path, address: &str) -> Child {
    let exe = std::env::var_os("WIFI_TEST_API")
        .map(PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_wifi-recovery-api").into());
    let engine =
        std::env::var_os("WIFI_TEST_ENGINE").expect("Set WIFI_TEST_ENGINE to WiFi GPU 0.17.2");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data.join("api.log"))
        .unwrap();
    let mut command = Command::new(exe);
    command
        .args(["--listen", address, "--data-dir"])
        .arg(data)
        .arg("--engine")
        .arg(engine);
    if std::env::var_os("WIFI_TEST_HIP").is_none() {
        command.arg("--cpu");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap()
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Guardian cleanup can take a batch plus its bounded stop grace period.
        let path = self.data.path().join("engine.lock");
        let deadline = Instant::now() + Duration::from_secs(20);
        while wifi_recovery_api::process::lock(&path).is_err() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
