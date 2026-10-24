//! Payload shapes the guard cannot read must refuse (ledger 1576 (a)).
//!
//! Closed by yadgar#64; these two are the shapes no test named. Kept out of
//! `guard/tests/mod.rs` on purpose.

use serde_json::{json, Value};

use super::{pre_tool_guard, Decision, Policy};

fn judge(payload: &Value) -> Decision {
    pre_tool_guard(payload, &Policy::load(None), None)
}

#[test]
fn a_top_level_null_refuses() {
    assert!(matches!(judge(&Value::Null), Decision::Deny(_)));
}

#[test]
fn a_bash_call_whose_input_is_null_or_empty_or_not_a_string_refuses() {
    for payload in [
        json!({"tool_name": "Bash", "tool_input": null}),
        json!({"tool_name": "Bash", "tool_input": {}}),
        json!({"tool_name": "Bash", "tool_input": {"command": null}}),
        json!({"tool_name": "Bash", "tool_input": {"command": ["git"]}}),
    ] {
        let Decision::Deny(reason) = judge(&payload) else {
            panic!("allowed a payload it could not read: {payload}");
        };
        assert!(reason.contains("payload"), "{reason}");
    }
}
