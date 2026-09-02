//! What this client trusts for the gateway connection, and for nothing else.
//!
//! **THE CA TRAVELS IN THE ENROLMENT TOKEN, and this is what makes that worth
//! doing.** A person on a fresh machine has never met the deployment, so they
//! have nothing to verify the gateway against, and every other answer is wrong
//! in a specific way: fetching the CA over the connection it is meant to
//! authenticate is circular, installing it in the system trust store risks the
//! whole machine to solve a problem scoped to one host, and shipping it in the
//! binary makes it public and unrotatable. The out-of-band channel that carried
//! the token is already trusted, so the anchor travels on it.
//!
//! **ADDED, NOT SUBSTITUTED.** `add_root_certificate` extends the trust set for
//! THIS client rather than replacing it, so a deployment whose gateway has a
//! publicly-trusted certificate still works when a token happens to carry a CA
//! as well. Nothing here touches the system trust store, and nothing here is
//! reachable by any connection this client does not make itself.
//!
//! `None` is the ordinary case and not a degraded one: an absent `ca_pem` means
//! the deployment uses a publicly-trusted certificate and system trust applies.

use std::time::Duration;

/// Why a CA could not be trusted.
///
/// TWO ARMS, because the two failures are different things a person can act on.
/// One says the bytes are damaged; the other says the bytes are fine and carry
/// no certificate — which is the case that used to succeed.
#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    #[error("the enrolment token's `ca_pem` is not a readable certificate: {0}")]
    Unreadable(#[from] reqwest::Error),
    /// **A DISTINCT ARM, and it is the whole point of this module's fix.**
    ///
    /// A PEM with no `CERTIFICATE` section is not a broken certificate; it is a
    /// file that contains none. Reported separately so the message can say so,
    /// rather than blaming an encoding that is perfectly fine.
    #[error(
        "the enrolment token's `ca_pem` contains no certificate: it carries no PEM \
         CERTIFICATE section, so it would add no trust anchor and the gateway would \
         be verified against system trust alone. Ask for a new token."
    )]
    NoCertificate,
}

/// Build a client that trusts *ca_pem* in addition to the system roots.
///
/// The PEM is REFUSED HERE rather than at connect time. A certificate that
/// rustls cannot parse otherwise surfaces as a TLS handshake failure against a
/// hostname the person has never seen, on their first contact with the
/// deployment — which is the exact failure the token exists to prevent.
///
/// **`from_pem_bundle` RATHER THAN `from_pem`, AND AN EMPTY RESULT IS REFUSED.
/// That is a real defect closed, not a tidy-up.** Under rustls
/// `Certificate::from_pem` parses NOTHING — it stores the bytes, and the parse
/// happens later inside `build()`, which calls `read_pem_certs`. That returns an
/// EMPTY VECTOR for input containing no PEM section, and an empty vector is not
/// an error. Measured against reqwest 0.13.4 with this crate's exact features,
/// each of `" "`, `"\n"`, `"no certificate here"`, `"# comment\n"`, `"null"` and
/// a `BEGIN PRIVATE KEY` block returned `Ok` **having added no trust anchor at
/// all**. It refused only when a `BEGIN CERTIFICATE` line was present with an
/// unparseable body — which is exactly why the first version of the test below
/// passed while the paragraph above it was false.
///
/// A token that SAYS "trust this CA" would therefore have produced a client
/// trusting only the system store: on a private deployment, the undiagnosable
/// handshake failure the eager refusal exists to prevent, arriving anyway and
/// now with a comment promising it could not.
///
/// A BUNDLE IS STILL ACCEPTED WHOLE. `from_pem_bundle` collects every
/// certificate in the input, so a root-plus-intermediate PEM adds both — and so
/// did `from_pem`, whose singular name is misleading rather than limiting.
/// Nothing about that behaviour changes here; only the empty case does.
pub fn client(
    ca_pem: Option<&str>,
    timeout: Option<Duration>,
) -> Result<reqwest::Client, TrustError> {
    let mut builder = reqwest::Client::builder();
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(pem) = ca_pem {
        let anchors = reqwest::Certificate::from_pem_bundle(pem.as_bytes())?;
        // THE COUNT IS THE CHECK. Nothing before this point rejects a PEM that
        // simply has no certificate in it: `from_pem_bundle` returns an empty
        // list, and `from_der` under rustls stores bytes without validating
        // them either — so neither an `Ok` nor a non-empty parse is evidence on
        // its own. What can be relied on is that a real anchor produces at
        // least one entry.
        if anchors.is_empty() {
            return Err(TrustError::NoCertificate);
        }
        for anchor in anchors {
            builder = builder.add_root_certificate(anchor);
        }
    }
    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_ca_is_the_ordinary_case_and_not_an_error() {
        // The deployment this client meets today mints tokens with no `ca_pem`
        // at all. A builder that refused `None` would refuse every real token.
        assert!(client(None, None).is_ok());
    }

    #[test]
    fn a_pem_that_is_not_a_certificate_is_refused_here_rather_than_at_connect_time() {
        // Otherwise it surfaces as a handshake failure against a hostname the
        // person has never seen, on their first contact with the deployment.
        //
        // THIS TEST USED TO BE THE ONLY ONE, AND IT PASSED FOR THE WRONG
        // REASON: its fixture happens to carry a `BEGIN CERTIFICATE` line,
        // which is the single case the old `from_pem` path refused. Every
        // input below reached `Ok` with no anchor added. Keep it, and keep the
        // one under it.
        assert!(client(Some("-----BEGIN CERTIFICATE-----\nnot-a-cert\n"), None).is_err());
    }

    #[test]
    fn a_ca_that_contains_no_certificate_at_all_is_refused_rather_than_silently_ignored() {
        // THE DEFECT. `Certificate::from_pem` stores bytes without parsing
        // them; the parse happens in `build()`, and input with no PEM section
        // yields an EMPTY certificate list, which is not an error. So each of
        // these produced a client that trusted only the system store while the
        // token asserted a CA — on a private deployment, a handshake failure
        // against a hostname the person has never seen, which is precisely what
        // shipping the anchor in the token exists to prevent.
        //
        // Measured against reqwest 0.13.4 with this crate's exact features:
        // every string below returned `Ok` before `from_pem_bundle`.
        for empty in [
            " ",
            "\n",
            "\t\n\n",
            "no certificate here",
            "# comment\n",
            "null",
            "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n",
        ] {
            assert!(
                client(Some(empty), None).is_err(),
                "a ca_pem carrying no certificate was accepted: {empty:?}"
            );
        }
    }
}
