//! The TEXT plane's exposure decision: does a block-tier identifier hit block?
//!
//! Extracted from `rule_planes` (yupana #66 review) rather than raising that
//! file's frozen size baseline — the ratchet's contract is that a listed file
//! may shrink and never grow, and "the change is big enough to need more room"
//! is exactly the argument the ratchet exists to refuse.
//!
//! It is a natural seam on its own terms: this is the one place that turns an
//! exposure verdict plus a rule tier into block-or-warn, and it is where the
//! aegis-8tumi4 measured/unmeasured distinction lives.

/// The text plane's decision, PURE: which messages, and whether anything
/// blocks, given the violations and the repo's exposure. Separated from
/// [`governed_check`] so the tier x exposure matrix — the part of this whole
/// circuit that must never be wrong in the blocking direction — is testable
/// without a network.
///
/// The matrix (mqnl's seam, verbatim):
///   block tier + Public   -> BLOCKS (this is the leak the rule exists for)
///   block tier + Internal -> warning, downgraded and saying why
///   block tier + Unknown  -> warning that SAYS the repo is unknown — never
///                            block on a guess, never be silent on ignorance
///   warn tier  + anything -> warning
#[cfg(feature = "quipu")]
pub(super) fn text_plane(
    violations: &[crate::textrules::TextViolation],
    exposure: &crate::project::RepoExposure,
    exposure_source: &str,
) -> (Vec<String>, bool) {
    use crate::project::RepoExposure;
    use crate::textrules::TextTier;

    // A CHECK THAT CANNOT MEASURE MUST NOT REPORT SAFE (aegis-8tumi4 item 3,
    // ruled 2026-09-11).
    //
    // `Unknown` has two causes and they are opposites. Quipu ANSWERED "no such
    // repo" — we measured, and the honest result is ignorance about a repo
    // nobody has pinned; blocking on that is blocking on a guess, and it stays
    // a warning. Or the lookup FAILED with nothing cached to fall back on — we
    // did not measure at all, so "not public" is not a finding, it is the
    // absence of one. The old code could not tell these apart, so an edit into
    // a genuinely public repo sailed through whenever quipu was slow, which at
    // the measured timeout rate was ~4 in 10 of the lookups that needed the
    // network.
    //
    // The MIRROR of the existing rule, deliberately: the fleet already holds
    // `a-check-that-cannot-measure-must-not-report-broken`. This is the other
    // direction, and it is the one that costs a leak rather than a false alarm.
    //
    // ADVISE MODE IS UNAFFECTED AND NEEDS NO BRANCH HERE: `governed_check`
    // gates the returned flag behind `Mode::Enforce`, so advise keeps warning —
    // loudly, because the message below is emitted either way.
    let unmeasured = matches!(exposure, RepoExposure::Unknown(_))
        && exposure_source == crate::project_exposure::SOURCE_UNREACHABLE;

    let mut messages: Vec<String> = Vec::new();
    let mut any_blocking = false;
    for v in violations {
        if v.tier == TextTier::Block && (*exposure == RepoExposure::Public || unmeasured) {
            any_blocking = true;
        }
        messages.push(v.message.clone());
    }
    match exposure {
        RepoExposure::Public => messages.push(
            "[exposure: this repo has a PUBLIC remote (per the graph), so \
             block-tier rules block]"
                .to_string(),
        ),
        RepoExposure::Internal => messages.push(
            "[exposure: downgraded to a warning — the graph knows this repo and \
             it has no public remote, so the token leaks nowhere; the same edit \
             would BLOCK in a public repo]"
                .to_string(),
        ),
        // The lookup FAILED and no last-known verdict could stand in for it.
        RepoExposure::Unknown(reason) if unmeasured => messages.push(format!(
            "[exposure: REFUSING — the repo's exposure could not be MEASURED \
             ({reason}). This is not the same as a repo the graph does not \
             know: nothing was measured, so \"not public\" is the absence of a \
             finding rather than one, and a block-tier rule must not pass on \
             it. Retry when quipu answers, or pin this repo's `repo_<name>` \
             entity and remote facts in the graph.]"
        )),
        // Quipu ANSWERED, and the answer was that it has never heard of this
        // repo. We measured; blocking here really would be blocking on a guess.
        RepoExposure::Unknown(reason) => messages.push(format!(
            "[exposure: warning, NOT blocking — the repo's exposure is unknown \
             ({reason}), and a governed rule never blocks on a guess. Add this \
             repo's `repo_<name>` entity and remote facts to quipu to pin it.]"
        )),
    }
    (messages, any_blocking)
}
