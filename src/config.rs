//! Yupana configuration.
//!
//! Yupana shares the stack's `.bobbin/config.toml` under a `[yupana]` table, with
//! the same resolution order Quipu uses: compiled defaults are overlaid by the
//! user config (`~/.config/bobbin/config.toml`) and then the project config
//! (`.bobbin/config.toml`). CLI flags win over all of them (applied by the
//! caller). See `docs/yupana-spec.md` §11.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::errors::{Error, Result};

/// Where the effective policy mode was explicitly set.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyModeProvenance {
    /// The mode used by the guard after configuration layering.
    pub effective: crate::policy::Mode,
    /// The layer that explicitly supplied the effective mode.
    pub source: String,
    /// The user-level mode, when the user explicitly set one.
    pub user_mode: Option<crate::policy::Mode>,
    /// Whether a workspace explicitly lowered the user's mode.
    pub lowered_by_project: bool,
}

/// Top-level Yupana configuration (the `[yupana]` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YupanaConfig {
    /// Baseline ref the shared read-only graph is built at.
    pub base_ref: String,
    /// Run the LSP tier for precise facts where a build resolves.
    pub enable_lsp: bool,
    /// Run the CPG/dataflow tier (Phase 2).
    pub enable_cpg: bool,
    /// Languages to extract (defaults to Bobbin's grammar set).
    pub languages: Vec<String>,
    /// Freshness / debounce settings.
    pub freshness: FreshnessConfig,
    /// Multi-tenancy limits.
    pub tenancy: TenancyConfig,
    /// Serving surface (MCP/HTTP) settings.
    pub serve: ServeConfig,
    /// Quipu promotion settings (Phase 4).
    pub quipu: QuipuConfig,
    /// Capability-scoped edit policy for the pre-edit guard (§5.8/FR-25).
    pub policy: crate::policy::PolicyConfig,
    /// What the usage spool records about a guard decision (yupana #77).
    pub metrics: crate::audit::MetricsConfig,
}

impl Default for YupanaConfig {
    fn default() -> Self {
        Self {
            base_ref: "main".to_string(),
            enable_lsp: true,
            enable_cpg: false,
            languages: ["rust", "typescript", "python", "go", "java", "cpp"]
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            freshness: FreshnessConfig::default(),
            tenancy: TenancyConfig::default(),
            serve: ServeConfig::default(),
            quipu: QuipuConfig::default(),
            policy: crate::policy::PolicyConfig::default(),
            metrics: crate::audit::MetricsConfig::default(),
        }
    }
}

/// Freshness / debounce settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FreshnessConfig {
    /// Debounce for keystroke-driven tree-sitter updates, in milliseconds.
    pub debounce_ms: u64,
    /// Debounce for the deferred heavy tier (graph/frontier recompute, and later
    /// LSP/CPG), in milliseconds. Longer than `debounce_ms` so a burst of edits
    /// does not thrash the expensive recompute (FR-17).
    pub heavy_debounce_ms: u64,
    /// When to compute LSP facts: `"save"` or `"on_demand"`.
    pub lsp_on: String,
}

impl Default for FreshnessConfig {
    fn default() -> Self {
        Self {
            debounce_ms: 300,
            heavy_debounce_ms: 1500,
            lsp_on: "save".to_string(),
        }
    }
}

/// Multi-tenancy limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TenancyConfig {
    /// Maximum concurrent per-tenant overlays over one base.
    pub max_overlays: usize,
    /// Symbols with fan-in above this get special frontier handling.
    pub high_fanin_threshold: usize,
    /// Overlay eviction policy: `"on_session_close"` or `"lru"`.
    pub overlay_eviction: String,
}

impl Default for TenancyConfig {
    fn default() -> Self {
        Self {
            max_overlays: 32,
            high_fanin_threshold: 200,
            overlay_eviction: "on_session_close".to_string(),
        }
    }
}

/// Serving surface settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServeConfig {
    /// Bind address for the HTTP / streamable-HTTP MCP surface.
    pub bind_address: String,
    /// Port for the streamable-HTTP MCP + HTTP API (distinct from Bobbin/Quipu).
    pub mcp_http_port: u16,
    /// Write guard for the broker / promotion endpoints.
    pub read_only: bool,
    /// Whether the pre-edit guard should EXPECT a resident daemon at
    /// `bind_address:mcp_http_port` and use it to size edits (FR-31).
    ///
    /// This flag is what makes "daemon not running" LOUD rather than noisy. When
    /// false (the default, and true everywhere today since no daemon runs), the
    /// guard builds the graph transiently and says nothing — absence is normal. When
    /// true, the guard asks the daemon and, if it cannot, warns ONCE per session that
    /// the resident guard is down while still guarding via a transient rebuild. Only
    /// an operator who has actually started a daemon sets this, so the warning fires
    /// exactly when a daemon was expected and isn't there — the cheapest-bypass case.
    pub use_daemon: bool,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1".to_string(),
            mcp_http_port: 3040,
            read_only: false,
            use_daemon: false,
        }
    }
}

/// Quipu promotion settings (Phase 4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QuipuConfig {
    /// Whether promotion into Quipu is enabled.
    pub enabled: bool,
    /// When to promote: `"commit"`, `"merge"`, or `"manual"`.
    pub promote_on: String,
    /// Branch model for promoted facts: `"qualifier"` (implemented — a
    /// `bobbin:onBranch` term per promoted entity, zero Quipu change) or
    /// `"named_graph"` (preferred by §9.4, blocked on quad support in Quipu,
    /// scbrown/quipu#36, and therefore REFUSED rather than silently degraded).
    /// See `docs/yupana-spec.md` §9.4 and `src/promote_branch.rs`.
    pub branch_model: String,
    /// Directory holding the SHACL shapes promotion validates against.
    pub shapes_path: String,
    /// Base URL of the Quipu to promote into (e.g. `http://localhost:8080`).
    /// Deployment config, NOT a per-call parameter: the graph a promotion writes
    /// into is data identity, not a caller's choice. Empty by default so a
    /// misconfigured deployment refuses rather than guessing a graph. The CLI
    /// `--to` overrides it for one-off promotions.
    pub endpoint: String,
    /// Path to the PKCS#8 verdict-signing key. The pre-edit guard signs with
    /// this key IF IT ALREADY EXISTS and spools the verdict; it never creates
    /// one, because a key materialising as a side effect of an agent's edit is
    /// not something that should happen quietly. `yupana verifier` creates it.
    pub signing_key_path: String,
    /// How old the persisted projection (`crate::projection_cache`) may be and
    /// still be SERVED when quipu cannot be projected live. Past it the guard
    /// fails open loudly instead of enforcing a catalogue nobody has confirmed
    /// since.
    ///
    /// One hour by default, and the number is chosen against the measured
    /// failure rather than picked round. The projection failures this cache
    /// exists for are `/query` TIMEOUTS UNDER CONCURRENCY, not outages: even on
    /// the worst measured day 81% of invocations succeeded, so in practice the
    /// cache a failing edit falls back on is seconds old. An hour is slack for
    /// a genuinely bad patch, not a licence to enforce yesterday's policy — the
    /// failure it bounds is a RETIRED rule that keeps firing from cache, which
    /// is worse than no rule because it is unfalsifiable from the outside.
    ///
    /// `0` disables cache serving: every projection failure fails open, which
    /// is the pre-aegis-0upyu behaviour and is offered as an escape hatch, not
    /// as a recommendation.
    pub projection_cache_ttl_secs: u64,
}

impl Default for QuipuConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            promote_on: "merge".to_string(),
            // The IMPLEMENTED model, not the preferred one. §9.4 names
            // named-graph as preferred and that is still true, but defaulting
            // to a model whose only honest behaviour is a refusal would refuse
            // every promotion out of the box. Flip this back when quipu#36
            // lands.
            branch_model: "qualifier".to_string(),
            shapes_path: "shapes/".to_string(),
            endpoint: String::new(),
            signing_key_path: "yupana-signing.pk8".to_string(),
            projection_cache_ttl_secs: 3600,
        }
    }
}

impl YupanaConfig {
    /// Refuse a mutating operation when `serve.read_only` is set.
    ///
    /// The write guard the docs promise (config.md: "Write guard for the broker /
    /// promotion endpoints"). Before this it was documented, settable, and INERT —
    /// an operator who set `read_only = true` before exposing `yupana serve` to a
    /// broker got no guard and no warning, which is strictly worse than an absent
    /// switch: a safety control that does nothing invites the trust it cannot
    /// honour (aegis-ltjo). Now the one write yupana performs — promotion — calls
    /// this and REFUSES with a distinguishable error naming the key, so the guard
    /// is real and any future served write is one `write_guard` call from being
    /// covered too.
    pub fn write_guard(&self, operation: &str) -> Result<()> {
        if self.serve.read_only {
            return Err(Error::Config(format!(
                "refused: `serve.read_only = true` — this yupana instance is \
                 configured read-only, so {operation} (a write) is refused. \
                 Unset serve.read_only to allow writes."
            )));
        }
        Ok(())
    }

    /// Load the merged configuration for a project rooted at `root`.
    ///
    /// Starts from defaults, overlays the user config if present, then the
    /// project's `.bobbin/config.toml` `[yupana]` table. Missing files are not an
    /// error; a malformed file is.
    ///
    /// "Overlay" is per-key, not per-file. Replacing the whole table would mean
    /// a project config that sets one unrelated key silently discards every
    /// other setting the user config established — and when the discarded
    /// setting is `[yupana.policy]`, the capability guard goes inert while
    /// looking exactly like a clean run. A fleet keeps its scopes in one
    /// user-level file precisely so they cannot drift, so that file has to
    /// survive a workspace defining `base_ref`.
    pub fn load(root: &Path) -> Result<Self> {
        Self::load_layered(user_config_path().as_deref(), root)
    }

    /// Identify the config layer that supplied the effective policy mode.
    ///
    /// This intentionally reads the raw layers as well as the merged result:
    /// the merged `PolicyConfig` can say `off`, but cannot say whether that was
    /// the safe default, an operator choice, or a workspace overriding a more
    /// restrictive user policy.
    pub fn policy_mode_provenance(
        root: &Path,
        effective: crate::policy::Mode,
    ) -> Result<PolicyModeProvenance> {
        let user = user_config_path();
        let project = root.join(".bobbin").join("config.toml");
        policy_mode_provenance_from_paths(user.as_deref(), &project, effective)
    }

    /// Resolve configuration honouring an explicit `--config` override.
    ///
    /// `Some(path)` **replaces** discovery: FR-29 ranks a flag above project and
    /// user config, so the override loads exactly that file over defaults and
    /// the ambient `.bobbin/config.toml` is never consulted. `None` runs the
    /// normal [`load`](Self::load) discovery rooted at `root`.
    ///
    /// A `--config` path that cannot be read is an ERROR, never a silent
    /// fall-back to discovery. That fall-back is the whole defect this override
    /// closes (aegis-ll3p): an operator who points the guard at a scope file and
    /// mistypes the path must get a loud failure, not the ambient scope wearing
    /// the success of the command they meant to scope.
    pub fn resolve(override_path: Option<&Path>, root: &Path) -> Result<Self> {
        match override_path {
            Some(path) => Self::load_from(path),
            None => Self::load(root),
        }
    }

    /// Load configuration from exactly one file, over defaults — no discovery.
    ///
    /// The file must exist: [`read_yupana_table`] returns `None` both for an
    /// absent file and for a present file with no `[yupana]` table, and only the
    /// first is an error, so existence is checked explicitly. A present file
    /// with no `[yupana]` table is a valid (if unusual) request for defaults.
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(Error::Config(format!(
                "--config path does not exist: {}",
                path.display()
            )));
        }
        match read_yupana_table(path)? {
            Some(value) => value
                .try_into()
                .map_err(|e| Error::Config(format!("{}: [yupana]: {e}", path.display()))),
            None => Ok(Self::default()),
        }
    }

    /// The layering itself, with the user-config path injected.
    ///
    /// Taking it as an argument keeps this testable without reassigning
    /// `$HOME`: that variable is process-global, and Cargo runs tests in
    /// threads, so a test that moved it would race every other test reading it.
    fn load_layered(user: Option<&Path>, root: &Path) -> Result<Self> {
        let project = root.join(".bobbin").join("config.toml");
        let sources = [user, Some(project.as_path())];

        let mut merged: Option<toml::Value> = None;
        for path in sources.into_iter().flatten() {
            let Some(table) = read_yupana_table(path)? else {
                continue;
            };
            merged = Some(match merged {
                Some(base) => merge(base, table),
                None => table,
            });
        }

        match merged {
            None => Ok(Self::default()),
            Some(value) => value
                .try_into()
                .map_err(|e| Error::Config(format!("[yupana]: {e}"))),
        }
    }
}

fn policy_mode_provenance_from_paths(
    user: Option<&Path>,
    project: &Path,
    effective: crate::policy::Mode,
) -> Result<PolicyModeProvenance> {
    let user_mode = user.map(explicit_policy_mode).transpose()?.flatten();
    let project_mode = explicit_policy_mode(project)?;
    let source = if project_mode.is_some() {
        project.display().to_string()
    } else if user_mode.is_some() {
        user.expect("user mode requires a path")
            .display()
            .to_string()
    } else {
        "compiled default".to_string()
    };
    let lowered_by_project =
        user_mode.is_some_and(|user| effective.is_lower_than(user)) && project_mode.is_some();
    Ok(PolicyModeProvenance {
        effective,
        source,
        user_mode,
        lowered_by_project,
    })
}

/// Deep-merge `overlay` onto `base`: tables merge key-by-key, everything else
/// is replaced outright.
///
/// Arrays replace rather than concatenate. Accumulating them would let a
/// workspace's `allow_paths` silently *widen* a scope the user config narrowed,
/// which inverts the direction a capability scope is allowed to move.
fn merge(base: toml::Value, overlay: toml::Value) -> toml::Value {
    match (base, overlay) {
        (toml::Value::Table(mut base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                let merged = match base.remove(&key) {
                    Some(existing) => merge(existing, value),
                    None => value,
                };
                base.insert(key, merged);
            }
            toml::Value::Table(base)
        }
        (_, overlay) => overlay,
    }
}

fn explicit_policy_mode(path: &Path) -> Result<Option<crate::policy::Mode>> {
    let Some(table) = read_yupana_table(path)? else {
        return Ok(None);
    };
    let Some(mode) = table
        .get("policy")
        .and_then(toml::Value::as_table)
        .and_then(|policy| policy.get("mode"))
    else {
        return Ok(None);
    };
    let Some(mode) = mode.as_str() else {
        return Err(Error::Config(format!(
            "{}: [yupana.policy].mode must be a string",
            path.display()
        )));
    };
    match mode {
        "off" => Ok(Some(crate::policy::Mode::Off)),
        "advise" => Ok(Some(crate::policy::Mode::Advise)),
        "enforce" => Ok(Some(crate::policy::Mode::Enforce)),
        _ => Err(Error::Config(format!(
            "{}: invalid [yupana.policy].mode `{mode}`",
            path.display()
        ))),
    }
}

/// Path to the per-user config, if a home directory is resolvable.
fn user_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("bobbin")
            .join("config.toml")
    })
}

/// Read the raw `[yupana]` table from a config file, if the file exists.
///
/// Returns the un-deserialized [`toml::Value`] so callers can merge tables
/// before building the struct — deserializing each file separately would bake
/// in defaults for its absent keys, and those defaults would then overwrite
/// real values from a lower-precedence file.
///
/// Each file is still type-checked here, even though the result is discarded,
/// so a malformed `[yupana]` is reported against the file that actually contains
/// it rather than surfacing later as an error about the merged whole.
fn read_yupana_table(path: &Path) -> Result<Option<toml::Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    let root: toml::Value =
        toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
    match root.get("yupana") {
        Some(section) => {
            let _: YupanaConfig = section
                .clone()
                .try_into()
                .map_err(|e| Error::Config(format!("{}: [yupana]: {e}", path.display())))?;
            Ok(Some(section.clone()))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
#[path = "config_test.rs"]
mod config_test;
