//! `yupana share reshare` — publish a graph back out as a share bundle, and
//! name its parent when it has one.
//!
//! # Why the parent is DERIVED rather than asked for
//!
//! A share that came from someone else and goes back out changed is a DELTA,
//! and a delta that does not name its parent has lost the only thing that makes
//! it a delta. The iv3df7.5 discipline says name it; this module makes
//! forgetting structurally difficult instead of merely discouraged.
//!
//! Quipu names a staged graph after the share it came from —
//! `urn:quipu:import:staging:<hash>` / `urn:quipu:import:quarantine:<hash>`,
//! where `<hash>` is the share id minus its `sha256:` prefix. So the lineage is
//! already written on the graph IRI, and re-sharing a pulled graph can recover
//! its parent without being told. [`parent_of`] is that recovery, and it is
//! deliberately total: a graph that is NOT a pulled one yields `None` and
//! re-shares as a root, because inventing a parent for a local graph would be a
//! worse failure than omitting one.
//!
//! `--parent` overrides the derivation and `--root` refuses it outright, so an
//! operator who means to publish a pulled graph as a new lineage can say so —
//! but they have to say it.
//!
//! # Why this is REST, like the rest of `share`
//!
//! `POST /share` returns the bundle as `manifest` plus exact file contents, so a
//! consumer with no local store can author one. That is the same reason
//! `share pull` posts to `/import`: yupana talks to a quipu server, and a
//! `--db` path would mean nothing to it.

use std::path::Path;

use crate::errors::{Error, Result};

/// The two prefixes quipu uses for a staged import graph.
const STAGING: &str = "urn:quipu:import:staging:";
const QUARANTINE: &str = "urn:quipu:import:quarantine:";

/// The share id a pulled graph came from, or `None` for a graph that is not one.
///
/// Total by design: a local graph IRI is not a failure here, it simply has no
/// parent. Only a graph quipu itself named after a share can carry one.
pub(crate) fn parent_of(graph: &str) -> Option<String> {
    let hash = graph
        .strip_prefix(STAGING)
        .or_else(|| graph.strip_prefix(QUARANTINE))?;
    // Quipu's own identifier check: 64 lowercase hex. Anything else is a graph
    // that merely starts with the prefix, and guessing a parent from it would
    // put a false lineage claim in a published manifest.
    if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(format!("sha256:{hash}"))
    } else {
        None
    }
}

/// Write a `SharePayload`'s files to a directory, verbatim.
///
/// The bytes are written exactly as quipu produced them, because the manifest's
/// hashes are over those bytes: re-serializing the manifest here — even to
/// pretty-print it — would produce a bundle whose own `share_id` no longer
/// verifies. `files` is the authority, not the parsed `manifest`.
pub(crate) fn write_bundle(payload: &serde_json::Value, out: &Path) -> Result<Vec<String>> {
    let files = payload
        .get("files")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| Error::Share("quipu's share response carried no `files` map".to_string()))?;
    if files.is_empty() {
        return Err(Error::Share(
            "quipu returned a share with no files — refusing to write an empty bundle".to_string(),
        ));
    }
    std::fs::create_dir_all(out)
        .map_err(|e| Error::Share(format!("creating {}: {e}", out.display())))?;
    let mut written = Vec::new();
    for (name, content) in files {
        // A filename from the server is still a path component we are about to
        // join: refuse anything that could escape the output directory.
        if name.contains('/') || name.contains('\\') || name.starts_with('.') {
            return Err(Error::Share(format!(
                "refusing to write share file with a suspicious name: {name:?}"
            )));
        }
        let text = content
            .as_str()
            .ok_or_else(|| Error::Share(format!("share file {name:?} was not a string")))?;
        std::fs::write(out.join(name), text)
            .map_err(|e| Error::Share(format!("writing {name}: {e}")))?;
        written.push(name.clone());
    }
    Ok(written)
}

/// Build the `POST /share` request body.
pub(crate) fn request(
    graph: &str,
    parent: Option<&str>,
    shapes: &[String],
    no_shapes: bool,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "scope": { "kind": "graph", "value": graph },
        "shapes": shapes,
        "no_shapes": no_shapes,
    });
    if let Some(parent) = parent {
        body["parent_share"] = serde_json::Value::String(parent.to_string());
    }
    body
}

#[cfg(test)]
#[path = "share_reshare_test.rs"]
mod share_reshare_test;
