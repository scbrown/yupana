use super::ProjectionRegistry;
use crate::errors::{Error, Result};
use crate::types::Freshness;

impl ProjectionRegistry {
    /// Refresh from Quipu and persist for short-lived hooks. Unlike
    /// `refresh_or_cached`, this always contacts Quipu for scheduled use.
    pub fn refresh_and_persist(&mut self, cache_path: &std::path::Path, now: u64) -> Result<()> {
        self.refresh()?;
        crate::projection_cache::save(cache_path, &self.cached_projection(now));
        let written = crate::projection_cache::load_servable(cache_path, &self.endpoint, 0, now)
            .map_err(|e| {
                Error::Projection(format!(
                    "projection refreshed but cache was not persisted: {e}"
                ))
            })?;
        if written.written_at != now {
            return Err(Error::Projection(format!(
                "projection refreshed but cache timestamp is {} instead of {now}",
                written.written_at
            )));
        }
        Ok(())
    }

    pub(super) fn cached_projection(
        &self,
        written_at: u64,
    ) -> crate::projection_cache::CachedProjection {
        crate::projection_cache::CachedProjection {
            version: crate::projection_cache::CACHE_VERSION,
            written_at,
            endpoint: self.endpoint.clone(),
            policies: self.policies.clone(),
            text_rules: self.text_rules.clone(),
            tripwires: self.tripwires.clone(),
            memory_policies: self.memory_policies.clone(),
            grounded_rules: self.grounded_rules.clone(),
            grounding: self.grounding.clone(),
            work_item_scopes: self.work_item_scopes.clone(),
            work_item_parents: self.work_item_parents.clone(),
        }
    }

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
