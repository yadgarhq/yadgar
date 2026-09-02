//! The enrolment token an admin hands over, decoded (D73).
//!
//! One base64 string, pasted once. Inside it is an `iam.v1.EnrolmentToken`
//! carrying the enrolment secret, the gateway's address and — on a private
//! deployment — the CA to trust for it. The person never assembles any of that
//! and never types the address, because `iam` filled both in from its own
//! configuration and a typed address is one that can be wrong.
//!
//! **THE ALPHABET IS RFC 4648 SECTION 4, STANDARD, WITH PADDING**, because the
//! contract says so. Standard versus URL-safe is a token that decodes to noise
//! on precisely the machines that have never met this deployment and can least
//! diagnose it.
//!
//! **NO PROTOBUF RUNTIME, and that is deliberate.** `Cargo.toml` says this
//! client carries no tonic, no prost and no generated code: it speaks MCP over
//! HTTPS and the gateway does the translation. One message with four scalar
//! fields does not overturn that — the wire format is four tag bytes and a
//! varint, read below in fifty lines that need no build script, no `.proto`
//! copy in this tree and no second source of truth to drift from. What this
//! module owes the contract is the SEMANTICS, and those are transcribed from
//! `yadgar/iam/v1/iam.proto` at tag v1.6.0 field by field:
//!
//! * `secret` (1) — never empty in a minted token; empty is refused by name.
//! * `gateway` (2) — never empty in a minted token; empty is refused by name.
//!   A misconfigured `iam` produces a structurally valid token pointing at
//!   nothing, and without this rule that surfaces on a stranger's machine, on
//!   their first contact with the deployment, as an undiagnosable TLS error.
//! * `ca_pem` (3) — OPTIONAL. **ABSENT means system trust**, which is a
//!   legitimate deployment rather than a malformed token. **PRESENT AND EMPTY
//!   IS NEITHER, and is REFUSED by name**: absence is the whole of "use system
//!   trust", so an empty string is a token that was assembled wrong.
//! * `expires_at` (4) — D73's 24 hours, read so a client can say the token
//!   expired instead of reporting a generic refusal.
//!
//! Unknown fields are SKIPPED rather than refused. A token minted by a newer
//! `iam` that added a field must still enrol the person holding it; refusing it
//! would make every additive contract change a client release.

use base64::Engine as _;

/// What the admin's blob says, once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enrolment {
    /// Presented to `POST /auth/enrol`. Never the whole blob — the contract is
    /// explicit that the endpoint takes the FIELD.
    pub secret: String,
    /// Where to send it. Nobody types this and nobody types it wrong.
    pub gateway: String,
    /// The root CA to trust for yadgar's connections and for nothing else on
    /// the machine. `None` is "use system trust", not "missing".
    pub ca_pem: Option<String>,
    /// Seconds since the epoch, or `None` when the token carries no expiry.
    pub expires_at: Option<i64>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EnrolmentError {
    #[error("that is not an enrolment token: it is not standard base64 (RFC 4648 §4)")]
    NotBase64,
    #[error("that is not an enrolment token: the bytes are not a well-formed EnrolmentToken")]
    Malformed,
    /// The field is NAMED, because the contract says to name it.
    ///
    /// One error per field rather than a shared "invalid token": an admin who
    /// minted a token against a misconfigured `iam` needs to be told which half
    /// of their configuration is empty, and the person holding the token cannot
    /// find that out any other way.
    #[error("that enrolment token is not usable: its `{0}` field is empty")]
    Empty(&'static str),
    /// A field the contract knows, carrying a type the contract does not give
    /// it.
    ///
    /// REFUSED RATHER THAN SKIPPED, and the difference is not pedantry: while
    /// unknown-field skipping also swallowed these, a `ca_pem` sent as a varint
    /// read as absent, and absent means "use system trust". A token asserting a
    /// CA would have produced a client trusting none, with nothing said.
    #[error("that enrolment token is malformed: its `{0}` field is not the type it must be")]
    WrongType(&'static str),
}

/// Decode the blob, or say why it is not a token.
pub fn decode(blob: &str) -> Result<Enrolment, EnrolmentError> {
    // ALL WHITESPACE IS STRIPPED, not merely trimmed from the ends.
    //
    // The token arrives out of band, and every mail client wraps a long line at
    // 76-78 characters. That never bit while a token was ~160 characters and
    // fitted on one line — but the live deployment now mints a 2 kB `ca_pem`,
    // which makes the blob ~2800 characters, so a wrapped paste is the ORDINARY
    // case rather than an edge one. Trimming the ends turned it into "that is
    // not an enrolment token", which sends a person back to the admin for a
    // replacement that will wrap in exactly the same way.
    //
    // Interior whitespace is never significant in base64 (RFC 4648 §3.3 leaves
    // it to the specification, and every mail-derived one ignores it), so
    // nothing is lost by removing it.
    let packed: String = blob.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&packed)
        .map_err(|_| EnrolmentError::NotBase64)?;

    let mut secret = None;
    let mut gateway = None;
    let mut ca_pem = None;
    let mut expires_at = None;

    // THE FIELD NUMBER IS MATCHED FIRST AND THE WIRE TYPE IS THEN REQUIRED.
    //
    // Keying on `(number, type)` together looks equivalent and is not: a known
    // field carrying the WRONG type fell through to the skip-unknown arm, so a
    // `ca_pem` sent as a varint read as ABSENT — which this client treats as
    // "use system trust". A token that says "trust this CA" would then have
    // silently produced a client that trusts none. Skipping is right for
    // unknown field NUMBERS only; a real protobuf runtime rejects a wire-type
    // mismatch on a field it knows, and so does this.
    for (field, value) in fields(&bytes)? {
        let bytes_of = |name| match value {
            Field::Bytes(b) => Ok(b),
            _ => Err(EnrolmentError::WrongType(name)),
        };
        match field {
            1 => secret = Some(text(bytes_of("secret")?)?),
            2 => gateway = Some(text(bytes_of("gateway")?)?),
            3 => ca_pem = Some(text(bytes_of("ca_pem")?)?),
            // `google.protobuf.Timestamp`: an embedded message whose field 1 is
            // the seconds. Nanoseconds are read past, because nothing here is
            // deciding anything at that resolution.
            4 => {
                expires_at = fields(bytes_of("expires_at")?)?
                    .into_iter()
                    .find_map(|(f, v)| match (f, v) {
                        (1, Field::Varint(n)) => Some(n as i64),
                        _ => None,
                    })
            }
            // A newer `iam` added a field. Skipping is what keeps an additive
            // contract change from needing a client release on every laptop.
            _ => {}
        }
    }

    // ABSENT AND EMPTY ARE THE SAME THING for these two — the contract says a
    // minted token never carries either — so one arm covers both.
    let secret = secret.filter(|s| !s.is_empty());
    let gateway = gateway.filter(|g| !g.is_empty());

    Ok(Enrolment {
        secret: secret.ok_or(EnrolmentError::Empty("secret"))?,
        gateway: gateway.ok_or(EnrolmentError::Empty("gateway"))?,
        // ABSENT AND EMPTY ARE DIFFERENT THINGS HERE, and that is the whole
        // rule. `None` means the deployment uses a publicly-trusted certificate
        // and system trust applies; `Some("")` is a token assembled wrong.
        //
        // **BLANK COUNTS AS EMPTY.** `is_empty()` alone let `" "` and `"\n"`
        // through, and reqwest then accepted them and added NO trust anchor at
        // all — so a token that says "trust this CA" produced a client trusting
        // only the system store, which on a private deployment is exactly the
        // undiagnosable handshake failure this rule exists to prevent. The
        // second half of that fix is in `trust.rs`; both halves are needed,
        // because a CA that is not blank can still contain no certificate.
        ca_pem: match ca_pem {
            Some(pem) if pem.trim().is_empty() => return Err(EnrolmentError::Empty("ca_pem")),
            other => other,
        },
        expires_at,
    })
}

/// One field as it appears on the wire.
#[derive(Debug, Clone, Copy)]
enum Field<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    /// A fixed-width value, wire type 1 or 5.
    ///
    /// **RECORDED RATHER THAN SKIPPED, and the value is deliberately dropped.**
    /// Nothing in this message is fixed-width, so the only fixed-width field
    /// that can arrive is either an unknown one — which is skipped by NUMBER,
    /// higher up — or a KNOWN field carrying the wrong type, which must be
    /// refused. While these were consumed without being recorded, a `ca_pem`
    /// sent as a fixed64 simply vanished from the parse and read as absent,
    /// which is exactly the silent "use system trust" the wire-type check
    /// exists to stop. What matters is that the field NUMBER is seen; what the
    /// eight bytes said is of no interest to anybody.
    Fixed,
}

/// Split protobuf bytes into `(field number, value)`, or say they are not that.
///
/// Truncation is an ERROR rather than a short read: a blob that ends mid-field
/// is a corrupted paste, and reporting the fields that happened to arrive first
/// would enrol somebody against half a token.
fn fields(bytes: &[u8]) -> Result<Vec<(u32, Field<'_>)>, EnrolmentError> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let (key, next) = varint(bytes, i)?;
        i = next;
        let field = u32::try_from(key >> 3).map_err(|_| EnrolmentError::Malformed)?;
        match key & 7 {
            0 => {
                let (value, next) = varint(bytes, i)?;
                i = next;
                out.push((field, Field::Varint(value)));
            }
            1 => {
                i = i
                    .checked_add(8)
                    .filter(|e| *e <= bytes.len())
                    .ok_or(EnrolmentError::Malformed)?;
                out.push((field, Field::Fixed));
            }
            2 => {
                let (len, next) = varint(bytes, i)?;
                let len = usize::try_from(len).map_err(|_| EnrolmentError::Malformed)?;
                let end = next.checked_add(len).ok_or(EnrolmentError::Malformed)?;
                let slice = bytes.get(next..end).ok_or(EnrolmentError::Malformed)?;
                i = end;
                out.push((field, Field::Bytes(slice)));
            }
            5 => {
                i = i
                    .checked_add(4)
                    .filter(|e| *e <= bytes.len())
                    .ok_or(EnrolmentError::Malformed)?;
                out.push((field, Field::Fixed));
            }
            // Groups (3 and 4) were removed from proto3 and nothing mints them.
            _ => return Err(EnrolmentError::Malformed),
        }
    }
    Ok(out)
}

fn varint(bytes: &[u8], mut i: usize) -> Result<(u64, usize), EnrolmentError> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(i).ok_or(EnrolmentError::Malformed)?;
        i += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, i));
        }
    }
    Err(EnrolmentError::Malformed)
}

fn text(bytes: &[u8]) -> Result<String, EnrolmentError> {
    String::from_utf8(bytes.to_vec()).map_err(|_| EnrolmentError::Malformed)
}

#[cfg(test)]
mod tests;
