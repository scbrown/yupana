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
