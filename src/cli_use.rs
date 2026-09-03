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
    })
}
