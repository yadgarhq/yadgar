//! The MCP registration: one entry, user-level, carrying no credential.
//!
//! The entry names the binary and nothing else. `yadgar serve` is a local stdio
//! MCP server that forwards to the gateway with the credential attached on the
//! way out (D75), so the agent never holds a token and neither does this file.
//!
//! **The token must never be written here.** The Python version wrote
//! `Authorization: Bearer <literal>` into `~/.claude.json`, which is a secret at
//! rest in a file that gets pasted into bug reports — and a dry-run once echoed
//! a real one to stdout. The entry that replaces it has no `headers` key at all.

use std::path::Path;

use serde_json::{json, Value};

use super::jsonfile::ensure_object;

/// The key under `mcpServers` that yadgar owns.
pub const SERVER_KEY: &str = "yadgar";

/// The subcommand the agent spawns (D75).
pub const SERVE_VERB: &str = "serve";

/// Write yadgar's entry, leaving every other MCP server untouched.
///
/// The entry is REPLACED WHOLE rather than merged field by field. A field-wise
/// merge over the entry that exists on machines today — `{"type":"http","url":…,
/// "headers":{"Authorization":"Bearer …"}}` — would leave the url and the live
/// token sitting beside the new `command`, so an install whose entire purpose is
/// to stop carrying tokens would have carried one forward.
pub fn merge(config: &mut Value, binary: &Path) {
    let Some(servers) = ensure_object(config, "mcpServers") else {
        return;
    };
    servers.insert(SERVER_KEY.to_string(), entry_for(binary));
}

/// Remove yadgar's entry and nothing else.
pub fn strip(config: &mut Value) {
    if let Some(servers) = config.get_mut("mcpServers").and_then(Value::as_object_mut) {
        servers.remove(SERVER_KEY);
    }
}

/// The entry itself: a command, an argument, no credential, no URL.
///
/// No URL because the gateway address is the binary's own configuration, not the
/// agent's. Putting it here would mean two places to change it and one of them
/// silently winning.
pub fn entry_for(binary: &Path) -> Value {
    json!({
        "type": "stdio",
        "command": binary.to_string_lossy(),
        "args": [SERVE_VERB],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_carries_no_credential() {
        // The failure this prevents: a token at rest in a file people paste into
        // bug reports. Asserted on the whole serialised entry rather than on the
        // absence of one key, so a future field cannot smuggle one back in.
        let text = entry_for(Path::new("/usr/local/bin/yadgar")).to_string();
        assert!(!text.contains("Bearer"), "{text}");
        assert!(!text.to_lowercase().contains("authorization"), "{text}");
        assert!(!text.to_lowercase().contains("token"), "{text}");
    }

    #[test]
    fn a_legacy_http_entry_is_replaced_whole_not_merged() {
        // The failure this prevents: the entry live on machines today carries a
        // real bearer token. A field-wise merge leaves `headers` in place beside
        // the new `command`, and the install that was supposed to remove the
        // token preserves it instead.
        let mut config = json!({
            "mcpServers": {
                "yadgar": {
                    "type": "http",
                    "url": "http://127.0.0.1:8765/mcp",
                    "headers": { "Authorization": "Bearer a-real-token" }
                }
            }
        });
        merge(&mut config, Path::new("/usr/local/bin/yadgar"));
        let entry = &config["mcpServers"]["yadgar"];
        assert_eq!(entry["type"], "stdio");
        assert!(entry.get("headers").is_none(), "{entry}");
        assert!(entry.get("url").is_none(), "{entry}");
    }
}
