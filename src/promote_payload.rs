//! Refused-payload diagnostics for promotion, split out of `promote` for size
//! (yupana #83): dumping the exact Turtle a gate refused, bounded per failing
//! commit, and naming the dump in the error the operator reads.

/// Write the projection that failed to disk, and return where it landed.
///
/// WHY A FAILED PROMOTION MUST LEAVE ITS PAYLOAD BEHIND. The Turtle is generated,
/// posted and dropped; when it does not parse or does not conform, the document
/// that failed no longer exists anywhere. The parse errors are positional —
/// "line 8656 between columns 1 and 97" — so without the payload the one fact the
/// error gives you is unusable, and the line MOVES between runs because the
/// content is regenerated. Measured (aegis-o8rq8): a scheduled promotion failed
/// every hour for a day and the payload's absence was most of why diagnosing it
/// was hard.
///
/// Best-effort BY DESIGN: a promotion already failing must not fail differently
/// because a dump could not be written, so every error here collapses to `None`
/// and the caller reports the original failure without a path.
///
/// The filename is derived from `source`, which for a CLI promotion carries the
/// repo AND the resolved commit. So the hourly case — the same commit refused
/// over and over because the marker did not advance — overwrites ONE file rather
/// than growing without bound; a diagnostic that fills a disk is its own outage.
/// Across DIFFERENT failing commits it is one dump each, deliberately: the SHA is
/// what tells you which projection you are holding, and reusing one name would
/// overwrite the payload you were still reading. That bound is
/// distinct-failing-commits, not runs. `YUPANA_PROMOTE_DUMP_DIR` overrides the
/// temp-dir default.
pub(super) fn dump_payload(turtle: &str, source: &str) -> Option<std::path::PathBuf> {
    let dir = std::env::var("YUPANA_PROMOTE_DUMP_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    dump_payload_to(&dir, turtle, source)
}

/// [`dump_payload`] with the directory passed in.
///
/// Split so the whole of the write — the naming, the directory creation, the
/// best-effort contract — is testable without setting `YUPANA_PROMOTE_DUMP_DIR`:
/// parallel tests race on env vars, and this crate denies `unsafe_code`, which
/// `std::env::set_var` now requires.
pub(super) fn dump_payload_to(
    dir: &std::path::Path,
    turtle: &str,
    source: &str,
) -> Option<std::path::PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("yupana-promote-{}.ttl", payload_slug(source)));
    std::fs::write(&path, turtle).ok()?;
    Some(path)
}

/// A filesystem-safe stem for a dump file, from the promotion's `source` string.
///
/// Anything outside `[A-Za-z0-9-]` becomes `-`, so a source carrying a path, a
/// URL or a shell metacharacter cannot escape the dump directory or produce a
/// name the shell would re-interpret. `.` is excluded too, deliberately: keeping
/// it would let a source spelling `..` survive into the name, which is harmless
/// only for as long as no separator ever survives with it. Bounded length keeps
/// the name under filesystem limits; an empty result falls back to a constant
/// rather than to a bare extension.
pub(super) fn payload_slug(source: &str) -> String {
    let mut s: String = source
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    s.truncate(80);
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "payload".to_string()
    } else {
        s
    }
}

/// Append the retained-payload path to a failure message, when one was written.
pub(super) fn with_payload(message: String, payload: Option<&std::path::Path>) -> String {
    match payload {
        Some(p) => format!("{message}\n  payload retained at: {}", p.display()),
        None => message,
    }
}
