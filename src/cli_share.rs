//! The `yupana share` arm: resolving which quipu endpoint a share subcommand
//! may talk to.
//!
//! A child module of `cli` purely for size (the 500-line limit), reaching
//! `super::Cli` the way `cli_serve` does.

use std::path::PathBuf;

use crate::config::YupanaConfig;

impl super::Cli {
    /// The configured `[yupana.quipu] endpoint`, if there is a non-empty one.
    ///
    /// **Only READS may default to it.** The key is set host-wide so every
    /// agent's pre-edit guard can fetch the rule catalogue, so treating its
    /// presence as authorization to WRITE would silently promote a read
    /// credential into a write one. That is the reasoning `yupana promote`
    /// already states for requiring `--to`, and `share pull` / `share promote`
    /// inherit it: they take `--to` as a required argument and never reach
    /// this function. `share policy` is a read and does.
    pub(super) fn quipu_endpoint(&self) -> Option<String> {
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let config = YupanaConfig::resolve(self.config.as_deref(), &root).unwrap_or_default();
        Some(config.quipu.endpoint).filter(|e| !e.is_empty())
    }
}
