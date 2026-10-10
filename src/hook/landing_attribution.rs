//! Ref attribution is evidence, not the conservative target used by the guard.
//!
//! An unstated push can use upstreams, push refspecs, or push.default. The local
//! branch alone does not prove its destination; a failed lookup proves even
//! less. Keep evaluating policy as before, but never sign that guess as main.

use crate::landing::{Landing, LandingVerb, RefTarget};

pub(super) fn recording_ref<'a>(
    landing: &Landing,
    evaluated_ref: &'a str,
) -> (&'a str, &'static str) {
    match (&landing.verb, &landing.git_ref) {
        (LandingVerb::Push, RefTarget::Unstated) => ("UNKNOWN", "unknown"),
        (_, RefTarget::Named(_)) => (evaluated_ref, "command"),
        _ => (evaluated_ref, "assumed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unstated_push_never_signs_the_guard_fallback_as_main() {
        for command in ["git push", "git push origin"] {
            let landing = crate::landing::resolve(command).unwrap();
            for evaluated in ["main", "wt/fixture"] {
                assert_eq!(recording_ref(&landing, evaluated), ("UNKNOWN", "unknown"));
            }
        }
    }

    #[test]
    fn explicit_destination_and_merge_assumption_stay_distinct() {
        let push = crate::landing::resolve("git push origin wt/fixture:main").unwrap();
        assert_eq!(recording_ref(&push, "main"), ("main", "command"));
        let topic = crate::landing::resolve("git push origin wt/fixture").unwrap();
        assert_eq!(
            recording_ref(&topic, "wt/fixture"),
            ("wt/fixture", "command")
        );
        let merge = crate::landing::resolve("gh pr merge 3").unwrap();
        assert_eq!(recording_ref(&merge, "main"), ("main", "assumed"));
    }
}
