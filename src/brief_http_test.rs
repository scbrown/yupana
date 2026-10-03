use super::*;
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A stand-in quipu that accepts and then never answers, counting connections.
fn stalled_quipu() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = seen.clone();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            counter.fetch_add(1, Ordering::SeqCst);
            held.push(stream); // keep it open, send nothing
        }
    });
    (endpoint, seen)
}

#[test]
fn a_spent_budget_starts_no_request_at_all() {
    // aegis-drywac: the call that would start past the budget is not sent, so
    // it cannot become a read quipu keeps executing for a hook that was killed.
    let (endpoint, seen) = stalled_quipu();
    let started = Instant::now();
    assert!(post_within(&endpoint, "/context", &serde_json::json!({}), None).is_none());
    assert!(started.elapsed() < Duration::from_millis(100));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        seen.load(Ordering::SeqCst),
        0,
        "no connection may be opened"
    );
}

#[test]
fn a_stalled_quipu_is_abandoned_at_the_budgeted_timeout_not_the_flat_one() {
    // Before the fix `post` used the flat 10s ceiling, which outlived the 10s
    // SessionStart kill. Given a small budget it must give up inside it.
    let (endpoint, seen) = stalled_quipu();
    let started = Instant::now();
    let out = post_within(
        &endpoint,
        "/context",
        &serde_json::json!({ "query": "x" }),
        Some(Duration::from_millis(300)),
    );
    let took = started.elapsed();
    assert!(out.is_none());
    assert!(
        seen.load(Ordering::SeqCst) >= 1,
        "the request was actually sent"
    );
    assert!(
        took >= Duration::from_millis(250) && took < Duration::from_secs(3),
        "gave up after {took:?}; expected ~300ms"
    );
}
