//! A binary built without `mcp` must FAIL `serve` and `daemon`, never exit 0
//! (aegis-6h3ycl). The stub used to print a note and exit 0: an MCP client saw
//! only a closed connection, and any install wrapper checking the exit status
//! reported a server that never ran as healthy. Exit 2 matches `promote`
//! without `quipu`.
#![cfg(not(feature = "mcp"))]

use std::process::{Command, Stdio};

fn run(sub: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_yupana"))
        .arg(sub)
        .stdin(Stdio::null())
        .output()
        .expect("spawn yupana")
}

#[test]
fn serve_without_mcp_exits_2_and_names_the_feature() {
    let out = run("serve");
    assert_eq!(
        out.status.code(),
        Some(2),
        "serve stub must not report success"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("--features mcp"));
}

#[test]
fn daemon_without_mcp_exits_2_and_names_the_feature() {
    let out = run("daemon");
    assert_eq!(
        out.status.code(),
        Some(2),
        "daemon stub must not report success"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("--features mcp"));
}
