//! Host-memory arm of the command guard.
//!
//! Policy is projected from Quipu; evidence is read locally from
//! `/proc/meminfo`. Both failure directions are loud fail-open. The deployment
//! mode remains the ceiling, so a new hard policy reports under `advise` and
//! cannot deny until the measured soak promotes the mode.

#[cfg(feature = "quipu")]
use super::{first_notice_for_session, HookInput};
use crate::hook::pre_edit::Outcome;
#[cfg(feature = "quipu")]
use crate::policy::Mode;

#[cfg(feature = "quipu")]
const MEMINFO: &str = "/proc/meminfo";

#[cfg(any(feature = "quipu", test))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct Reading {
    available_gib: f64,
    total_gib: f64,
}

#[cfg(any(feature = "quipu", test))]
fn parse_meminfo(text: &str) -> Result<Reading, String> {
    let mut available_kib = None;
    let mut total_kib = None;
    for line in text.lines() {
        let (key, rest) = line.split_once(':').unwrap_or(("", ""));
        let value = rest
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<f64>().ok());
        match key {
            "MemAvailable" => available_kib = value,
            "MemTotal" => total_kib = value,
            _ => {}
        }
    }
    let available_kib = available_kib.ok_or("no MemAvailable")?;
    let total_kib = total_kib.ok_or("no MemTotal")?;
    const KIB_PER_GIB: f64 = 1024.0 * 1024.0;
    Ok(Reading {
        available_gib: available_kib / KIB_PER_GIB,
        total_gib: total_kib / KIB_PER_GIB,
    })
}

#[cfg(feature = "quipu")]
fn read_meminfo() -> Result<Reading, String> {
    let text =
        std::fs::read_to_string(MEMINFO).map_err(|e| format!("cannot read {MEMINFO}: {e}"))?;
    parse_meminfo(&text).map_err(|e| format!("cannot interpret {MEMINFO}: {e}"))
}

#[cfg(feature = "quipu")]
pub(super) fn check(payload: &str, command: &str) -> Outcome {
    // This is only a cheap routing superset, never the policy decision. The
    // graph regex remains authoritative, but an unrelated shell command must
    // not pay for (or receive failures from) a remote projection.
    if !might_be_memory_heavy(command) {
        return Outcome::Allow;
    }
    let Some(input) = HookInput::parse(payload) else {
        return Outcome::Allow;
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let root = input.root(&cwd);
    let config = match crate::config::YupanaConfig::resolve(None, &root) {
        Ok(config) => config,
        Err(e) => {
            return notify_once(
                input.session_id.as_deref(),
                "memory-config",
                &format!("memory policy was NOT EVALUATED: unreadable config ({e})"),
            )
        }
    };
    if config.policy.mode == Mode::Off || !config.quipu.enabled || config.quipu.endpoint.is_empty()
    {
        return Outcome::Allow;
    }

    let mut registry = crate::project::ProjectionRegistry::new(&config.quipu.endpoint);
    let source = match registry.refresh_or_cached(
        crate::projection_cache::cache_path().as_deref(),
        config.quipu.projection_cache_ttl_secs,
        crate::projection_cache::now_secs(),
    ) {
        Ok(source) => source,
        Err(e) => {
            return notify_once(
                input.session_id.as_deref(),
                "memory-projection",
                &format!(
                    "memory policy was NOT EVALUATED: governed policy projection failed ({e}). \
                     The command is allowed, loudly fail-open."
                ),
            )
        }
    };
    if !registry
        .memory_policies()
        .iter()
        .any(|p| p.matches(command))
    {
        return Outcome::Allow;
    }
    let reading = match read_meminfo() {
        Ok(reading) => reading,
        Err(e) => {
            return notify_once(
                input.session_id.as_deref(),
                "memory-signal",
                &format!(
                    "memory policy SIGNAL LOST: {e}. The command is allowed UNGOVERNED by \
                     memory; the guard fails open rather than inventing a host-wide outage."
                ),
            )
        }
    };
    evaluate(
        registry.memory_policies(),
        command,
        reading,
        config.policy.mode,
        matches!(source, crate::project::ProjectionSource::Cache { .. }),
    )
}

#[cfg(feature = "quipu")]
fn might_be_memory_heavy(command: &str) -> bool {
    ["cargo", "rustc", "cc", "gcc", "clang", "ld", "caboodle"]
        .iter()
        .any(|word| {
            command
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|part| part == *word)
        })
}

#[cfg(not(feature = "quipu"))]
pub(super) fn check(_payload: &str, _command: &str) -> Outcome {
    Outcome::Allow
}

#[cfg(feature = "quipu")]
fn evaluate(
    policies: &[crate::project_memory::MemoryPolicy],
    command: &str,
    reading: Reading,
    mode: Mode,
    cached: bool,
) -> Outcome {
    let mut violations = Vec::new();
    let mut blocks = false;
    for policy in policies.iter().filter(|p| p.matches(command)) {
        if reading.total_gib < policy.threshold_gib {
            return Outcome::Notify(format!(
                "yupana: memory policy `{}` is not applicable: its {:.1} GiB threshold exceeds \
                 this host's {:.1} GiB total. The command is allowed; correct the graph data.",
                policy.label, policy.threshold_gib, reading.total_gib
            ));
        }
        if reading.available_gib >= policy.threshold_gib {
            continue;
        }
        let policy_blocks = policy.blocks(mode);
        blocks |= policy_blocks;
        violations.push(format!(
            "memory policy `{}`: {:.1} GiB available of {:.1} GiB is below the governed {:.1} \
             GiB floor for this memory-heavy command; wait for a build to finish{}",
            policy.label,
            reading.available_gib,
            reading.total_gib,
            policy.threshold_gib,
            if cached {
                " (policy served from stale cache)"
            } else {
                ""
            }
        ));
        crate::metrics::emit(
            "memory_policy",
            &[
                ("policy", policy.id.clone().into()),
                ("available_gib", reading.available_gib.into()),
                ("total_gib", reading.total_gib.into()),
                ("threshold_gib", policy.threshold_gib.into()),
                (
                    "result",
                    if policy_blocks { "deny" } else { "advise" }.into(),
                ),
                (
                    "policy_source",
                    if cached { "cache" } else { "live" }.into(),
                ),
            ],
        );
    }
    if violations.is_empty() {
        Outcome::Allow
    } else if blocks {
        Outcome::Deny(violations.join("\n"))
    } else {
        Outcome::Notify(format!(
            "yupana (governed, not blocking): {}",
            violations.join("\n")
        ))
    }
}

#[cfg(feature = "quipu")]
fn notify_once(session: Option<&str>, kind: &str, message: &str) -> Outcome {
    eprintln!("yupana: {message}");
    if first_notice_for_session(session, kind) {
        Outcome::Notify(format!("yupana: {message}"))
    } else {
        Outcome::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_real_kib_units() {
        let got = parse_meminfo("MemTotal: 64592284 kB\nMemAvailable: 16777216 kB\n").unwrap();
        assert!((got.available_gib - 16.0).abs() < 0.001);
        assert!(got.total_gib > 61.0);
    }

    #[test]
    fn missing_memavailable_is_not_guessed_from_memfree() {
        assert!(parse_meminfo("MemTotal: 64592284 kB\nMemFree: 100 kB\n").is_err());
    }

    #[cfg(feature = "quipu")]
    #[test]
    fn unrelated_commands_skip_remote_policy_projection() {
        assert!(!might_be_memory_heavy("git status --short"));
        assert!(might_be_memory_heavy("nice -n 10 cargo test"));
        assert!(might_be_memory_heavy("/usr/bin/rustc --crate-name demo"));
    }

    #[cfg(feature = "quipu")]
    fn policy() -> crate::project_memory::MemoryPolicy {
        crate::project_memory::MemoryPolicy {
            id: "memory-policy".into(),
            label: "Rust build headroom".into(),
            command_regex: "(^|[;&|[:space:]])cargo[[:space:]]+(build|test)".into(),
            threshold_gib: 24.0,
            effect: "deny".into(),
            class: crate::constraint::ConstraintClass::Hard,
            verification_point: crate::constraint::VerificationPoint::Pag,
            rationale: None,
        }
    }

    #[cfg(feature = "quipu")]
    #[test]
    fn low_memory_advises_before_it_can_deny() {
        let got = evaluate(
            &[policy()],
            "cargo build --release",
            Reading {
                available_gib: 16.0,
                total_gib: 61.6,
            },
            Mode::Advise,
            false,
        );
        assert!(
            matches!(got, Outcome::Notify(ref m) if m.contains("16.0 GiB") && m.contains("24.0 GiB"))
        );
    }

    #[cfg(feature = "quipu")]
    #[test]
    fn enforce_is_a_separate_promotion() {
        let got = evaluate(
            &[policy()],
            "cargo test",
            Reading {
                available_gib: 8.0,
                total_gib: 61.6,
            },
            Mode::Enforce,
            false,
        );
        assert!(matches!(got, Outcome::Deny(_)));
    }

    #[cfg(feature = "quipu")]
    #[test]
    fn ordinary_commands_pass_untouched() {
        assert_eq!(
            evaluate(
                &[policy()],
                "git status",
                Reading {
                    available_gib: 1.0,
                    total_gib: 61.6
                },
                Mode::Enforce,
                false,
            ),
            Outcome::Allow
        );
    }
}
