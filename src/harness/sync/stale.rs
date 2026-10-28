//! Records of done or dropped tasks that no state owns (#460).
//!
//! A fork copies the parent's list byte for byte. Records of tasks that were
//! out of scope when it forked carry an ownership line (or are legacy seeds)
//! for a URN the fork's state does not own and no in-scope pass names, so
//! nothing else ever removes them.
//!
//! Such a record is deleted only when yaadgaar lists its task as done or
//! dropped: the lookup is ONE paged `find_tasks` for those statuses, made only
//! when a candidate exists. A task the listing does not hold (another
//! project, one yaadgaar no longer has, one still open under a narrower
//! scope) is not proof of anything, so its record is kept. A failed lookup
//! keeps every record.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::super::plan::Action;
use super::super::record::{self, Desired};
use super::super::state::State;
use super::Closed;

/// The `Stale` actions for this pass. `Err` carries how many records were
/// kept because *lookup* failed. *lookup* is called only when a record might
/// be stale.
pub(super) fn actions(
    desired: &[Desired],
    st: &State,
    files: &BTreeMap<u64, Value>,
    lookup: &dyn Fn() -> Result<Closed, String>,
) -> Result<Vec<Action>, usize> {
    let candidates = candidates(desired, st, files);
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let closed = lookup().map_err(|_| candidates.len())?;
    // The number comes from yaadgaar, not the file: the delete checks the
    // file against both ([`record::copy_of`]), as an adopt does.
    Ok(candidates
        .into_iter()
        .filter_map(|(harness_id, urn)| {
            let number = *closed.get(&urn)?;
            Some(Action::Stale {
                urn,
                number,
                harness_id,
            })
        })
        .collect())
}

/// Every file no state entry owns whose ownership line, or legacy seed
/// description, names a yaadgaar URN that is not in scope.
fn candidates(desired: &[Desired], st: &State, files: &BTreeMap<u64, Value>) -> Vec<(u64, String)> {
    let owned: BTreeSet<u64> = st.entries.values().map(|e| e.harness_id).collect();
    let in_scope: BTreeSet<&str> = desired.iter().map(|d| d.urn.as_str()).collect();
    files
        .iter()
        .filter(|(id, _)| !owned.contains(id))
        .filter_map(|(id, f)| {
            let urn = record::owner(f).or_else(|| legacy_urn(f))?;
            (urn.starts_with(record::URN_SCHEME) && !in_scope.contains(urn))
                .then(|| (*id, urn.to_string()))
        })
        .collect()
}

/// The URN a legacy seed description names, `yaadgaar #<n> · <urn> · …`.
/// Loose on purpose: only a candidate. The delete needs [`record::legacy_of`].
fn legacy_urn(file: &Value) -> Option<&str> {
    let description = file.get("description")?.as_str()?;
    let rest = description.strip_prefix("yaadgaar #")?;
    rest.split(" · ").nth(1)
}

/// A [`Desired`] carrying only what [`record::copy_of`] reads: the URN and
/// the number. Never written to a file.
pub(super) fn probe(urn: &str, number: i64) -> Desired {
    Desired {
        urn: urn.to_string(),
        number,
        title: String::new(),
        status: String::new(),
        version: 0,
    }
}
