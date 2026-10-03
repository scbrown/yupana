//! Evidence required before synthesizing an unresolved landing.
use super::resolve_segment;

// An unknown program is not a landing. Require a literal landing-shaped argv
// suffix before widening uncertainty to the current repository's protected ref.
pub(super) fn unresolved_landing_evidence(words: &[String]) -> bool {
    // A dynamic executable may be an editor/interpreter, even if its data
    // arguments happen to spell git and push. Only known execution wrappers
    // provide a reason to inspect nested argv as executable evidence.
    if !words.first().is_some_and(|word| {
        matches!(
            word.rsplit('/').next(),
            Some("env" | "sudo" | "command" | "timeout" | "xargs" | "bash" | "sh" | "eval")
        )
    }) {
        return false;
    }
    words.iter().enumerate().any(|(at, word)| {
        let suffix: Vec<&str> = words[at..].iter().map(String::as_str).collect();
        if resolve_segment("", &suffix).is_some() {
            return true;
        }
        // GNU env's inline split-string operand contains executable argv, unlike
        // arbitrary quoted prose passed to an editor or interpreter.
        if words
            .first()
            .is_some_and(|p| p.rsplit('/').next() == Some("env"))
        {
            if let Some(code) = word.strip_prefix("--split-string=") {
                if let Ok(decoded) = shell_words::split(code) {
                    let argv: Vec<&str> = decoded.iter().map(String::as_str).collect();
                    return resolve_segment("", &argv).is_some();
                }
            }
        }
        false
    })
}
