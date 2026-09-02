//! FIXED BYTES IN, EXPECTED VALUES OUT.
//!
//! Every constant below was encoded OUTSIDE this crate, by a separate protobuf
//! writer, and pasted here as a literal. Nothing in this file shares code with
//! the decoder, so no round trip can agree with itself: a decoder that dropped
//! `ca_pem`, swapped two field numbers or invented a gateway fails here.
//!
//! The values are chosen so no hardcode could produce them. The gateway is not
//! `https://gateway.yadgar.internal:18443` — that is the address the login
//! prompt already names, and a fixture declaring it could not tell a decoded
//! token from a client that simply kept its own default.

use super::*;

/// secret, gateway, ca_pem and expires_at, all four present.
const FULL: &str = "CjRjMlZ1ZEdsdVpXd3RjMlZqY21WMExXNXZkQzFoTFdoaGNtUmpiMlJsTFdGaFlXRmhZUT09EiBodHRwczovL2d3LnNlbnRpbmVsLmludmFsaWQ6MTk5ORpGLS0tLS1CRUdJTiBDRVJUSUZJQ0FURS0tLS0tCnNlbnRpbmVsLWFuY2hvcgotLS0tLUVORCBDRVJUSUZJQ0FURS0tLS0tCiIGCICgsLQD";

/// `ca_pem` ABSENT — the deployment this client was actually built against.
const WITHOUT_CA: &str = "CjRjMlZ1ZEdsdVpXd3RjMlZqY21WMExXNXZkQzFoTFdoaGNtUmpiMlJsTFdGaFlXRmhZUT09EiBodHRwczovL2d3LnNlbnRpbmVsLmludmFsaWQ6MTk5OSIGCICgsLQD";

/// `ca_pem` PRESENT AND EMPTY — the token the contract says to refuse.
const EMPTY_CA: &str = "CjRjMlZ1ZEdsdVpXd3RjMlZqY21WMExXNXZkQzFoTFdoaGNtUmpiMlJsTFdGaFlXRmhZUT09EiBodHRwczovL2d3LnNlbnRpbmVsLmludmFsaWQ6MTk5ORoAIgYIgKCwtAM=";

/// `gateway` present and empty — a misconfigured `iam` minting a token that
/// points at nothing.
const EMPTY_GATEWAY: &str =
    "CjRjMlZ1ZEdsdVpXd3RjMlZqY21WMExXNXZkQzFoTFdoaGNtUmpiMlJsTFdGaFlXRmhZUT09EgAiBgiAoLC0Aw==";

/// `secret` absent altogether.
const NO_SECRET: &str = "EiBodHRwczovL2d3LnNlbnRpbmVsLmludmFsaWQ6MTk5OSIGCICgsLQD";

const SENTINEL_SECRET: &str = "c2VudGluZWwtc2VjcmV0LW5vdC1hLWhhcmRjb2RlLWFhYWFhYQ==";
const SENTINEL_GATEWAY: &str = "https://gw.sentinel.invalid:1999";
const SENTINEL_CA: &str =
    "-----BEGIN CERTIFICATE-----\nsentinel-anchor\n-----END CERTIFICATE-----\n";

#[test]
fn every_field_comes_out_of_the_blob_and_not_out_of_this_client() {
    // The gateway address is the field that earns the whole design: the person
    // enrolling has never met this deployment, so a client that prompted for an
    // address would be asking for something the token already knows, and asking
    // a stranger for a hostname they cannot check is how a typo becomes a TLS
    // error nobody can diagnose.
    let e = decode(FULL).expect("a well-formed token");
    assert_eq!(e.secret, SENTINEL_SECRET);
    assert_eq!(e.gateway, SENTINEL_GATEWAY);
    assert_eq!(e.ca_pem.as_deref(), Some(SENTINEL_CA));
    // 1999-01-01T00:00:00Z. Not a plausible expiry for anything, which is the
    // point: a client substituting "now plus 24 hours" cannot produce it.
    assert_eq!(e.expires_at, Some(915_148_800));
}

#[test]
fn an_absent_ca_means_system_trust_rather_than_a_broken_token() {
    // THE DEPLOYMENT THIS CLIENT MEETS TODAY. `ca_pem` is absent on it, so a
    // decoder that treated absence as an error would refuse every real token
    // in existence — and the contract calls absence "a legitimate deployment
    // rather than a malformed token".
    let e = decode(WITHOUT_CA).expect("absence is legitimate");
    assert_eq!(e.ca_pem, None);
    assert_eq!(e.gateway, SENTINEL_GATEWAY);
    assert_eq!(e.secret, SENTINEL_SECRET);
}

#[test]
fn a_ca_that_is_present_and_empty_is_refused_by_name() {
    // ABSENT AND PRESENT-AND-EMPTY ARE DIFFERENT, and collapsing them is the
    // easy mistake: `Option<String>` decoded into `String` makes both `""`,
    // the test above still passes, and a token assembled wrong is accepted as
    // a deployment without a CA. The FIELD IS NAMED because the contract says
    // to name it — the admin cannot otherwise learn which half of their `iam`
    // configuration is empty.
    assert_eq!(decode(EMPTY_CA), Err(EnrolmentError::Empty("ca_pem")));
    assert!(decode(EMPTY_CA).unwrap_err().to_string().contains("ca_pem"));
}

#[test]
fn a_token_that_points_at_nothing_is_refused_before_it_is_used() {
    // Without this the misconfiguration surfaces on a stranger's machine, on
    // their first contact with the deployment, as an undiagnosable TLS error.
    assert_eq!(decode(EMPTY_GATEWAY), Err(EnrolmentError::Empty("gateway")));
    assert!(decode(EMPTY_GATEWAY)
        .unwrap_err()
        .to_string()
        .contains("gateway"));
}

#[test]
fn a_token_with_no_secret_is_refused_by_name_too() {
    // `RedeemEnrolment` is unauthenticated by construction, so the secret IS
    // the whole authenticator. An empty one presented to the endpoint would be
    // refused as a wrong secret, which sends the person back to the admin for a
    // new token instead of telling them the one they hold was minted broken.
    assert_eq!(decode(NO_SECRET), Err(EnrolmentError::Empty("secret")));
    assert!(decode(NO_SECRET)
        .unwrap_err()
        .to_string()
        .contains("secret"));
}

#[test]
fn a_blob_that_is_not_a_token_says_so_rather_than_decoding_to_noise() {
    // The two failures a person actually meets: a truncated paste, and a token
    // pasted from a chat client that mangled it. Neither may produce an
    // `Enrolment` with plausible-looking empty fields.
    assert_eq!(
        decode("this is not base64 at all!"),
        Err(EnrolmentError::NotBase64)
    );
    // Valid base64, bytes that are not a message: a length-delimited field
    // claiming more bytes than the blob holds.
    assert_eq!(decode("CgU="), Err(EnrolmentError::Malformed));
}

#[test]
fn a_field_a_newer_iam_added_is_skipped_rather_than_refused() {
    // Additive contract changes must not need a client release on every laptop.
    // Field 9, a varint, appended to the token above: it decodes exactly as
    // before, and nothing about the added field reaches the result.
    let with_future_field = {
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(WITHOUT_CA)
            .unwrap();
        bytes.extend_from_slice(&[0x48, 0x2a]); // field 9, varint, 42
        base64::engine::general_purpose::STANDARD.encode(bytes)
    };
    assert_eq!(
        decode(&with_future_field).expect("a newer token still enrols"),
        decode(WITHOUT_CA).unwrap()
    );
}

#[test]
fn a_ca_that_is_present_and_blank_is_refused_like_an_empty_one() {
    // `is_empty()` alone let `" "` and `"\n"` through, and reqwest then
    // accepted them and added NO trust anchor at all — so a token saying "trust
    // this CA" produced a client trusting only the system store. On a private
    // deployment that is the undiagnosable handshake failure the whole
    // CA-in-the-token design exists to prevent. `trust.rs` holds the other half
    // of this fix; both are needed, because a `ca_pem` that is not blank can
    // still contain no certificate.
    for blank in [" ", "\n", "\t \n"] {
        let mut b = base64::engine::general_purpose::STANDARD
            .decode(WITHOUT_CA)
            .unwrap();
        b.push(0x1a); // field 3, length-delimited
        b.push(blank.len() as u8);
        b.extend_from_slice(blank.as_bytes());
        let token = base64::engine::general_purpose::STANDARD.encode(b);
        assert_eq!(
            decode(&token),
            Err(EnrolmentError::Empty("ca_pem")),
            "a blank ca_pem was accepted: {blank:?}"
        );
    }
}

/// Append one raw field to a known-good token and re-encode it.
fn with_field(tag: u8, payload: &[u8]) -> String {
    let mut b = base64::engine::general_purpose::STANDARD
        .decode(WITHOUT_CA)
        .unwrap();
    b.push(tag);
    b.extend_from_slice(payload);
    base64::engine::general_purpose::STANDARD.encode(b)
}

#[test]
fn a_known_field_with_the_wrong_wire_type_is_refused_rather_than_read_as_absent() {
    // Keying the match on `(number, type)` TOGETHER looks equivalent to keying
    // on the number and then requiring the type. It is not: a known field
    // carrying the wrong type fell through to the skip-unknown arm, so a
    // `ca_pem` sent as a VARINT read as ABSENT — and absent means "use system
    // trust". A token asserting a CA would have produced a client trusting
    // none, with nothing said anywhere.
    //
    // Skip-unknown is correct for unknown field NUMBERS only. A real protobuf
    // runtime rejects a wire-type mismatch on a field it knows.
    assert_eq!(
        decode(&with_field(0x18, &[0x2a])),
        Err(EnrolmentError::WrongType("ca_pem"))
    );
    assert_eq!(
        decode(&with_field(0x19, &[0; 8])),
        Err(EnrolmentError::WrongType("ca_pem"))
    );
    assert_eq!(
        decode(&with_field(0x08, &[0x2a])),
        Err(EnrolmentError::WrongType("secret"))
    );
    assert_eq!(
        decode(&with_field(0x10, &[0x2a])),
        Err(EnrolmentError::WrongType("gateway"))
    );
    assert_eq!(
        decode(&with_field(0x20, &[0x2a])),
        Err(EnrolmentError::WrongType("expires_at"))
    );
    // An UNKNOWN number with any wire type is STILL skipped — that is the rule
    // this must not have broken on its way past.
    assert_eq!(decode(&with_field(0x48, &[0x2a])), decode(WITHOUT_CA));
}

#[test]
fn a_token_wrapped_by_a_mail_client_still_decodes() {
    // THE SECOND THING `ca_pem` MADE LIVE. A token used to be ~160 characters
    // and fitted on one line; with the live 2 kB CA it is ~2800, and every mail
    // client wraps at 76-78. Trimming only the ends turned an ordinary paste
    // into "that is not an enrolment token", which sends a person back to the
    // admin for a replacement that wraps in exactly the same way.
    let wrapped = WITHOUT_CA
        .as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(wrapped.contains('\n'), "the fixture did not wrap");
    assert_eq!(decode(&wrapped), decode(WITHOUT_CA));
    // CRLF and a leading indent, which is what a quoted mail body looks like.
    assert_eq!(decode(&wrapped.replace('\n', "\r\n  ")), decode(WITHOUT_CA));
}

#[test]
fn whitespace_around_a_pasted_token_does_not_make_it_unreadable() {
    // It arrives out of band — a chat message, an e-mail — and gets pasted with
    // a newline on the end. Refusing that teaches the person their token is
    // broken when it is not.
    assert_eq!(decode(&format!("  {WITHOUT_CA}\n")), decode(WITHOUT_CA));
}
