//! Reading and VERIFYING a Quipu share bundle, before anything is sent anywhere.
//!
//! A share bundle is four files — `manifest.json`, `export.nt`, `shapes.ttl`,
//! and an optional `export.ttl` turtle view. The manifest declares a SHA-256
//! over the exact bytes of the two payloads it ships, which is the publisher's
//! statement of what they meant to send.
//!
//! **This module checks those hashes locally, and refuses on a mismatch, even
//! though quipu's server checks them again.** That is deliberate duplication,
//! for two reasons that are not the same reason:
//!
//! 1. **A local refusal is a different finding from a remote one.** If the
//!    bytes were corrupted in transit to us — a truncated download, a
//!    half-written file, a mangled checkout — the honest report is "the bundle
//!    you handed me is not the bundle its manifest describes", named here,
//!    before the graph is involved at all. Shipping corrupt bytes to quipu and
//!    relaying its complaint puts a store write in the middle of a story that
//!    is entirely local.
//! 2. **It is testable without a server.** The whole point of a verification
//!    step is that it REFUSES; a guard whose refusal has only ever been reasoned
//!    about is not a verified guard. These hashes can be tampered with in a
//!    `tempfile` and the refusal observed, with no quipu running anywhere.
//!
//! What this module deliberately does NOT verify is `share_id`, which quipu
//! computes over a canonical serialization of the manifest with the id omitted.
//! Reimplementing that canonicalization here would be a second source of truth
//! for share identity, and a drift between the two would be worse than not
//! checking: the server checks it, and the server is the authority on it.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::errors::{Error, Result};

/// The payload file names quipu's manifest schema fixes. A bundle naming
/// anything else is refused by the server (`share manifest contains unsupported
/// payload paths`), so there is no value in guessing here.
pub(crate) const MANIFEST: &str = "manifest.json";
pub(crate) const EXPORT_NT: &str = "export.nt";
pub(crate) const SHAPES_TTL: &str = "shapes.ttl";

/// A share bundle's bytes plus where they came from.
///
/// `source` rides along because it becomes the import's provenance in quipu
/// (`share-import:<source>`), so it must name the place a reader could go back
/// to — not a temporary directory we happened to stage it in.
#[derive(Debug, Clone)]
pub(crate) struct Bundle {
    /// The manifest, parsed but not interpreted — yupana passes it through to
    /// quipu verbatim rather than modelling a schema it does not own.
    pub(crate) manifest: serde_json::Value,
    pub(crate) export_nt: String,
    pub(crate) shapes_ttl: String,
    pub(crate) source: String,
}

impl Bundle {
    /// The publisher's share id, if the manifest carries one.
    pub(crate) fn share_id(&self) -> Option<&str> {
        self.manifest.get("share_id").and_then(|v| v.as_str())
    }

    /// Whether the bundle ships any shapes at all.
    ///
    /// `quipu share --no-shapes` is a documented, ordinary way to produce a
    /// bundle, so an empty `shapes.ttl` is a real case and not a corner. It
    /// matters downstream: suggesting that the operator adopt a vocabulary that
    /// would adopt nothing is worse than suggesting nothing.
    pub(crate) fn has_shapes(&self) -> bool {
        !self.shapes_ttl.trim().is_empty()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Refuse anything whose declared hash does not match the bytes we hold.
///
/// The message names BOTH hashes. A mismatch report that gives only "hash
/// mismatch" cannot distinguish a truncated download from a substituted
/// payload, and those want different responses from the reader.
pub(crate) fn verify(bundle: &Bundle) -> Result<()> {
    for (label, file, declared, bytes) in [
        (
            "graph",
            EXPORT_NT,
            bundle.manifest.get("graph_hash").and_then(|v| v.as_str()),
            bundle.export_nt.as_bytes(),
        ),
        (
            "shapes",
            SHAPES_TTL,
            bundle.manifest.get("shapes_hash").and_then(|v| v.as_str()),
            bundle.shapes_ttl.as_bytes(),
        ),
    ] {
        let Some(declared) = declared else {
            return Err(Error::Share(format!(
                "share manifest declares no {label}_hash — this is not a v1 share bundle, \
                 and yupana will not import bytes nobody has vouched for"
            )));
        };
        let actual = sha256_hex(bytes);
        if declared != actual {
            return Err(Error::Share(format!(
                "share {label} hash MISMATCH for {file}: manifest declares {declared}, \
                 the bytes here hash to {actual}. Nothing was sent to quipu. The bundle \
                 is not the one its manifest describes — re-fetch it, and if it still \
                 mismatches, the publisher's bytes and their manifest disagree."
            )));
        }
    }
    Ok(())
}

/// Read a bundle from a directory, or over HTTP from a base URL that serves the
/// three files beneath it.
///
/// **A `.qpack.db` and a bundle ARCHIVE are refused, by name, with the command
/// that does work.** That refusal is an architectural fact and not a gap:
/// quipu's `unpack` and `pack --verify` exist ONLY on the CLI — measured
/// 2026-09-05, there is no `/unpack` or `/pack` route in quipu's server — and
/// they operate on a local store file. Yupana has no local store; every quipu
/// interaction it has is HTTP to an endpoint whose database lives on another
/// host. So there is no honest way for yupana to unpack a pack, and pretending
/// otherwise would mean shipping a verb that fails at the far end for a reason
/// the caller cannot act on.
pub(crate) fn read(source: &str) -> Result<Bundle> {
    if source.starts_with("http://") || source.starts_with("https://") {
        return read_http(source);
    }
    let path = PathBuf::from(source);
    if path.is_file() {
        return Err(unsupported_local_artifact(&path));
    }
    if !path.is_dir() {
        return Err(Error::Share(format!(
            "{source}: no such share directory, and not an http(s) URL"
        )));
    }
    if !path.join(MANIFEST).is_file() {
        return Err(Error::Share(format!(
            "{source} is a directory but has no {MANIFEST} — that is not a share bundle. \
             `quipu share --output <dir>` writes one."
        )));
    }
    let manifest = read_file(&path.join(MANIFEST))?;
    Ok(Bundle {
        manifest: parse_manifest(&manifest, source)?,
        export_nt: read_file(&path.join(EXPORT_NT))?,
        // A bundle produced with `--no-shapes` legitimately has no shapes file.
        shapes_ttl: read_optional(&path.join(SHAPES_TTL)),
        source: source.to_string(),
    })
}

/// The refusal for a local file that is an artifact of the sharing story but
/// not one yupana can consume. It names the working command rather than
/// describing one — a discovery tool that hands you an action it has just told
/// you will not work is worse than one that offers none.
fn unsupported_local_artifact(path: &Path) -> Error {
    let shown = path.display();
    let what = if path.extension().is_some_and(|e| e == "db") {
        "a packed graph (`.qpack.db`)"
    } else {
        "a file"
    };
    Error::Share(format!(
        "{shown} is {what}, and yupana cannot load it: quipu's `unpack`/`pack` exist only \
         on the CLI against a LOCAL store, and yupana talks to a quipu SERVER over HTTP. \
         Load it with the quipu CLI on the machine holding the store:\n    \
         quipu pack --verify {shown}\n    quipu unpack {shown} --db <store.db>\n\
         Then `yupana share pull` a bundle exported from that store, or point this verb \
         at a share directory."
    ))
}

fn read_http(base: &str) -> Result<Bundle> {
    let base = base.trim_end_matches('/');
    if base.ends_with(".db") || base.ends_with(".tar.gz") || base.ends_with(".tgz") {
        return Err(Error::Share(format!(
            "{base} is a packed or archived artifact. yupana reads a share bundle as \
             files, and carries no archive extractor by design. The quipu CLI takes this \
             form directly:\n    quipu import {base} --db <store.db>\n\
             Or point this verb at an unpacked bundle directory or a base URL serving \
             {MANIFEST} beneath it."
        )));
    }
    let manifest = fetch(&format!("{base}/{MANIFEST}"))?;
    Ok(Bundle {
        manifest: parse_manifest(&manifest, base)?,
        export_nt: fetch(&format!("{base}/{EXPORT_NT}"))?,
        shapes_ttl: fetch(&format!("{base}/{SHAPES_TTL}")).unwrap_or_default(),
        source: base.to_string(),
    })
}

fn fetch(url: &str) -> Result<String> {
    // A transport failure and a failed VERIFICATION are different findings and
    // must not collapse into one bucket: this is "I could not get the bytes",
    // never "the bytes are wrong".
    ureq::get(url)
        .timeout(std::time::Duration::from_secs(120))
        .call()
        .map_err(|e| Error::Share(format!("fetching {url}: {e}")))?
        .into_string()
        .map_err(|e| Error::Share(format!("reading {url}: {e}")))
}

fn parse_manifest(text: &str, source: &str) -> Result<serde_json::Value> {
    serde_json::from_str(text)
        .map_err(|e| Error::Share(format!("{source}/{MANIFEST} is not valid JSON: {e}")))
}

fn read_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| Error::Share(format!("reading {}: {e}", path.display())))
}

fn read_optional(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[cfg(test)]
#[path = "share_bundle_test.rs"]
mod share_bundle_test;
