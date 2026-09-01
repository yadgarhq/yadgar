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
/// Returns the address to point a client at, and a handle yielding the request
/// head exactly as it arrived on the wire.
pub async fn answer_once(
    status: &'static str,
    body: &'static str,
) -> (SocketAddr, JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let addr = listener.local_addr().expect("the bound address");

    let served = tokio::spawn(async move {
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

        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len(),
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.flush().await;
        head
    });

    (addr, served)
}

/// A directory no test shares with another, and none shares with a real install.
pub fn scratch_dir(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "yadgar-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock after 1970")
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("a scratch directory");
    path
}
