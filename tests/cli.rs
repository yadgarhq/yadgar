//! What the commands actually PRINT, read off the built binary's stdout.
//!
//! **An extraction made for testability is not wired by being extracted.** Both
//! reports in this client were pulled out of their `println!` so a test could
//! read them, and both were then tested at the wrong end: `health::report_lines`
//! had its text pinned while deleting `verify`'s loop over it — so `verify`
//! printed NOTHING AT ALL — left the suite green, and `main::report` was one
//! private function ending in `println!` that nothing covered at all. A unit
//! test on the lines proves the lines; only the process proves the caller.
//!
//! So this runs the real binary, with `HOME` pointed at a scratch directory.
//! `install` is deliberately NOT exercised here: it goes through
//! `resolve_durable_command`, which judges the path of the binary running the
//! test — green in an ordinary checkout, refused inside `.claude/worktrees` —
//! and a test whose colour depends on where somebody cloned the repository is
//! worse than no test. `uninstall` and `verify` call neither, and between them
//! they reach every line either report can print.
//!
//! The fixtures are literal TEXT rather than built through the crate: this is a
//! binary-only package with no library target, so nothing here can call
//! `hooks::merge`. Writing the JSON by hand is also the more honest fixture —
//! it is what is actually on a machine, rather than what this crate believes it
//! wrote there.
//!
//! **`#![cfg(unix)]`, and the reason is a destroyed machine rather than a
//! platform difference.** These are the only tests in this repository that go
//! through `main::home()`, and therefore through `dirs::home_dir()`. On Unix
//! that reads `$HOME`, so the binary can be pointed at a scratch directory. On
//! Windows it is `known_folder_profile()` — `SHGetKnownFolderPath` with
//! `FOLDERID_Profile` — which reads NEITHER `HOME` nor `USERPROFILE`. Running
//! these there would run a real `yaadgaar uninstall` against the developer's own
//! `~/.claude`: their hooks stripped, their MCP entry removed, their rules file
//! deleted. `install/tests/mod.rs` states the invariant these would break — a
//! test that writes into somebody's live settings has already failed at the
//! thing this module is for — and it held until now only because every other
//! test takes the home as a parameter.
//!
//! Setting `USERPROFILE` as well would not help and would be worse: it buys
//! nothing against the known-folder API, and leaves a test that LOOKS hermetic.
//! What both reports SAY is platform-independent and covered by the
//! `report_lines` unit tests, which run everywhere; only the wiring is gated.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A fresh scratch home directory, with `.claude` already in it.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("yaadgaar-cli-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".claude")).expect("scratch home");
    dir
}

/// Run the built binary with *home* as `HOME`, and give back what it said.
fn run(home: &Path, subcommand: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_yaadgaar"))
        .arg(subcommand)
        // HOME, not the current directory: `main::home()` is the one place that
        // reads it, and everything below is derived from what it returns.
        .env("HOME", home)
        .output()
        .expect("the binary under test did not run")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout was not UTF-8")
}

/// Everything `install` writes, written by hand.
///
/// The hook command matches `hooks::is_managed` — a binary whose stem is
/// `yaadgaar`, with `hook` as its first argument — and the reference line is
/// the absolute `@` import `rules::reference_line` builds.
fn seed_an_installed_home(home: &Path) {
    let claude = home.join(".claude");
    let rules = claude.join("yadgar-rules.md");
    std::fs::write(
        claude.join("settings.json"),
        r#"{"model":"opus","hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"/usr/local/bin/yaadgaar hook stop-checkpoint"}]}]}}"#,
    )
    .unwrap();
    std::fs::write(
        home.join(".claude.json"),
        r#"{"numStartups":41,"mcpServers":{"yadgar":{"type":"stdio","command":"/usr/local/bin/yaadgaar","args":["serve"]}}}"#,
    )
    .unwrap();
    std::fs::write(&rules, "# the rules yadgar owns\n").unwrap();
    std::fs::write(
        claude.join("CLAUDE.md"),
        format!("@{}\n# my own instructions\n", rules.display()),
    )
    .unwrap();
}

#[test]
fn verify_prints_its_report_and_exits_non_zero() {
    // The wiring `health::report_lines` was extracted FOR. Deleting the loop in
    // `verify` — so a scheduled check prints nothing whatsoever and only its
    // exit status says anything — left the whole suite green.
    let home = scratch("verify-drift");
    std::fs::write(home.join(".claude").join("CLAUDE.md"), "# mine\n").unwrap();

    let output = run(&home, "verify");
    let said = stdout_of(&output);

    assert!(
        said.lines()
            .any(|l| l.starts_with("yaadgaar verify: DRIFT")),
        "verify reported nothing on a machine with no install: {said:?}"
    );
    assert!(
        said.contains("not registered"),
        "the findings themselves went unprinted: {said:?}"
    );
    assert!(
        !output.status.success(),
        "verify found drift and exited zero"
    );
}

#[test]
fn an_uninstall_that_removed_nothing_says_so_out_loud() {
    // A command that prints NOTHING AT ALL reads as a command that did not run.
    // Deleting the `nothing` block was measured and invisible.
    let home = scratch("uninstall-nothing");

    let output = run(&home, "uninstall");
    let said = stdout_of(&output);

    assert_eq!(
        said.trim_end(),
        "nothing to do — none of yadgar's registrations were there.",
        "an uninstall that removed nothing did not say so"
    );
    assert!(output.status.success(), "an uninstall of nothing failed");
}

#[test]
fn an_uninstall_names_every_file_it_actually_touched() {
    // The four gated lines, through the process that prints them. Inverting all
    // four gates — the fix this PR exists for, exactly reversed — left the
    // suite green, because nothing read a line of this report.
    let home = scratch("uninstall-reports");
    seed_an_installed_home(&home);

    let output = run(&home, "uninstall");
    let said = stdout_of(&output);
    let lines: Vec<&str> = said.lines().collect();

    assert_eq!(lines.len(), 4, "{said:?}");
    assert!(lines[0].starts_with("removed 1 hook(s) in "), "{said:?}");
    assert!(lines[0].ends_with("settings.json"), "{said:?}");
    assert!(
        lines[1].starts_with("removed the MCP entry in "),
        "{said:?}"
    );
    assert!(lines[1].ends_with(".claude.json"), "{said:?}");
    assert!(lines[2].starts_with("removed the rules file "), "{said:?}");
    assert!(lines[2].ends_with("yadgar-rules.md"), "{said:?}");
    assert_eq!(lines[3], "removed the reference line in CLAUDE.md");
    assert!(
        !said.contains("nothing to do"),
        "an uninstall that removed four things also claimed it did nothing: {said:?}"
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_uninstall_leaves_the_configs_of_a_machine_it_was_installed_on() {
    // The destructive probe, through the BINARY rather than through the library
    // — which is how it was found. Neither file below holds a scalar anywhere,
    // and the vacancy rule hunted for scalars, so an uninstall deleted both:
    // somebody's permissions configuration and a third party's MCP server.
    let home = scratch("uninstall-keeps-configs");
    let claude = home.join(".claude");
    std::fs::write(
        claude.join("settings.json"),
        r#"{"permissions":{"allow":[],"deny":[],"ask":[]},"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"/usr/local/bin/yaadgaar hook stop-checkpoint"}]}]}}"#,
    )
    .unwrap();
    std::fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"other-server":{},"yadgar":{"type":"stdio","command":"/usr/local/bin/yaadgaar","args":["serve"]}}}"#,
    )
    .unwrap();

    let output = run(&home, "uninstall");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let settings = std::fs::read_to_string(claude.join("settings.json"))
        .expect("somebody's permissions configuration was deleted");
    assert!(settings.contains("permissions"), "{settings}");
    assert!(!settings.contains("stop-checkpoint"), "{settings}");

    let config = std::fs::read_to_string(home.join(".claude.json"))
        .expect("a third party's MCP registration was deleted");
    assert!(config.contains("other-server"), "{config}");
    assert!(!config.contains("\"yadgar\""), "{config}");
}

#[test]
fn enrol_is_a_command_the_binary_actually_has() {
    // THE WIRING, and nothing about the decoding. `login::enrol` and every rule
    // it enforces are covered as units; what no unit can see is a `Command`
    // variant that parses and dispatches nowhere, or a subcommand somebody
    // named `enroll` in one place and `enrol` in another. Clap answers an
    // unknown subcommand with a usage error on stderr, which is what this tells
    // apart from a refusal the command itself produced.
    //
    // The token is deliberately unusable, so nothing reaches a network: the
    // point is that the argument was PARSED and handed to the enrolment path.
    let home = scratch("enrol-wiring");
    let output = Command::new(env!("CARGO_BIN_EXE_yaadgaar"))
        .args(["enrol", "this-is-not-a-token"])
        .env("HOME", &home)
        // Its own directory, so a refused enrolment cannot be confused with —
        // or write over — the config of whoever is running the tests.
        .env("YADGAR_CONFIG_DIR", home.join("config"))
        .output()
        .expect("the binary under test did not run");

    let said = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(
        !output.status.success(),
        "an unusable token enrolled: {said}"
    );
    assert!(
        !said.contains("unrecognized subcommand") && !said.contains("unexpected argument"),
        "`enrol` is not a subcommand this binary has: {said}"
    );
    assert!(
        said.contains("not an enrolment token"),
        "the refusal did not come from the enrolment path: {said}"
    );
    assert!(
        !home.join("config").join("config.json").exists(),
        "a refused enrolment wrote a config"
    );
    let _ = std::fs::remove_dir_all(&home);
}

// ============================================================================
// `--password-stdin` (ledger 638) — unattended `login` and `enrol`, driven
// with NO CONTROLLING TERMINAL AT ALL.
//
// **PIPED STDIN IS NOT ENOUGH ON ITS OWN.** `Command::new(...).stdin(piped)`
// still leaves the child in this test process's own session, so it inherits
// whatever controlling terminal the test process has — and `rpassword` opens
// `/dev/tty` directly (rpassword 7.5.4, `src/unix.rs`, `DEFAULT_INPUT_PATH`),
// bypassing stdin entirely. A stdout assertion of the form "no `Password:`
// prompt appeared" cannot see that prompt either, because `rpassword`'s
// default OUTPUT is `/dev/tty` too. So every test below runs the binary
// through `setsid`, which starts it in a brand-new session with no
// controlling terminal — the one thing that actually makes `/dev/tty`
// unopenable, which is what turns "did it prompt" into an observable exit
// status rather than something these tests are structurally blind to.
//
// **`setsid` RATHER THAN `CommandExt::pre_exec`.** `unsafe_code = "forbid"`
// (`Cargo.toml`) covers this file, and `pre_exec` needs `unsafe`. `setsid` is
// util-linux and Linux-only; this whole file is already `#![cfg(unix)]`, and
// `run_detached` additionally requires `target_os = "linux"` for the same
// reason the module doc above does.
//
// **A REAL TLS SERVER, not a bypass in the shipped binary.** `require_https`
// (ADR-0569, ledger 717) has no exemption for a test double, on purpose, so
// the only way to prove `--password-stdin` actually reaches the gateway is a
// socket that speaks real TLS. `rcgen` mints a CA and a leaf it signs —
// **never a self-signed leaf presented as its own anchor**, which `webpki`
// commonly refuses as `UnknownIssuer` — and the CA's PEM travels to the
// client exactly the way a real deployment's does: through `login`'s stored
// `config.json` (`ca_for` only reuses a CA for the gateway it was issued
// for), and through `enrol`'s own token, which is allowed to carry one.

/// A CA certificate and a leaf it signs for `127.0.0.1`, both fresh per call.
///
/// **A SEPARATE CA, NOT A SELF-SIGNED LEAF USED AS ITS OWN ANCHOR.** The two
/// look interchangeable and are not: some verifiers refuse a leaf presented as
/// its own trust anchor as `UnknownIssuer`, and a real deployment's
/// certificate is never self-signed either — matching that shape is what
/// makes this test prove something about the real path rather than about a
/// shortcut only the test takes.
fn generate_ca_and_leaf() -> (
    String,
    rustls::pki_types::CertificateDer<'static>,
    rustls::pki_types::PrivateKeyDer<'static>,
) {
    let mut ca_params =
        rcgen::CertificateParams::new(Vec::<String>::new()).expect("empty SAN list");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_key = rcgen::KeyPair::generate().expect("a CA key pair");
    let ca_cert = ca_params.self_signed(&ca_key).expect("a self-signed CA");

    let leaf_key = rcgen::KeyPair::generate().expect("a leaf key pair");
    // `127.0.0.1` AS THE SUBJECT ALTERNATIVE NAME, not `localhost`: every
    // gateway address in these tests is the loopback address by IP, because
    // that is the only address a one-shot `TcpListener::bind("127.0.0.1:0")`
    // actually answers on.
    let leaf_params =
        rcgen::CertificateParams::new(vec!["127.0.0.1".to_string()]).expect("one SAN");
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &ca_cert, &ca_key)
        .expect("a leaf signed by the CA");

    let ca_pem = ca_cert.pem();
    let leaf_der = leaf_cert.der().clone();
    let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(leaf_key.serialize_der()),
    );
    (ca_pem, leaf_der, key_der)
}

/// Bind an ephemeral loopback port, and answer the first TLS connection with
/// *body* once the whole request head (and any declared body) has arrived.
///
/// Returns the port to point the binary under test at, and a handle yielding
/// the request as it arrived — same contract as `src/testserver.rs`'s
/// `answer_once`, which this cannot reuse: that module is `#[cfg(test)]`
/// inside a binary-only crate with no `[lib]` target, so nothing under
/// `tests/` can see it.
///
/// Takes an already-minted leaf and key rather than minting its own, because
/// every caller here also needs the CA's PEM handed to the client side —
/// `login`'s `config.json`, `enrol`'s own token — so [`generate_ca_and_leaf`]
/// is always called at the test, not inside this function.
fn serve_one_tls_with(
    leaf_der: rustls::pki_types::CertificateDer<'static>,
    key_der: rustls::pki_types::PrivateKeyDer<'static>,
    body: &'static str,
) -> (u16, std::thread::JoinHandle<String>) {
    use std::io::{Read as _, Write as _};

    // `aws_lc_rs`, MATCHING `reqwest`'S OWN BACKEND. A crypto provider is a
    // process-global install; the binary under test runs in a SEPARATE
    // process, so there is no runtime collision either way, but there is no
    // reason to pull in `ring` as well when nothing here needs it.
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let server_config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("TLS 1.2 and 1.3 are both known to aws_lc_rs")
        .with_no_client_auth()
        .with_single_cert(vec![leaf_der], key_der)
        .expect("the leaf and its key match");
    let server_config = std::sync::Arc::new(server_config);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("the bound address").port();

    let served = std::thread::spawn(move || {
        let (sock, _) = listener.accept().expect("one connection");
        let conn = rustls::ServerConnection::new(server_config).expect("a TLS server connection");
        let mut tls = rustls::StreamOwned::new(conn, sock);

        // READ UNTIL THE HEAD IS COMPLETE — same reason `testserver.rs` does:
        // a single `read` returns whatever one TLS record happened to carry.
        let mut head = String::new();
        let mut buf = [0u8; 4096];
        while !head.contains("\r\n\r\n") {
            match tls.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => head.push_str(&String::from_utf8_lossy(&buf[..n])),
            }
        }
        let wanted = head
            .find("\r\n\r\n")
            .zip(content_length(&head))
            .map(|(blank, len)| blank + 4 + len);
        while wanted.is_some_and(|w| head.len() < w) {
            match tls.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => head.push_str(&String::from_utf8_lossy(&buf[..n])),
            }
        }

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len(),
        );
        let _ = tls.write_all(response.as_bytes());
        let _ = tls.flush();
        head
    });

    (port, served)
}

/// The declared body length, or `None` when the request declared none. Same
/// rule as `src/testserver.rs`'s copy, needed here for the reason above.
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

/// A protobuf varint (LEB128, unsigned) — the one encoding `iam.v1
/// .EnrolmentToken` uses for a tag or a length.
fn varint(mut n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out
}

/// One length-delimited (wire type 2) field: a tag, a length, then the bytes.
/// Every field `iam.v1.EnrolmentToken` carries — `secret`, `gateway`,
/// `ca_pem` — is this shape; see `src/enrolment.rs` for the read side.
fn protobuf_bytes_field(number: u32, data: &[u8]) -> Vec<u8> {
    let key = (u64::from(number) << 3) | 2;
    let mut out = varint(key);
    out.extend(varint(data.len() as u64));
    out.extend_from_slice(data);
    out
}

/// Mint an enrolment blob good enough to pass `src/enrolment.rs::decode` —
/// `secret`, `gateway`, and `ca_pem` — with no `expires_at` field at all,
/// which `expired(None, _)` reads as never expiring.
fn enrolment_token(secret: &str, gateway: &str, ca_pem: &str) -> String {
    let mut wire = Vec::new();
    wire.extend(protobuf_bytes_field(1, secret.as_bytes()));
    wire.extend(protobuf_bytes_field(2, gateway.as_bytes()));
    wire.extend(protobuf_bytes_field(3, ca_pem.as_bytes()));
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(wire)
}

/// Run the built binary with **no controlling terminal at all** — not merely
/// piped stdin — and a bounded wait, so a regression that reintroduces a real
/// prompt fails this test rather than stalling CI.
///
/// See the module doc above for why `setsid` and not `pre_exec`, and why
/// piped stdin alone would not exercise anything.
#[cfg(target_os = "linux")]
fn run_detached(
    home: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
    stdin_data: &str,
) -> (std::process::ExitStatus, String, String) {
    use std::io::{Read as _, Write as _};

    let mut cmd = Command::new("setsid");
    // `-w`/`--wait`: without it, `setsid` FORKS the target and exits
    // immediately once the fork succeeds (the underlying `setsid()` syscall
    // refuses a process that is already a session leader, which is why the
    // tool forks in the first place). That would make our immediate child
    // the wrapper rather than the binary under test — its exit status is the
    // fork's, not the real one, and reading `stdout`/`stderr` after IT exits
    // races the detached grandchild that is still writing to the same pipes.
    // `-w` keeps `setsid` attached until the real process exits and adopts
    // its exit code, so `child.wait()` below reflects the binary under test.
    cmd.arg("-w")
        .arg(env!("CARGO_BIN_EXE_yaadgaar"))
        .args(args)
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    let mut child = cmd.spawn().expect("`setsid` is not on PATH");

    // WRITTEN THEN DROPPED, not left open: dropping the handle closes the
    // write end, so a process that reads past the lines supplied here sees
    // the pipe close rather than blocking on more input that will never come.
    {
        let mut stdin = child.stdin.take().expect("stdin was piped");
        let _ = stdin.write_all(stdin_data.as_bytes());
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("polling the child") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "the binary did not exit within 10s with no controlling terminal — \
                 it is prompting instead of failing fast or refusing outright, \
                 or a real network call hung"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };

    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .expect("stdout was piped")
        .read_to_string(&mut stdout)
        .expect("stdout was UTF-8");
    child
        .stderr
        .take()
        .expect("stderr was piped")
        .read_to_string(&mut stderr)
        .expect("stderr was UTF-8");

    (status, stdout, stderr)
}

#[test]
#[cfg(target_os = "linux")]
fn the_password_prompt_fails_fast_with_no_terminal_rather_than_hanging_forever() {
    // THE RED CASE THIS PR CLOSES. Before `--password-stdin` existed, this was
    // the ONLY way `login` could run: `rpassword::prompt_password` opens
    // `/dev/tty` directly (rpassword 7.5.4, `src/unix.rs`), bypassing stdin
    // entirely, so a process with no controlling terminal cannot answer this
    // prompt no matter what is piped to it. Measured, not assumed: this is
    // the exact command line that used to be the only option, now demonstrated
    // to fail fast (`ENXIO`) rather than hang — the property `run_detached`'s
    // own timeout exists to enforce on every test in this file.
    let home = scratch("login-no-tty-old-path");
    let (status, stdout, stderr) = run_detached(
        &home,
        &["login"],
        &[],
        "https://gw.sentinel.invalid/\nsomeone\n",
    );

    assert!(
        !status.success(),
        "a login with no terminal at all somehow succeeded: stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("Gateway address") && stdout.contains("Username:"),
        "the two prompts `--gateway`/`--username` exist to remove did not even run: {stdout:?}"
    );
    assert!(
        stderr.contains("cannot read input"),
        "expected the password prompt to fail opening /dev/tty, got: {stderr:?}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(target_os = "linux")]
fn login_with_password_stdin_reaches_a_real_gateway_with_no_terminal_and_prompts_nothing() {
    // THE GREEN CASE. `--gateway` and `--username` remove the two prompts
    // stdin would otherwise answer, and `--password-stdin` removes the third
    // — so this runs start to finish with no controlling terminal at all, and
    // succeeding IS the proof nothing tried to prompt: `rpassword` opening
    // `/dev/tty` under `setsid` is not slow, it is refused outright, so any
    // regression that reintroduced a prompt would turn this from a pass into
    // the exact failure the RED case above demonstrates.
    let home = scratch("login-password-stdin-e2e");
    let config_dir = home.join("yadgar-config");
    std::fs::create_dir_all(&config_dir).expect("the config directory");

    let (ca_pem, leaf_der, key_der) = generate_ca_and_leaf();
    let (port, served) = serve_one_tls_with(leaf_der, key_der, r#"{"token":"tok-login-e2e"}"#);
    let gateway = format!("https://127.0.0.1:{port}/");

    // SEEDED BY HAND, matching `Config`'s own JSON shape (`src/config.rs`):
    // `login` has no CA source of its own, and `ca_for` only reuses a stored
    // CA for the exact gateway it was issued for — so the trust anchor for
    // THIS gateway has to already be on file before `login` runs at all.
    let seeded = serde_json::json!({
        "gateway": gateway,
        "token": "old-token",
        "ca_pem": ca_pem,
    });
    std::fs::write(
        config_dir.join("config.json"),
        serde_json::to_string(&seeded).unwrap(),
    )
    .unwrap();

    let (status, stdout, stderr) = run_detached(
        &home,
        &[
            "login",
            "--gateway",
            &gateway,
            "--username",
            "sentinel-person",
            "--password-stdin",
        ],
        &[(
            "YADGAR_CONFIG_DIR",
            config_dir.to_str().expect("a UTF-8 path"),
        )],
        "hunter2\n",
    );

    assert!(status.success(), "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stdout.contains("logged in to"),
        "the success message did not print: {stdout:?}"
    );
    // NEITHER PROMPT'S TEXT APPEARS, because neither ran: `--gateway` and
    // `--username` were given, and `--password-stdin` replaced the third.
    assert!(!stdout.contains("Gateway address"), "{stdout:?}");
    assert!(!stdout.contains("Username:"), "{stdout:?}");

    let sent = served.join().expect("the server thread did not panic");
    assert!(sent.starts_with("POST /auth/login "), "{sent}");
    assert!(sent.contains(r#""username":"sentinel-person""#), "{sent}");
    assert!(sent.contains(r#""password":"hunter2""#), "{sent}");

    let written = std::fs::read_to_string(config_dir.join("config.json")).unwrap();
    assert!(
        written.contains("tok-login-e2e"),
        "the token the fake gateway issued was never written: {written}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(target_os = "linux")]
fn enrol_with_password_stdin_reaches_a_real_gateway_with_no_terminal_and_prompts_nothing() {
    // Same shape as the `login` case above, for `enrol`: the token supplies
    // the gateway and its CA on its own, so nothing needs seeding on disk —
    // only the password prompt (asked TWICE, ordinarily) needs replacing, and
    // `--password-stdin` replaces it with exactly one read.
    let home = scratch("enrol-password-stdin-e2e");
    let config_dir = home.join("yadgar-config");
    std::fs::create_dir_all(&config_dir).expect("the config directory");

    let (ca_pem, leaf_der, key_der) = generate_ca_and_leaf();
    let (port, served) = serve_one_tls_with(
        leaf_der,
        key_der,
        r#"{"token":"tok-enrol-e2e","username":"sentinel-person"}"#,
    );
    let gateway = format!("https://127.0.0.1:{port}/");
    let token = enrolment_token("a-secret-nobody-hardcoded", &gateway, &ca_pem);

    let (status, stdout, stderr) = run_detached(
        &home,
        &[
            "enrol",
            &token,
            // Deliberately no `--token-file`: the positional argument is the
            // token source under test here.
            "--password-stdin",
        ],
        &[(
            "YADGAR_CONFIG_DIR",
            config_dir.to_str().expect("a UTF-8 path"),
        )],
        "a-new-password\n",
    );

    assert!(status.success(), "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stdout.contains("enrolled with") && stdout.contains("sentinel-person"),
        "the success message did not print: {stdout:?}"
    );
    // THE REPEAT PROMPT NEVER RAN: there is exactly one password on stdin,
    // and a binary that still asked for a second would have failed with
    // `ENXIO` opening `/dev/tty` instead of succeeding.
    assert!(!stdout.contains("Choose a password"), "{stdout:?}");
    assert!(!stdout.contains("Repeat it"), "{stdout:?}");

    let sent = served.join().expect("the server thread did not panic");
    assert!(sent.starts_with("POST /auth/enrol "), "{sent}");
    assert!(
        sent.contains(r#""secret":"a-secret-nobody-hardcoded""#),
        "{sent}"
    );
    assert!(sent.contains(r#""password":"a-new-password""#), "{sent}");
    assert!(
        !sent.contains(&token),
        "the whole enrolment blob was sent instead of its secret field: {sent}"
    );

    let written = std::fs::read_to_string(config_dir.join("config.json")).unwrap();
    assert!(
        written.contains("tok-enrol-e2e"),
        "the token the fake gateway issued was never written: {written}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

// ============================================================================
// Argument-shape guards (review follow-up) — each of these pins one clap
// declaration or one runtime check by driving the binary through the exact
// mistake that declaration exists to catch, so deleting the declaration turns
// the test red rather than leaving it silently unexercised.

#[test]
fn a_missing_token_file_is_reported_with_its_own_path() {
    // `std::fs::read_to_string(&path)?` alone reports only the OS error
    // ("No such file or directory") with nothing naming which of possibly
    // several paths in play was the one that failed to open.
    let home = scratch("enrol-token-file-missing");
    let missing = home.join("no-such-token-file");
    let output = Command::new(env!("CARGO_BIN_EXE_yaadgaar"))
        .args(["enrol", "--token-file"])
        .arg(&missing)
        .env("HOME", &home)
        .env("YADGAR_CONFIG_DIR", home.join("config"))
        .output()
        .expect("the binary under test did not run");

    assert!(!output.status.success(), "a missing token file enrolled");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains(&missing.display().to_string()),
        "the read failure did not name the path that failed: {said}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn enrol_refuses_a_token_argument_together_with_token_file() {
    // Two sources for the same blob is a contradiction for the CALLER to
    // resolve, not a precedence for this binary to pick silently — pinned as
    // a clap usage error (`conflicts_with`), not a runtime one.
    let home = scratch("enrol-token-and-token-file-conflict");
    let output = Command::new(env!("CARGO_BIN_EXE_yaadgaar"))
        .args([
            "enrol",
            "this-is-not-a-token",
            "--token-file",
            "/does/not/matter",
        ])
        .env("HOME", &home)
        .env("YADGAR_CONFIG_DIR", home.join("config"))
        .output()
        .expect("the binary under test did not run");

    assert!(
        !output.status.success(),
        "both a token argument and --token-file were accepted together"
    );
    let said = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(
        said.contains("cannot be used with") || said.contains("conflict"),
        "the refusal did not read as a clap usage error: {said}"
    );
    assert!(
        !home.join("config").join("config.json").exists(),
        "a refused enrolment wrote a config"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn login_password_stdin_without_gateway_and_username_is_a_usage_error() {
    // GUARDS `requires_all = ["gateway", "username"]` on `Login`'s
    // `password_stdin` field. Without it, this would fall through to the
    // interactive prompts for gateway and username, which would then read
    // stdin themselves — consuming the very lines meant for the password —
    // rather than failing before either prompt runs at all.
    let home = scratch("login-password-stdin-needs-gateway-and-username");
    let output = Command::new(env!("CARGO_BIN_EXE_yaadgaar"))
        .args(["login", "--password-stdin"])
        .env("HOME", &home)
        .output()
        .expect("the binary under test did not run");

    assert!(
        !output.status.success(),
        "--password-stdin ran with neither --gateway nor --username"
    );
    // Exit status 2 is clap's own usage-error code — the process never
    // reached `login::login` at all, so no prompt text can have printed.
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let said = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(
        said.contains("required") || said.contains("requires"),
        "the refusal did not read as a clap usage error: {said}"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).is_empty(),
        "a prompt printed before the usage error was reported"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(target_os = "linux")]
fn password_stdin_is_refused_on_a_real_pty_not_only_a_process_with_no_terminal_at_all() {
    // THE OTHER HALF of the red/green pair earlier in this file. Every test
    // above removes the controlling terminal entirely (`setsid`), which
    // proves `--password-stdin` does not HANG without one — it says nothing
    // about whether `ensure_stdin_pipeable` would notice a terminal that IS
    // there. `script` allocates a real pseudo-terminal and connects the
    // child's stdin (and stdout, and stderr — one pty, one fd on the far
    // side) to it, which is the one condition `IsTerminal::is_terminal`
    // actually inspects.
    //
    // `script` COPIES THE PTY SESSION TO ITS OWN STDOUT, measured: with the
    // child's stdout and stderr both landing on the pty side, there is only
    // one stream for `script` to forward, and it forwards it as stdout, not
    // stderr, on this system's util-linux `script`.
    let home = scratch("password-stdin-real-pty");
    let bin = env!("CARGO_BIN_EXE_yaadgaar");
    let inner = format!(
        "{bin} login --gateway https://pty.sentinel.invalid/ --username someone --password-stdin"
    );
    let output = Command::new("script")
        .args(["-qec", &inner, "/dev/null"])
        .env("HOME", &home)
        .output()
        .expect("`script` is not on PATH");

    assert!(
        !output.status.success(),
        "a real terminal was accepted by --password-stdin: {output:?}"
    );
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        said.contains("not a terminal"),
        "the terminal refusal did not print: {said:?}"
    );
    let _ = std::fs::remove_dir_all(&home);
}
