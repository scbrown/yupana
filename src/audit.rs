//! audit — what a guard decision RECORDS about its subject (yupana #77).
//!
//! The spool (`crate::metrics`) answers "how many denies, of what kind". This
//! module answers the question that follows it: **which file, and under which
//! rule**. A deny line carrying `agent`, `tenant`, `ext`, `mode` and `result`
//! is auditable in aggregate and unauditable in particular — it can prove that
//! 18 writes were blocked and cannot say whether they were the files that broke
//! something, files near them, or something unrelated. That gap was measured on
//! a live host: a fleet-wide incident during a deny burst could not be tied to
//! or cleared of the guard, because the record had no subject.
//!
//! Paths are more sensitive than extensions, so recording them is OPT-IN and
//! configurable rather than simply switched on: a repo-relative path is the
//! useful default for a fleet, an absolute path leaks the host's directory
//! layout, and some deployments want neither. [`PathRecording::Off`] is the
//! default and reproduces the previous behaviour exactly.
//!
//! The rule id is NOT gated the same way. "Which rule fired" is what makes a
//! false positive diagnosable — without it a wrongly-scoped rule and a
//! correctly-scoped one produce identical records — and a rule id is a name the
//! operator themselves wrote, not user content, so it carries none of the
//! sensitivity that argues for gating paths.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// How much of an edit's target path a guard record may carry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathRecording {
    /// Record no path at all — the pre-#77 behaviour, and the default. A
    /// deployment opts IN to path recording; it is never turned on beneath one.
    #[default]
    Off,
    /// Record the repo-relative path (`src/auth.rs`). The useful setting for a
    /// fleet: it identifies the file without disclosing where the checkout
    /// lives or what the enclosing directories are named.
    Relative,
    /// Record the absolute path. Discloses the host's directory layout —
    /// including, on a multi-customer host, directory names that may themselves
    /// be sensitive. For single-tenant hosts where the relative path is
    /// ambiguous across several checkouts.
    Absolute,
}

/// Settings for what the usage spool records.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsConfig {
    /// Whether guard records carry the edited path, and in what form.
    pub record_paths: PathRecording,
}

/// The path a guard record should carry, or `None` when the configuration says
/// to record none.
///
/// `rel` is the repo-relative path the guard already computed; `absolute` is the
/// path as the harness supplied it. Returning `Option` rather than an empty
/// string is deliberate: the field is then ABSENT from the record instead of
/// present-and-blank, so a reader can tell "recording is off" from "the path was
/// empty" without consulting the host's config.
#[must_use]
pub fn record_path(mode: PathRecording, rel: &str, absolute: &Path) -> Option<String> {
    match mode {
        PathRecording::Off => None,
        PathRecording::Relative => Some(rel.to_string()),
        PathRecording::Absolute => Some(absolute.display().to_string()),
    }
}

/// A stable, non-reversible key for the file a governed record is about
/// (aegis-x894x2, ruled by sattler 2026-09-05).
///
/// ## Why this exists alongside `record_path`
///
/// Adjudicating a block means asking "was this really a violation in THAT
/// file". `matched` (aegis-mqnl) made the block adjudicable by saying what
/// tripped; this makes it adjudicable by saying WHERE, without the thing that
/// stopped the path being recorded in the first place.
///
/// [`record_path`] is gated on `metrics.record_paths`, which is a DEPLOYMENT'S
/// OPT-IN. The tempting fix was to widen that knob "only for block rows" — but
/// an opt-in a deployment deliberately left off must stay off underneath every
/// row, and turning it on beneath one is exactly the move the knob exists to
/// prevent. So the plain path stays behind it and this goes beside it.
///
/// A digest keys adjudication 1:1 to a file without EXPORTING the path, which
/// is what the opt-in protects: the adjudicator recomputes the same digest over
/// the repo tree and matches, while a record that escapes its host carries no
/// filename. That is why it is safe to record unconditionally, and why it is
/// not a quiet way to record paths anyway.
///
/// ## Contract
///
/// Full SHA-256 hex over the REPO-RELATIVE path exactly as the guard computed
/// it — not truncated, and not normalised. Truncation would trade away the 1:1
/// property this is for, and any normalisation would have to be reproduced
/// exactly by the adjudicator to match. One rule: hash the `rel` string.
#[must_use]
pub fn path_digest(rel: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(rel.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_is_the_default_and_records_nothing() {
        assert_eq!(PathRecording::default(), PathRecording::Off);
        assert_eq!(
            record_path(PathRecording::Off, "src/a.rs", Path::new("/repo/src/a.rs")),
            None,
            "the default must reproduce the pre-#77 record exactly"
        );
    }

    #[test]
    fn relative_and_absolute_record_their_own_form() {
        let abs = Path::new("/home/agent/repo/src/a.rs");
        assert_eq!(
            record_path(PathRecording::Relative, "src/a.rs", abs),
            Some("src/a.rs".to_string())
        );
        assert_eq!(
            record_path(PathRecording::Absolute, "src/a.rs", abs),
            Some("/home/agent/repo/src/a.rs".to_string())
        );
    }

    /// The digest is recorded UNCONDITIONALLY, so it must not depend on the
    /// knob that gates the plain path — that independence is the whole point of
    /// the ruling, and a regression here would silently re-couple them.
    #[test]
    fn the_digest_does_not_depend_on_the_path_recording_knob() {
        let with_off = path_digest("src/a.rs");
        assert_eq!(
            record_path(PathRecording::Off, "src/a.rs", Path::new("/repo/src/a.rs")),
            None,
            "the plain path stays behind the opt-in"
        );
        assert_eq!(
            with_off,
            path_digest("src/a.rs"),
            "and the digest is the same either way"
        );
        assert_eq!(with_off.len(), 64, "full sha256 hex, never truncated");
        assert!(with_off.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// Different files must not share a key, or adjudication cannot be 1:1 —
    /// and the digest must not leak the path it stands for.
    #[test]
    fn distinct_paths_get_distinct_digests_and_none_carry_the_path() {
        let a = path_digest("src/a.rs");
        let b = path_digest("src/b.rs");
        assert_ne!(a, b);
        assert!(
            !a.contains("src") && !a.contains(".rs"),
            "a digest that carried the path would defeat its own purpose"
        );
    }

    /// The adjudicator recomputes this over the repo tree, so it is pinned to a
    /// known-good vector rather than only to itself. `sha256("src/a.rs")`.
    #[test]
    fn the_digest_is_plain_sha256_of_the_relative_path() {
        assert_eq!(
            path_digest("src/a.rs"),
            "bdfc5619650b795d1ffec5e8af3154a9a4b1833e5ed458694907d33709d12825",
            "vector taken from `printf 'src/a.rs' | sha256sum`, NOT from this \
             function — a self-generated expectation would pin the bug too"
        );
    }

    #[test]
    fn record_paths_parses_from_config_toml() {
        // The knob is reachable from a config file, not just constructible in
        // Rust — a setting that only exists in the type system is a setting no
        // deployment can turn on.
        let cfg: MetricsConfig = toml::from_str("record_paths = \"relative\"").unwrap();
        assert_eq!(cfg.record_paths, PathRecording::Relative);
        let empty: MetricsConfig = toml::from_str("").unwrap();
        assert_eq!(empty.record_paths, PathRecording::Off, "absent = off");
    }
}
