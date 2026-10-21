//! The stored credential expiry (ADR-0972 (3), ledger 1628).
//!
//! `/auth/login` and `/auth/enrol` answer an `expires_at` in RFC 3339 UTC, and
//! the client stores the text as the gateway sent it. This module reads that
//! text back, decides whether it has passed, and says so.
//!
//! **HAND-PARSED, and only the one shape the gateway writes.** The gateway
//! renders `prost_types::Timestamp`, which is always `Z` with 0, 3, 6 or 9
//! fraction digits. A time crate would be a new dependency for one format.
//! Anything else reads as "not a time", and an unreadable expiry is never
//! treated as past: the gateway's own 401 reason is then shown, as before.

/// Seconds since the Unix epoch of an RFC 3339 UTC time, or `None`.
///
/// The fraction is checked and dropped: an expiry is compared in whole
/// seconds.
pub fn parse(text: &str) -> Option<i64> {
    let rest = text.strip_suffix('Z')?;
    let (date, time) = rest.split_once('T')?;
    let (clock, fraction) = match time.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (time, None),
    };
    if fraction.is_some_and(|f| f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let [year, month, day] = fields::<3>(date, '-', &[4, 2, 2])?;
    let [hour, minute, second] = fields::<3>(clock, ':', &[2, 2, 2])?;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => return None,
    };
    if day == 0 || day > days_in_month || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Split *text* on *sep* into `N` all-digit fields of the given widths.
fn fields<const N: usize>(text: &str, sep: char, widths: &[usize; N]) -> Option<[i64; N]> {
    let mut out = [0; N];
    let mut parts = text.split(sep);
    for (slot, width) in out.iter_mut().zip(widths) {
        let part = parts.next()?;
        if part.len() != *width || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Days from 1970-01-01 to the given proleptic Gregorian date (Howard
/// Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The wall clock, in Unix seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The stored text when it parses and is at or before *now*.
///
/// `None` for no expiry (a legacy credential, which never expires) and for an
/// unreadable one.
pub fn expired_at(stored: Option<&str>, now: i64) -> Option<&str> {
    stored.filter(|text| parse(text).is_some_and(|at| at <= now))
}

/// The stored expiry for display, or `unknown`.
///
/// `unknown`, not `never`: a config written by an older client, or from an
/// older gateway's answer, holds no expiry even when the credential has one.
pub fn shown(stored: Option<&str>) -> &str {
    stored.unwrap_or("unknown")
}

/// What a 401 says once the stored expiry has passed.
pub fn expired_sentence(at: &str) -> String {
    format!("your credential expired at {at}; run `yaadgaar login`")
}

#[cfg(test)]
mod tests;
