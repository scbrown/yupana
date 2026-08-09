//! Signing and spooling the guard's verdicts (`quipu` feature).
//!
//! Lifted out of `pre_edit` for size. The seam is real: `pre_edit` decides, this
//! records, and the recording rides strictly behind the decision — it runs after
//! the outcome is already fixed and cannot change it.

use std::path::Path;

use super::decision::Decision;
use super::{introduced_text, relative, HookInput};
use crate::config::YupanaConfig;

/// Sign and spool the decision's verdicts, if a signing key already exists.
///
/// Silent on every absence — no config, no key, no target, nothing evaluated.
/// The one thing it will not do is CREATE the key: `yupana verifier` is the
/// deliberate act that does that.
pub(super) fn spool_verdicts(
    decision: &Decision,
    config: Option<&YupanaConfig>,
    root: &Path,
    file_path: Option<&str>,
    input: Option<&HookInput>,
) {
    if decision.constraints.is_empty() {
        return;
    }
    let (Some(config), Some(file_path)) = (config, file_path) else {
        return;
    };
    if !config.quipu.enabled {
        return;
    }
    let key_path = root.join(&config.quipu.signing_key_path);
    let Some(key) = crate::verdict_spool::existing_key(&key_path) else {
        return;
    };
    let rel = relative(Path::new(file_path), root);
    // The evidence is the text the predicates ACTUALLY SAW — the introduced
    // text, not the whole file — so the verdict's hash binds to what was judged.
    // Change it and the stored verdict self-stales; hash the wrong thing and the
    // binding attests to something nobody evaluated.
    let Some(evidence) = input.and_then(introduced_text) else {
        return;
    };
    let _ = crate::verdict_spool::record(
        &key,
        &decision.constraints,
        &rel,
        &evidence,
        decision.freshness,
    );
}
