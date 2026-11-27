use uuid::Uuid;
use wifi_recovery_api::{
    model::{Job, Progress, Search, State},
    process,
    store::Store,
};

fn job() -> Job {
    Job {
        id: Uuid::new_v4(),
        state: State::Queued,
        gpu_batch: 4096,
        search: Search {
            pattern: "password".into(),
            runtime_seconds: 0,
            candidate_limit: 0,
            ..Search::default()
        },
        filename: "synthetic.hc22000".into(),
        input_sha256: "00".repeat(32),
        input_bytes: 42,
        created_at_ms: 1,
        updated_at_ms: 1,
        first_started_at_ms: None,
        total_active_seconds: 0.0,
        attempts: Vec::new(),
        progress: Progress::default(),
        error: None,
        resume_available: false,
        resume_reason: None,
    }
}

#[test]
fn queue_order_resume_and_attempt_history_survive_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("jobs.sqlite3");
    let store = Store::open(&path).unwrap();
    let first = job();
    let second = job();
    store.insert(&first, None, 3).unwrap();
    store.insert(&second, None, 3).unwrap();
    let claimed = store.claim().unwrap().unwrap();
    assert_eq!(claimed.id, first.id);
    assert_eq!(claimed.attempts.len(), 1);
    store
        .update(first.id, |j| {
            j.state = State::Stopped;
            j.total_active_seconds = 2.0;
            Ok(())
        })
        .unwrap();
    store.enqueue(first.id).unwrap();
    assert_eq!(store.claim().unwrap().unwrap().id, second.id);
    drop(store);
    let store = Store::open(&path).unwrap();
    store.interrupt_unfinished().unwrap();
    let interrupted = store.get(second.id).unwrap().unwrap();
    assert_eq!(interrupted.state, State::Interrupted);
    assert!(!interrupted.attempts[0].timing_complete);
    let resumed = store.claim().unwrap().unwrap();
    assert_eq!(resumed.id, first.id);
    assert_eq!(resumed.total_active_seconds, 2.0);
    assert_eq!(resumed.attempts.len(), 2);
    assert!(store.claim().unwrap().is_none());
}

#[test]
fn idempotency_tombstones_and_capacity_are_durable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("jobs.sqlite3");
    let store = Store::open(&path).unwrap();
    let job = job();
    let key = Uuid::new_v4();
    store.insert(&job, Some((key, "fingerprint")), 1).unwrap();
    assert_eq!(
        store.replay(key, "fingerprint").unwrap().unwrap().id,
        job.id
    );
    assert!(store.replay(key, "changed").is_err());
    assert!(store.insert(&job, None, 1).is_err());
    store.delete(job.id).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert!(
        store
            .replay(key, "fingerprint")
            .unwrap_err()
            .to_string()
            .contains("deleted")
    );
}

#[test]
fn ownership_excludes_second_manager() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("api.lock");
    let owner = process::lock(&path).unwrap();
    assert!(process::lock(&path).is_err());
    drop(owner);
    assert!(process::lock(&path).is_ok());
}
