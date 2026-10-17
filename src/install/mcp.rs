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

use super::hooks::binary_stem;
use super::jsonfile::ensure_object;

/// The key under `mcpServers` that this client owns: the product's binary name.
///
/// It was `"yadgar"`, which is also the key the separate yadgar memory MCP
/// registers itself under. Sharing it meant install overwrote that server and
/// uninstall deleted it. The two are now disjoint, and the MCP tool prefix the
/// agent sees is `mcp__yaadgaar__`.
pub const SERVER_KEY: &str = "yaadgaar";

/// The key older client versions wrote the same entry under.
///
/// Touched only when the entry there is provably ours ([`is_ours`]); otherwise
/// it belongs to somebody else (the memory MCP) and is never read as ours.
pub const LEGACY_SERVER_KEY: &str = "yadgar";

/// The one key in `~/.claude.json` that yadgar creates itself.
///
/// The counterpart of [`super::hooks::HOOKS_KEY`], and the distinction it
/// carries is the whole of the bug that made both constants exist: THIS key is
/// structure, and every key UNDER it is a name a person chose. An empty
/// `mcpServers` is nothing; an `mcpServers` holding `other-server` with an
/// empty body is somebody's registration, because the name IS the data — and
/// uninstall deleted the file for want of a scalar in it.
pub const SERVERS_KEY: &str = "mcpServers";

/// The subcommand the agent spawns (D75).
pub const SERVE_VERB: &str = "serve";

/// Write yadgar's entry, leaving every other MCP server untouched.
///
/// The entry is REPLACED WHOLE rather than merged field by field. A field-wise
/// merge over the entry that exists on machines today — `{"type":"http","url":…,
/// "headers":{"Authorization":"Bearer …"}}` — would leave the url and the live
/// token sitting beside the new `command`, so an install whose entire purpose is
/// to stop carrying tokens would have carried one forward.
///
/// Call [`check`] first: this assumes any entry under [`SERVER_KEY`] is ours.
/// A legacy entry of ours under [`LEGACY_SERVER_KEY`] is moved, not duplicated.
pub fn merge(config: &mut Value, binary: &Path) {
    let Some(servers) = ensure_object(config, SERVERS_KEY) else {
        return;
    };
    if servers.get(LEGACY_SERVER_KEY).is_some_and(is_ours) {
        servers.remove(LEGACY_SERVER_KEY);
    }
    servers.insert(SERVER_KEY.to_string(), entry_for(binary));
}

/// Refuse to install over an entry that is not ours.
///
/// Pre-flight: runs before anything is written, so a refusal leaves the machine
/// untouched. Overwriting somebody else's registration is the defect this exists
/// to prevent.
pub fn check(config: &Value, path: &Path) -> anyhow::Result<()> {
    match config.get(SERVERS_KEY).and_then(|s| s.get(SERVER_KEY)) {
        Some(entry) if !is_ours(entry) => anyhow::bail!(
            "refusing to overwrite the `{SERVERS_KEY}.{SERVER_KEY}` entry in {}: it was not \
             written by this client (expected a stdio server running a `yaadgaar` binary \
             with the argument `{SERVE_VERB}`). Rename or remove it, then run install again.",
            path.display()
        ),
        _ => Ok(()),
    }
}

/// Whether *entry* has the shape [`entry_for`] writes: stdio, `serve`, no url or
/// headers, and a command whose file name is the `yaadgaar` binary.
///
/// The file name comes from [`binary_stem`], not `Path::file_stem`: the entry
/// may have been written on another platform than the one reading it.
pub fn is_ours(entry: &Value) -> bool {
    let Some(obj) = entry.as_object() else {
        return false;
    };
    let stdio = obj.get("type").and_then(Value::as_str) == Some("stdio");
    let serve = obj
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|a| a.len() == 1 && a[0].as_str() == Some(SERVE_VERB));
    let binary = obj
        .get("command")
        .and_then(Value::as_str)
        .and_then(binary_stem)
        .is_some_and(|stem| stem.eq_ignore_ascii_case("yaadgaar"));
    stdio && serve && binary && !obj.contains_key("url") && !obj.contains_key("headers")
}

/// Remove this client's entry and nothing else.
///
/// Only an entry that is ours goes: a foreign `yaadgaar` entry stays, and so
/// does any `yadgar` entry that is not provably ours. `yadgar` is the memory
/// MCP's key and is never touched unless it carries this client's own shape.
///
/// An `mcpServers` object left empty is NOT removed, and the asymmetry with the
/// hook events — where an emptied key is dropped — is deliberate. Claude Code
/// writes `mcpServers` itself and an empty one is its own resting state; the
/// hook event keys it does not write, so `"PreCompact": []` is residue this
/// installer created and nobody else's business.
///
/// That is a decision about the KEY, not about the file. One call up,
/// `jsonfile::write_or_prune` deletes `~/.claude.json` outright when stripping
/// this entry leaves the whole file holding nothing — a file yadgar created on
/// a machine that had none. The two do not disagree: an empty `mcpServers` is
/// left alone inside a file somebody else has things in, and is not worth
/// keeping a file alive for on its own.
pub fn strip(config: &mut Value) {
    if let Some(servers) = config.get_mut(SERVERS_KEY).and_then(Value::as_object_mut) {
        for key in [SERVER_KEY, LEGACY_SERVER_KEY] {
            if servers.get(key).is_some_and(is_ours) {
                servers.remove(key);
            }
        }
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
    fn an_entry_written_on_windows_is_ours_when_read_on_any_platform() {
        // `Path::file_stem` splits on `\` only on a Windows build, so on Linux
        // this whole string is one "file name" and the stem is not `yaadgaar`.
        // A `.claude.json` read on WSL beside a Windows install would then make
        // install refuse over its own entry and uninstall leave it behind.
        let entry = json!({
            "type": "stdio",
            "command": "C:\\Program Files\\yaadgaar\\yaadgaar.exe",
            "args": ["serve"],
        });
        assert!(is_ours(&entry), "{entry}");
    }
}
