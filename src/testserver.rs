//! One request, one answer, and what arrived — for tests that need a socket.
//!
//! Deliberately not a web framework and deliberately not shared state: each call
//! binds its own ephemeral port, serves exactly one connection and stops. A test
//! that needs a second request starts a second server, so no test can ever
//! observe another test's traffic.
//!
//! It exists for the properties that live in the request itself. A credential
//! attached inside a request builder and a status code checked on the way back
//! are both invisible to a pure function, which is why deleting either survived
//! a full suite.

use std::net::SocketAddr;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::task::JoinHandle;

/// Bind an ephemeral port and answer the first request with *status* and *body*.
///
/// Returns the address to point a client at, and a handle yielding THE WHOLE
/// REQUEST as it arrived on the wire — head, blank line, and the body when the
/// request declared a `Content-Length`.
///
/// **THE BODY IS READ TOO, and it was not always.** A credential in a header is
/// visible in the head alone; an enrolment secret is a field in a JSON body, and
/// a test that stopped at the blank line could not tell a client sending the
/// `secret` field from one sending the whole base64 blob — which the contract
/// says are different requests, one of which the gateway refuses.
pub async fn answer_once(
    status: &'static str,
    body: &'static str,
) -> (SocketAddr, JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let addr = listener.local_addr().expect("the bound address");

    let served = tokio::spawn(async move { serve_one(&listener, status, body).await });
    (addr, served)
}

/// [`answer_once`] for a conversation: answer each request in turn with the
/// next of *answers*, one connection each, and yield every request as it
/// arrived. Each response says `Connection: close`, so the client opens a new
/// connection per request and the order is the order sent.
pub async fn answer_each(
    answers: Vec<(&'static str, &'static str)>,
) -> (SocketAddr, JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let addr = listener.local_addr().expect("the bound address");
    let served = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in answers {
            requests.push(serve_one(&listener, status, body).await);
        }
        requests
    });
    (addr, served)
}

/// [`answer_each`], then accept ONE more connection and never answer it: a
/// gateway that stalls after the first requests. The handle never completes;
/// dropping the runtime ends it.
pub async fn answer_each_then_hold(answers: Vec<(&'static str, &'static str)>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let addr = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        for (status, body) in answers {
            serve_one(&listener, status, body).await;
        }
        let held = listener.accept().await;
        std::future::pending::<()>().await;
        drop(held);
    });
    addr
}

/// Accept one connection on *listener*, read the whole request, answer it.
async fn serve_one(
    listener: &tokio::net::TcpListener,
    status: &'static str,
    body: &'static str,
) -> String {
    let (mut socket, _) = listener.accept().await.expect("one connection");

    // READ UNTIL THE HEAD IS COMPLETE, not once. A single `read` returns
    // whatever one TCP segment happened to carry, so a test asserting on a
    // header would pass or fail on packet boundaries.
    let mut head = String::new();
    let mut buf = [0u8; 1024];
    while !head.contains("\r\n\r\n") {
        match socket.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => head.push_str(&String::from_utf8_lossy(&buf[..n])),
        }
    }

    // KEEP READING UNTIL THE DECLARED BODY HAS ARRIVED. reqwest sends the
    // head and the body in separate writes, so a reader that stopped at the
    // blank line would see an empty body on every run and a test asserting
    // on a field would pass for a client that sent nothing at all.
    //
    // MEASURED FROM THE BLANK LINE, not from what has arrived: the head loop
    // above stops on the first read that COMPLETES the head, and that read
    // may already carry part of the body. Counting from `head.len()` would
    // then wait for bytes nobody is going to send, and the test would hang
    // rather than fail.
    let wanted = head
        .find("\r\n\r\n")
        .zip(content_length(&head))
        .map(|(blank, len)| blank + 4 + len);
    while wanted.is_some_and(|w| head.len() < w) {
        match socket.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => head.push_str(&String::from_utf8_lossy(&buf[..n])),
        }
    }

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.flush().await;
    head
}

/// The declared body length, or `None` when the request declared none.
fn content_length(head: &str) -> Option<usize> {
    head.lines()
        .take_while(|l| !l.is_empty())
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim())
        })
        .and_then(|v| v.parse().ok())
}

/// A directory no test shares with another, and none shares with a real install.
///
/// A COUNTER, NOT THE CLOCK (ledger 1195). Every test in one binary shares the
/// pid, and two threads reading the same nanosecond got the same path:
/// `create_dir_all` succeeded for both, and the two tests silently shared one
/// directory. The pid keeps parallel test binaries apart and the counter keeps
/// this binary's calls apart, so within one pid namespace no two live calls
/// can get the same path. A leftover from an earlier process that had the same
/// pid is removed first; `create_dir` then makes the leaf. Across pid
/// namespaces sharing one temp directory (containers), a collision would
/// delete or share a directory silently, not panic.
pub fn scratch_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("yadgar-{label}-{}-{n}", std::process::id()));
    // A leftover from an earlier run with the same pid is not a collision
    // between two live tests, so it is cleared rather than refused.
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir(&path).expect("a scratch directory nobody else holds");
    path
}

#[test]
fn scratch_dirs_never_repeat_under_one_label_across_threads() {
    let dirs: Vec<std::path::PathBuf> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|_| s.spawn(|| (0..100).map(|_| scratch_dir("same")).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    let unique: std::collections::HashSet<_> = dirs.iter().collect();
    assert_eq!(unique.len(), dirs.len());
    for dir in &dirs {
        let _ = std::fs::remove_dir(dir);
    }
}
