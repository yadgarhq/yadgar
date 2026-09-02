//! What a re-login KEEPS, and what it must not carry across deployments.
//!
//! Split out of `login/tests.rs` when the file passed its size ceiling. The
//! seam is `reconcile`: everything here is about assembling the config a new
//! credential produces — which fields survive it, and which must not follow a
//! person from one deployment to another — while the file beside it is about
//! what `login` and `enrol` put on the wire.

use super::super::*;

#[test]
fn a_second_login_keeps_the_install_id_the_first_one_minted() {
    // THE DEFECT THIS EXISTS FOR. `Config::new` starts from nothing and `save`
    // writes the whole struct, so building the new config out of the login
    // response alone dropped the UUID on every re-login, and `serve` then
    // minted a fresh one. ADR-0511 rejected a per-process instance because
    // "nothing could correlate a client across restarts, which rate-limit
    // accounting and audit both need" — an id a second `login` resets is that
    // rejection reintroduced through another door. A re-enrolment is not
    // hypothetical either: `iam.proto` describes an admin re-issuing a token as
    // a password reset.
    //
    // The id below is a value `uuid::new_v4()` cannot produce — its version
    // nibble is `0` — so a re-mint cannot satisfy this assertion.
    let dir = crate::testserver::scratch_dir("login-reconcile");
    let mut previous = Config::new(&dir, "https://old.gateway/".into(), "old-token".into());
    previous.set_instance(Some("77777777-7777-0777-7777-777777777777".into()));
    let previous = previous
        .with_username(Some("sentinel.person".into()))
        .with_ca_pem(Some("-----BEGIN CERTIFICATE-----\nsentinel\n".into()));

    // `login` HAS NO CA SOURCE, so it hands `reconcile` the one it read off the
    // machine — the same value `login` itself passes.
    let after = reconcile(
        &dir,
        Some(&previous),
        "https://new.gateway/".into(),
        "new-token".into(),
        "sentinel.person".into(),
        previous.ca_pem().map(str::to_string),
    );

    // The address and the credential are the NEW ones: carrying one half
    // forward must not stop the other half being replaced.
    assert_eq!(after.gateway_url(), "https://new.gateway/");
    assert_eq!(after.token(), "new-token");
    // And the install id is the one that already existed.
    assert_eq!(
        after.instance(),
        Some("77777777-7777-0777-7777-777777777777")
    );
    assert_eq!(
        after.ca_pem(),
        Some("-----BEGIN CERTIFICATE-----\nsentinel\n")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn enrolling_into_another_deployment_does_not_inherit_the_previous_one_s_ca() {
    // THE TRUST BUG THIS SPLIT EXISTS FOR, and it is why one `reconcile` cannot
    // hold both answers behind one `Option`.
    //
    // On the `login` path above, `None` means "I was not told" — `login` has no
    // CA source, so the machine's existing anchor is the only candidate. On the
    // `enrol` path `None` is a POSITIVE STATEMENT the contract defines: this
    // deployment uses a publicly-trusted certificate, use system trust. A
    // fallback there writes private deployment A's CA into a config pointing at
    // deployment B, and `proxy::serve` then trusts A for every connection to B
    // — so A can mint a certificate for B's hostname and this client takes it.
    //
    // The old assertion passed `ca_pem: None` and asserted the previous CA
    // SURVIVED. That is correct for login and is this bug for enrol, which is
    // why the two are now separate tests rather than one with another case.
    let dir = crate::testserver::scratch_dir("enrol-foreign-ca");
    let mut previous = Config::new(&dir, "https://deployment-a/".into(), "a-token".into());
    previous.set_instance(Some("77777777-7777-0777-7777-777777777777".into()));
    let previous = previous.with_ca_pem(Some(
        "-----BEGIN CERTIFICATE-----\ndeployment-a-anchor\n-----END CERTIFICATE-----\n".into(),
    ));

    // What `enrol` passes for a token that carries no `ca_pem`: system trust.
    let after = reconcile(
        &dir,
        Some(&previous),
        "https://deployment-b/".into(),
        "b-token".into(),
        "sentinel.person".into(),
        None,
    );

    assert_eq!(
        after.ca_pem(),
        None,
        "deployment A's CA was written into a config pointing at deployment B"
    );
    // The install id STILL survives — the two rules are independent, and fixing
    // the CA must not undo the other.
    assert_eq!(
        after.instance(),
        Some("77777777-7777-0777-7777-777777777777")
    );
}

#[test]
fn login_reuses_a_stored_ca_only_for_the_deployment_it_came_from() {
    // The enrol fix arriving through the one caller that legitimately HAS a
    // fallback. `login` has no CA source, so reusing the machine's anchor is
    // right for the deployment already stored — and pointing `login` at a
    // different address and reusing it anyway hands private deployment A's
    // issuer authority over connections to B, which is the same trust transfer
    // by another route.
    let dir = crate::testserver::scratch_dir("login-ca-scope");
    let previous = Config::new(&dir, "https://deployment-a/".into(), "tok".into()).with_ca_pem(
        Some("-----BEGIN CERTIFICATE-----\ndeployment-a\n-----END CERTIFICATE-----\n".into()),
    );

    assert_eq!(
        ca_for(Some(&previous), "https://deployment-a/").as_deref(),
        Some("-----BEGIN CERTIFICATE-----\ndeployment-a\n-----END CERTIFICATE-----\n"),
        "a re-login against the SAME gateway must not need the CA installed by hand again"
    );
    assert_eq!(
        ca_for(Some(&previous), "https://deployment-b/"),
        None,
        "deployment A's CA was offered for a login to deployment B"
    );
    assert_eq!(ca_for(None, "https://deployment-a/"), None);

    // A HAND-EDITED ADDRESS IS STILL THE SAME DEPLOYMENT. `config.json` is a
    // file a person can edit, and one whose address lost its trailing slash
    // would otherwise compare unequal to itself — dropping the CA sitting right
    // there and failing the login with a handshake error.
    let unslashed = Config::new(&dir, "https://deployment-a".into(), "tok".into()).with_ca_pem(
        Some("-----BEGIN CERTIFICATE-----\ndeployment-a\n-----END CERTIFICATE-----\n".into()),
    );
    assert!(
        ca_for(Some(&unslashed), "https://deployment-a/").is_some(),
        "a trailing slash made a deployment look like a different one"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn changing_deployment_drops_the_tool_list_cached_from_the_old_one() {
    // Same blast radius as the CA, one file along. The cache is served on an
    // offline start (D75), so deployment A's `tools/list` kept across a change
    // of gateway is presented to the agent as B's.
    let dir = crate::testserver::scratch_dir("login-cache-change");
    let previous = Config::new(&dir, "https://deployment-a/".into(), "a-token".into());
    previous.save().unwrap();
    previous
        .write_tool_cache(r#"{"result":{"tools":[{"name":"a-only"}]}}"#)
        .unwrap();
    assert!(previous.read_tool_cache().is_some(), "the fixture is wrong");

    let after = reconcile(
        &dir,
        Some(&previous),
        "https://deployment-b/".into(),
        "b-token".into(),
        "someone".into(),
        None,
    );
    assert_eq!(
        after.read_tool_cache(),
        None,
        "deployment A's tool list survived a move to deployment B"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn logging_in_again_to_the_same_deployment_keeps_the_cache() {
    // The other half, and it needs saying: a re-login against the SAME gateway
    // must not throw the cache away, or an offline start right after one looks
    // exactly like yadgar never being installed (D75).
    let dir = crate::testserver::scratch_dir("login-cache-same");
    let previous = Config::new(&dir, "https://deployment-a/".into(), "old-token".into());
    previous.save().unwrap();
    previous
        .write_tool_cache(r#"{"result":{"tools":[{"name":"recall"}]}}"#)
        .unwrap();

    let after = reconcile(
        &dir,
        Some(&previous),
        "https://deployment-a/".into(),
        "new-token".into(),
        "someone".into(),
        None,
    );
    assert!(
        after.read_tool_cache().is_some(),
        "a re-login against the same gateway threw away a valid cache"
    );

    // And a stored address that lost its trailing slash is still the same
    // deployment, for the reason `ca_for` states.
    let unslashed = Config::new(&dir, "https://deployment-a".into(), "old".into());
    let after = reconcile(
        &dir,
        Some(&unslashed),
        "https://deployment-a/".into(),
        "new-token".into(),
        "someone".into(),
        None,
    );
    assert!(
        after.read_tool_cache().is_some(),
        "a trailing slash made a deployment look like a different one"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_first_login_on_a_machine_with_no_config_carries_nothing() {
    // The other direction, and it needs saying separately: `reconcile` must not
    // invent a previous state where there is none, or a fresh machine acquires
    // an id from nowhere.
    let dir = crate::testserver::scratch_dir("login-reconcile-fresh");
    let fresh = reconcile(
        &dir,
        None,
        "https://gw/".into(),
        "tok".into(),
        "someone".into(),
        None,
    );
    assert_eq!(fresh.instance(), None);
    assert_eq!(fresh.ca_pem(), None);
    assert_eq!(fresh.username(), Some("someone"));
    std::fs::remove_dir_all(&dir).ok();
}
