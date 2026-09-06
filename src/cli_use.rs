//! The deliberate-use metric arm — lifted out of `cli` for size (the 500-line
//! limit), a child module reaching the private `Commands` through `super`.

use super::Commands;

/// The metric name for a DELIBERATELY-invoked command, or None for the two
/// invocations that are not "use": the hook (the guard spools its own line)
/// and shell completions. Exhaustive on purpose — a new command must decide
/// its own answer here or fail to compile, so the leverage metric can never
/// silently under-count a surface that grew.
pub(super) fn deliberate_use_name(cmd: &Commands) -> Option<&'static str> {
    Some(match cmd {
        Commands::Hook { .. } | Commands::Completions { .. } => return None,
        Commands::Serve { .. } => "serve",
        Commands::Daemon { .. } => "daemon",
        Commands::Analyze { .. } => "analyze",
        Commands::Refs { .. } => "refs",
        Commands::Watch { .. } => "watch",
        Commands::Status => "status",
        #[cfg(feature = "quipu")]
        Commands::RefreshProjection => "refresh-projection",
        Commands::Callers { .. } => "callers",
        Commands::Communities { .. } => "communities",
        Commands::Impact { .. } => "impact",
        Commands::Dataflow { .. } => "dataflow",
        Commands::Verify { .. } => "verify",
        Commands::Exemplar { .. } => "exemplar",
        Commands::Changed { .. } => "changed",
        Commands::Census { .. } => "census",
        Commands::Export { .. } => "export",
        Commands::Promote { .. } => "promote",
        #[cfg(feature = "quipu")]
        Commands::Verifier { .. } => "verifier",
        #[cfg(feature = "quipu")]
        Commands::Verdicts { .. } => "verdicts",
        #[cfg(feature = "quipu")]
        Commands::Certify(_) => "certify",
        #[cfg(feature = "quipu")]
        Commands::Share(_) => "share",
    })
}

/// Declare this process's quipu caller label, once, before dispatch.
///
/// Called from `run()` rather than folded into each command, because
/// [`crate::quipu_label::set`] is FIRST-write-wins: the label has to be in place
/// before anything issues a request, and a command that sets it on its own way
/// out would lose to whatever asked first.
///
/// A no-op without the `quipu` feature, so the call site needs no `cfg` and
/// cannot drift out of sync with the feature gate.
#[cfg(feature = "quipu")]
pub(super) fn declare_quipu_caller(cmd: &Commands) {
    if let Some(label) = quipu_caller_kind(cmd) {
        crate::quipu_label::set(label);
    }
}

/// See the `quipu`-gated twin above: without a quipu client there is nothing to
/// attribute.
#[cfg(not(feature = "quipu"))]
pub(super) fn declare_quipu_caller(_cmd: &Commands) {}

/// This PROCESS's quipu caller label, or `None` to leave the default in place.
///
/// PER VERB (`yupana-cli:<verb>`), not one flat `yupana-cli`. The flat form
/// would have answered the aegis-tjyhh4 question — "is it the hook or a CLI?" —
/// and then immediately failed the next one, which is the one an operator
/// actually acts on: WHICH CLI. Measured 2026-09-06, the two quipu-touching
/// verbs are nothing alike (`status` 19-21 `/query` per run; the twice-hourly
/// `refresh-projection` 6, two of them hitting the 30 s ceiling), so a bucket
/// holding both cannot tell an operator which one to fix.
///
/// ON THE SERVER-SIDE CAP. quipu caps distinct client labels and folds the
/// overflow into `other`, so a per-verb scheme has to be bounded. It is, and not
/// by this match: a label is only ever SENT by a verb that actually issues a
/// quipu request, and most of the command surface (`impact`, `callers`, `refs`,
/// `analyze`…) never touches the store. The exhaustive match below decides what
/// a verb WOULD be called; the store sees only the handful that call.
///
/// TWO VARIANTS DELIBERATELY ANSWER `None`, and both are load-bearing:
///
/// * `Hook` — the default already IS [`crate::quipu_label::HOOK`], and it is the
///   default precisely because it is the overwhelmingly common process.
/// * `Daemon` — the daemon labels itself [`crate::quipu_label::DAEMON`] when its
///   refresher starts. [`crate::quipu_label::set`] is FIRST-write-wins, so
///   setting a CLI label here would win the race against that call and file
///   every daemon refresh under a CLI. That inverts the aegis-x894x2
///   measurement: the daemon would look like it was generating the load it
///   exists to remove.
///
/// Gated with the client it labels: without the `quipu` feature there is no
/// quipu request to attribute, so a label would name a caller that never calls.
#[cfg(feature = "quipu")]
pub(super) fn quipu_caller_kind(cmd: &Commands) -> Option<&'static str> {
    Some(match cmd {
        Commands::Hook { .. } | Commands::Daemon { .. } => return None,
        // The resident MCP server is not a one-shot verb: long-lived, answering
        // tool calls, a different load shape and a different owner. Folding it
        // under `cli:` would repeat this bead's own mistake one level down.
        Commands::Serve { .. } => crate::quipu_label::MCP,
        Commands::Completions { .. } => "yupana-cli:completions",
        Commands::Analyze { .. } => "yupana-cli:analyze",
        Commands::Refs { .. } => "yupana-cli:refs",
        Commands::Watch { .. } => "yupana-cli:watch",
        Commands::Status => "yupana-cli:status",
        #[cfg(feature = "quipu")]
        Commands::RefreshProjection => "yupana-cli:refresh-projection",
        Commands::Callers { .. } => "yupana-cli:callers",
        Commands::Communities { .. } => "yupana-cli:communities",
        Commands::Impact { .. } => "yupana-cli:impact",
        Commands::Dataflow { .. } => "yupana-cli:dataflow",
        Commands::Verify { .. } => "yupana-cli:verify",
        Commands::Exemplar { .. } => "yupana-cli:exemplar",
        Commands::Changed { .. } => "yupana-cli:changed",
        Commands::Census { .. } => "yupana-cli:census",
        Commands::Export { .. } => "yupana-cli:export",
        Commands::Promote { .. } => "yupana-cli:promote",
        #[cfg(feature = "quipu")]
        Commands::Verifier { .. } => "yupana-cli:verifier",
        #[cfg(feature = "quipu")]
        Commands::Verdicts { .. } => "yupana-cli:verdicts",
        #[cfg(feature = "quipu")]
        Commands::Certify(_) => "yupana-cli:certify",
        #[cfg(feature = "quipu")]
        Commands::Share(_) => "yupana-cli:share",
    })
}

#[cfg(all(test, feature = "quipu"))]
mod caller_kind_tests {
    // Test names shout the invariant they turn on — the emphasis used in
    // `post_bash_test` and `daemon::tests`. Scoped to tests.
    #![allow(non_snake_case)]

    use super::*;

    /// The two abstentions. This is the arm that matters: `Daemon` answering
    /// anything but `None` would label the daemon's refresher as a CLI, because
    /// `quipu_label::set` is first-write-wins and `run()` fires before the
    /// refresher does.
    #[test]
    fn the_hook_and_the_daemon_keep_their_own_labels() {
        // Every hook event, not one: a classifier that matched a single
        // variant would leave the others reaching for the CLI arm.
        for event in [
            crate::cli::HookEvent::PreEdit,
            crate::cli::HookEvent::PostEdit,
            crate::cli::HookEvent::PreBash,
            crate::cli::HookEvent::PostBash,
        ] {
            assert_eq!(quipu_caller_kind(&Commands::Hook { event }), None);
        }
        assert_eq!(quipu_caller_kind(&Commands::Daemon { port: None }), None);
    }

    /// The other direction, so the abstentions above mean "declined" rather than
    /// "this classifier returns None for everything".
    #[test]
    fn a_one_shot_verb_is_labelled_WITH_ITS_VERB() {
        assert_eq!(
            quipu_caller_kind(&Commands::Status),
            Some("yupana-cli:status")
        );
        assert_eq!(
            quipu_caller_kind(&Commands::Serve { http: false }),
            Some(crate::quipu_label::MCP)
        );
    }

    /// The two quipu-touching verbs measured in aegis-tjyhh4 must land in
    /// DIFFERENT buckets — that is the entire point of going per-verb, and a
    /// flat `yupana-cli` would pass every other assertion here.
    #[cfg(feature = "quipu")]
    #[test]
    fn status_and_refresh_projection_are_not_the_same_bucket() {
        let status = quipu_caller_kind(&Commands::Status);
        let refresh = quipu_caller_kind(&Commands::RefreshProjection);
        assert_eq!(refresh, Some("yupana-cli:refresh-projection"));
        assert_ne!(
            status, refresh,
            "the twice-hourly cron and an interactive status must be separable"
        );
    }

    /// Every label this classifier can produce is attributable to yupana and
    /// carries the `cli:` namespace, so a store-side reader can group them.
    #[test]
    fn every_cli_label_is_namespaced() {
        for cmd in [
            Commands::Status,
            Commands::Analyze {
                path: std::path::PathBuf::from("."),
                at: None,
            },
        ] {
            let label = quipu_caller_kind(&cmd).expect("a verb is labelled");
            assert!(
                label.starts_with("yupana-cli:"),
                "{label} is not namespaced"
            );
        }
    }
}
