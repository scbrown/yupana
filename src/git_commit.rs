//! Git author time and distinct committer identity.

use super::git;
use std::path::Path;

/// A commit's author identity and authored time (strict ISO-8601), or `None`
/// when the ref does not resolve.
///
/// AUTHOR date, not committer date: it is when the change was MADE, which is
/// §9.3's valid-time notion. A rebase rewrites the committer date and leaves the
/// author date alone, so the author date is the one that survives history being
/// replayed — and a provenance fact that moved because someone rebased would be
/// a fact about the rebase, not about the work.
#[must_use]
pub fn commit_identity(root: &Path, reference: &str) -> Option<(String, String)> {
    let out = git(root, &["log", "-1", "--format=%an <%ae>%n%aI", reference])?;
    let mut lines = out.lines();
    let author = lines.next()?.trim().to_string();
    let date = lines.next()?.trim().to_string();
    (!date.is_empty()).then_some((author, date))
}

/// The committer identity, distinct from the author and from the promotion writer.
#[must_use]
pub fn commit_committer(root: &Path, reference: &str) -> Option<String> {
    git(root, &["log", "-1", "--format=%cn <%ce>", reference])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
