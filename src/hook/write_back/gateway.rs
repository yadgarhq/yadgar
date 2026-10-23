//! The gateway half of the write-back hooks: one bounded tool call, and what
//! its failure means. Split from `write_back.rs` along the seam between
//! deciding and calling.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use super::{str_at, Inputs};
use crate::config::ConfigError;
use crate::printable::printable;
use crate::proxy::{Outcome, ToolCaller};

/// Why one call did not land.
pub(super) enum Fault {
    /// yaadgaar answered and refused (`isError: true`): a stale version or an
    /// illegal move. The file is refreshed from yaadgaar.
    Refused(String),
    /// No answer, or not one about the task. Nothing is refreshed.
    Unreachable(String),
}

pub(super) fn unreachable(why: &str) -> String {
    format!(
        "could not reach yaadgaar ({}); the change was not made. Retry.",
        printable(why)
    )
}

/// A caller for *project*, from the saved login.
pub(super) fn caller(
    inputs: &Inputs,
    project: &str,
    payload: &Value,
) -> Result<ToolCaller, String> {
    let config = match (inputs.config)() {
        Ok(config) => config,
        Err(ConfigError::Missing) => return Err("not logged in; run `yaadgaar login`".into()),
        Err(_) => return Err("the saved login could not be read; run `yaadgaar login`".into()),
    };
    let client = crate::trust::client(config.ca_pem(), Some(inputs.deadline * 2))
        .map_err(|e| e.to_string())?;
    let cwd = str_at(payload, "cwd")
        .map(PathBuf::from)
        .unwrap_or_default();
    Ok(ToolCaller::for_project(client, config, &cwd, project))
}

pub(super) fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| unreachable(&format!("could not start: {e}")))
}

pub(super) async fn bounded(
    deadline: Duration,
    work: impl std::future::Future<Output = Result<Value, Fault>>,
) -> Result<Value, Fault> {
    tokio::time::timeout(deadline, work)
        .await
        .unwrap_or_else(|_| {
            Err(Fault::Unreachable(format!(
                "the gateway did not answer within {} s",
                deadline.as_secs()
            )))
        })
}

/// One tool call, and its `structuredContent`.
pub(super) async fn call(caller: &ToolCaller, tool: &str, args: Value) -> Result<Value, Fault> {
    let body = match caller.call(tool, args).await {
        Outcome::Answered(body) => body,
        Outcome::Unreachable(_) => {
            return Err(Fault::Unreachable(
                "the gateway could not be reached".into(),
            ))
        }
        Outcome::Rejected(reqwest::StatusCode::UNAUTHORIZED, ..) => {
            return Err(Fault::Unreachable(
                "the gateway refused the saved credential; run `yaadgaar login` again".into(),
            ))
        }
        Outcome::Rejected(status, ..) => {
            return Err(Fault::Unreachable(format!("the gateway answered {status}")))
        }
    };
    let malformed = || Fault::Unreachable(format!("the gateway's answer to {tool} was not read"));
    let value: Value = serde_json::from_str(&body).map_err(|_| malformed())?;
    if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
        return Err(Fault::Unreachable(format!(
            "the gateway refused {tool}: {message}"
        )));
    }
    let result = value.get("result").ok_or_else(malformed)?;
    if result.get("isError") == Some(&Value::Bool(true)) {
        let text = result
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        return Err(Fault::Refused(format!("{tool}: {text}")));
    }
    result
        .get("structuredContent")
        .cloned()
        .ok_or_else(malformed)
}
