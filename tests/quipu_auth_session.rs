//! Exercise separate real hook processes against an isolated HTTP fixture.
#![cfg(feature = "quipu")]
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

struct Server {
    address: std::net::SocketAddr,
    knots: Arc<AtomicUsize>,
    queries: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new(rejected: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let knots = Arc::new(AtomicUsize::new(0));
        let queries = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (k, q, stopped) = (knots.clone(), queries.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            for connection in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let mut connection = connection.unwrap();
                connection
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 8192];
                loop {
                    let n = connection.read(&mut buffer).unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(split) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        let length = String::from_utf8_lossy(&bytes[..split])
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|s| s.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= split + 4 + length {
                            break;
                        }
                    }
                }
                let knot = bytes.starts_with(b"POST /knot ");
                if knot {
                    k.fetch_add(1, Ordering::SeqCst);
                } else {
                    q.fetch_add(1, Ordering::SeqCst);
                }
                let (status, body) = if knot && rejected {
                    ("401 Unauthorized", "fixture-rejected-secret")
                } else if knot {
                    ("200 OK", "{\"count\":1,\"tx_id\":1}")
                } else {
                    ("200 OK", "{\"count\":0,\"rows\":[]}")
                };
                write!(connection, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        Self {
            address,
            knots,
            queries,
            stop,
            thread: Some(thread),
        }
    }
    fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn hook(
    home: &std::path::Path,
    root: &std::path::Path,
    session: &str,
    explicit: Option<&std::path::Path>,
) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_yupana"));
    command
        .args(["hook", "pre-bash"])
        .current_dir(root)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("YUPANA_DISK_STATE_DIR", home.join("disk"))
        .env_remove("QUIPU_AUTH_TOKEN")
        .env_remove("QUIPU_AUTH_TOKEN_FILE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = explicit {
        command.env("QUIPU_AUTH_TOKEN_FILE", path);
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(serde_json::json!({"session_id":session,"cwd":root,"tool_name":"Bash","tool_input":{"command":"cargo build"}}).to_string().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn setup(root: &std::path::Path, server: &Server) {
    std::fs::create_dir_all(root.join(".bobbin")).unwrap();
    std::fs::write(
        root.join(".bobbin/config.toml"),
        format!(
            "[yupana.quipu]\nenabled = true\nendpoint = {:?}\n",
            server.endpoint()
        ),
    )
    .unwrap();
}

#[test]
fn rejected_auth_is_reported_once_and_survives_fresh_hook_processes() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let server = Server::new(true);
    setup(root.path(), &server);
    std::fs::create_dir_all(home.path().join(".config/quipu")).unwrap();
    std::fs::write(
        home.path().join(".config/quipu/token"),
        "fixture-rejected-secret",
    )
    .unwrap();
    let mut notices = String::new();
    for _ in 0..4 {
        notices.push_str(&hook(home.path(), root.path(), "session-one", None));
    }
    assert_eq!(server.knots.load(Ordering::SeqCst), 1);
    assert_eq!(
        notices.matches("yupana: Quipu credential rejected").count(),
        1
    );
    assert!(!notices.contains("fixture-rejected-secret"));
    assert!(
        server.queries.load(Ordering::SeqCst) >= 4,
        "public reads remain available"
    );
    hook(home.path(), root.path(), "session-two", None);
    hook(home.path(), root.path(), "session-two", None);
    assert_eq!(
        server.knots.load(Ordering::SeqCst),
        2,
        "another session is independent"
    );
    let another = Server::new(true);
    setup(root.path(), &another);
    hook(home.path(), root.path(), "session-one", None);
    hook(home.path(), root.path(), "session-one", None);
    assert_eq!(
        another.knots.load(Ordering::SeqCst),
        1,
        "another endpoint is independent"
    );
}

#[test]
fn missing_explicit_file_never_falls_back_or_attempts_a_write() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let server = Server::new(false);
    setup(root.path(), &server);
    std::fs::create_dir_all(home.path().join(".config/quipu")).unwrap();
    std::fs::write(
        home.path().join(".config/quipu/token"),
        "fixture-good-token",
    )
    .unwrap();
    let missing = home.path().join("missing-issued-file");
    let mut notices = String::new();
    for _ in 0..4 {
        notices.push_str(&hook(
            home.path(),
            root.path(),
            "missing-session",
            Some(&missing),
        ));
    }
    assert_eq!(server.knots.load(Ordering::SeqCst), 0);
    assert_eq!(
        notices.matches("yupana: Quipu credential missing").count(),
        1
    );
    for _ in 0..3 {
        hook(home.path(), root.path(), "valid-session", None);
    }
    assert_eq!(
        server.knots.load(Ordering::SeqCst),
        2,
        "positive canonical-file control"
    );
}
