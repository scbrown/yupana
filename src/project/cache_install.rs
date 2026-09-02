use super::ProjectionRegistry;
use crate::types::Freshness;

impl ProjectionRegistry {
    pub(super) fn install_cached(
        &mut self,
        cached: crate::projection_cache::CachedProjection,
        freshness: Freshness,
    ) {
        self.policies = cached.policies;
        self.text_rules = cached.text_rules;
        self.tripwires = cached.tripwires;
        self.memory_policies = cached.memory_policies;
        self.grounded_rules = cached.grounded_rules;
        self.grounding = cached.grounding;
        self.work_item_scopes = cached.work_item_scopes;
        self.work_item_parents = cached.work_item_parents;
        self.freshness = freshness;
    }
}
