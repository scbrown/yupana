use super::*;

/// FR-3's tier-on-every-envelope invariant: `PromoteResponse` was the one
/// served response without a `tier`. A write outcome still names the
/// precision of the facts it moved.
#[test]
fn promote_response_carries_tier() {
    let v = serde_json::to_value(PromoteResponse {
        wrote: true,
        count: Some(1),
        tx_id: Some(1),
        chunks: Some(1),
        violations: Vec::new(),
        tier: crate::types::Tier::TreeSitter.as_str().to_string(),
    })
    .unwrap();
    assert_eq!(
        v["tier"], "treesitter",
        "PromoteResponse lost its tier: {v}"
    );
}
