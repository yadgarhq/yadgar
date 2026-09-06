//! Classifying a URL's scheme — the one rule ledger 717 needs shared.
//!
//! `login.rs` (a person-typed or token-carried address, refused before it is
//! ever written) and `config.rs` (the same address, refused again at load
//! time so a config a pre-fix binary wrote cannot dial out in cleartext)
//! each build a DIFFERENT error around an IDENTICAL rule: only `https` is
//! accepted, and a bare host names no scheme at all. Duplicating the
//! classification itself — the split on `://`, the case-insensitive
//! comparison — is exactly the failure ledger 590 spent a day undoing
//! elsewhere in the estate: a copy nobody remembers to find when the rule
//! next changes, whether that is accepting another scheme, handling a
//! schemeless edge case, or deciding case-folding is not enough. The rule
//! lives HERE, once; each caller still builds the error that fits where it
//! is said.

/// What an address's scheme is, for the one purpose this crate cares about.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Scheme<'a> {
    /// Accepted. Compared case-insensitively, so `HTTPS://` counts too.
    Https,
    /// Some other scheme, carried so the caller can name it in its error.
    Other(&'a str),
    /// No `://` at all — a bare host such as `gw.example.com`.
    Absent,
}

/// Classify the scheme of an address, ideally one already run through
/// `login::normalise` so it sits at a fixed offset.
pub(crate) fn scheme_of(url: &str) -> Scheme<'_> {
    match url.split_once("://") {
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("https") => Scheme::Https,
        Some((scheme, _)) => Scheme::Other(scheme),
        None => Scheme::Absent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_is_https_regardless_of_case() {
        assert_eq!(scheme_of("https://gw.sentinel.invalid/"), Scheme::Https);
        assert_eq!(scheme_of("HTTPS://gw.sentinel.invalid/"), Scheme::Https);
    }

    #[test]
    fn every_other_scheme_is_named_rather_than_collapsed() {
        assert_eq!(
            scheme_of("http://gw.sentinel.invalid/"),
            Scheme::Other("http")
        );
    }

    #[test]
    fn a_bare_host_names_no_scheme() {
        assert_eq!(scheme_of("gw.sentinel.invalid/"), Scheme::Absent);
    }
}
