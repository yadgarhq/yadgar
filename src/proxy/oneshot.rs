//! One `tools/call` sent on this client's own behalf, outside `serve`.
//!
//! The `session-start` hook lists the open tasks before the agent has made a
//! single call. It must reach the gateway EXACTLY as `serve` does: the same
//! project derivation, the same `_meta` the gateway refuses a body without, and
//! the same mirrored headers it cross-checks (`-32020 HeaderMismatch`). A second
//! hand-built request would be a second copy of those rules to drift from, so
//! this goes through [`super::forward`] and [`super::session::fill_meta`].

use std::path::Path;

use serde_json::{json, Value};

use super::context::Context;
use super::Outcome;
use crate::config::Config;

/// A client, a config and a workspace, ready to call one tool at a time.
pub(crate) struct ToolCaller {
    client: reqwest::Client,
    config: Config,
    context: Context,
}

impl ToolCaller {
    /// Resolve the workspace at *cwd* the way `serve` resolves it.
    pub(crate) fn new(client: reqwest::Client, config: Config, cwd: &Path) -> Self {
        let context = Context::discover(&config, cwd);
        Self {
            client,
            config,
            context,
        }
    }

    /// The project key the gateway will be told, or `None` when nothing names one.
    pub(crate) fn project(&self) -> Option<&str> {
        self.context.project.as_deref()
    }

    /// Call *name* with *arguments*, and return what the gateway answered.
    ///
    /// No capabilities are declared: this caller is not an MCP host and can act
    /// on none. The empty object is a valid value meaning exactly that.
    pub(crate) async fn call(&self, name: &str, arguments: Value) -> Outcome {
        let envelope = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        });
        let body = super::session::fill_meta(&envelope, &json!({}))
            .unwrap_or_else(|| envelope.to_string());
        super::forward(&self.client, &self.config, &self.context, &body).await
    }
}
