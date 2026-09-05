//! The `yupana` command-line interface.
//! `analyze`, `refs`, `status`, `serve` (MCP), the Phase-2 call-graph commands
//! `callers`/`impact` and `dataflow`, `export` (referential structure as Turtle,
//! §5.10/FR-34), the `watch` file-watcher (debounced, tiered re-extraction,
//! §5.5/FR-17), and the `hook` adapter (edit-reactive harness integration,
//! §5.9/FR-30) and `verify` (the FR-23/FR-24 edit-buffer verdict) are live.
//! `promote` is live behind the `quipu` feature (`docs/yupana-spec.md`).
use std::io;
use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use colored::Colorize;

use crate::cli_cmds;
use crate::config::YupanaConfig;
use crate::extract::extract_symbols;

// Command bodies live in child modules to keep this file under the size
// limit (yupana #83); they are `impl Cli` blocks reaching `self` as before.
#[path = "cli_analyze.rs"]
mod cli_analyze;
#[path = "cli_use.rs"]
mod cli_use;
use cli_use::deliberate_use_name;
#[path = "cli_export.rs"]
mod cli_export;
#[path = "cli_hook.rs"]
mod cli_hook;
#[path = "cli_promote.rs"]
pub mod cli_promote;
#[path = "cli_serve.rs"]
mod cli_serve;
#[cfg(feature = "quipu")]
#[path = "cli_share.rs"]
mod cli_share;
#[path = "cli_status.rs"]
mod cli_status;
#[path = "cli_status_rules.rs"]
mod cli_status_rules;
#[path = "cli_tracing.rs"]
mod cli_tracing;
use cli_hook::HookEvent;

pub use cli_tracing::init_tracing;

/// Yupana — live, per-tenant code structure for the Bobbin × Quipu stack.
#[derive(Debug, Parser)]
#[command(name = "yupana", version, about, long_about = None)]
pub struct Cli {
    /// The subcommand to run.
    #[command(subcommand)]
    command: Commands,

    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,

    /// Suppress non-essential output.
    #[arg(long, global = true)]
    quiet: bool,

    /// Show detailed progress.
    #[arg(long, global = true)]
    verbose: bool,

    /// Tenant/session id (defaults to single-tenant).
    #[arg(long, global = true, env = "BOBBIN_ROLE")]
    tenant: Option<String>,

    /// Path to a config file (overrides discovery).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
}

/// The available subcommands.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Run the MCP server (stdio by default; `--http` for streamable-HTTP).
    Serve {
        /// Serve over streamable-HTTP instead of stdio.
        #[arg(long)]
        http: bool,
    },
    /// Run the resident-graph daemon: build the base graph once and hold it,
    /// serving a local liveness surface (Phase 3, FR-31). Query endpoints and the
    /// hook/MCP thin-client cutover land in later stages.
    Daemon {
        /// Port for the daemon's local HTTP surface (defaults to `serve.mcp_http_port`).
        #[arg(long)]
        port: Option<u16>,
    },
    /// Build the base graph for a path and print a summary.
    Analyze {
        /// Directory or file to analyze (defaults to the current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Analyze the tree at a git ref (a baseline commit) instead of the
        /// working tree — the FR-13 base. Repo-relative; degrades to empty
        /// outside a repo or for an unresolved ref.
        #[arg(long)]
        at: Option<String>,
    },
    /// What does a CHANGE do — which entities does it add, remove or modify?
    ///
    /// The FR-13 baseline pointed at a change-time question instead of a tree
    /// one. `--base` is the ref to diff FROM (defaults to the configured
    /// `base_ref`); omit `--to` to judge the WORKING TREE, which is the shape a
    /// proposed, uncommitted change has.
    Changed {
        /// Ref to diff from (defaults to the configured `base_ref`).
        #[arg(long)]
        base: Option<String>,
        /// Ref to diff to. Omit to diff against the working tree.
        #[arg(long)]
        to: Option<String>,
    },
    /// Find the definition sites of a symbol by name.
    Refs {
        /// Symbol name to locate. With --at this slot is the search PATH
        /// instead: a name is redundant once a position names the symbol.
        symbol: Option<String>,
        /// Directory to search (defaults to the current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Resolve the symbol at a POSITION instead of by name: `FILE:LINE` or
        /// `FILE:LINE:COL` when built with the LSP precision tier,
        /// relative to the search path. Disambiguates common names — `build`,
        /// `new` — by pointing at the one you mean. The tree-sitter tier
        /// resolves to the innermost symbol enclosing that LINE. A build with
        /// the LSP tier accepts a column; other builds refuse it rather than
        /// silently ignoring precision the caller requested.
        #[arg(long, value_name = "FILE:LINE[:COL]")]
        at: Option<String>,
    },
    /// Direct callers and callees of a symbol.
    Callers {
        /// Symbol name.
        symbol: String,
        /// Directory to build the call graph over (defaults to current dir).
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Blast radius: symbols transitively affected by changing a symbol.
    Impact {
        /// Seed symbol.
        symbol: String,
        /// Directory to build the call graph over (defaults to current dir).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Maximum hops to follow.
        #[arg(long, default_value_t = 5)]
        hops: u32,
        /// Reconcile against a co-change file set (FR-11): a JSON array of
        /// paths, or a newline-separated list. Supplied by Bobbin.
        #[arg(long)]
        cochange: Option<PathBuf>,
    },
    /// Detected communities: densely-connected clusters of symbols (FR-9).
    Communities {
        /// Directory to build the call graph over (defaults to current dir).
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Count same-file symbol-name collisions — names whose definitions share
    /// one unqualified symbol IRI and merge into a single node on promotion.
    /// The sizing input for the scope-qualified IRI migration; only the
    /// extractor can see the same-kind variant, so this lives here and not in
    /// a graph query.
    Census {
        /// Directory to scan (defaults to the current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Export the referential structure (modules, symbols, edges) as Turtle,
    /// or promote it into Quipu with `--to` (FR-34; the promotion spelling of
    /// `promote`, spec §15).
    Export {
        /// Directory to export (defaults to current dir).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Repository name to attribute entities to. Defaults to the `origin`
        /// remote's repo name, else the dir name (print-only); a promotion via
        /// `--to` refuses the dir-name guess and wants `--repo` or an origin.
        /// The dir-name guess announces itself on stderr: it is a segment of
        /// every IRI, so output captured from two checkout paths describes two
        /// disjoint graphs. Pass `--repo` for anything shared or imported.
        #[arg(long)]
        repo: Option<String>,
        /// Output format.
        #[arg(long, default_value = "turtle")]
        format: ExportFormat,
        /// Promote into the Quipu at this base URL instead of printing Turtle.
        /// SHACL-validates before writing, exactly like `yupana promote`.
        #[arg(long)]
        to: Option<String>,
    },
    /// Intra-procedural data dependence within a function.
    Dataflow {
        /// Function to analyze.
        function: String,
        /// Directory to build the dataflow over (defaults to current dir).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Trace flow for a specific variable (omit to list all edges).
        #[arg(long)]
        var: Option<String>,
        /// Trace what the variable flows into, rather than what it depends on.
        #[arg(long)]
        forward: bool,
        /// Maximum hops to follow.
        #[arg(long, default_value_t = 5)]
        hops: u32,
    },
    /// Verdict on a proposed edit buffer (FR-23/FR-24).
    Verify {
        /// The file being edited.
        #[arg(long)]
        file: PathBuf,
        /// The edited buffer to check.
        #[arg(long)]
        buffer: PathBuf,
    },
    /// Draft policy raw material from an exemplar: Selector + tiered
    /// predicate candidates (policy-by-example, step 2). Output is JSON for
    /// quipu's drafting scaffold; the placement check remains the refusal
    /// authority.
    Exemplar {
        /// The offending text, verbatim. Omit to read a spooled denial.
        text: Option<String>,
        /// The file the text appeared in — names the Selector's context.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Verdict-spool path to read the newest denial from instead of TEXT.
        #[arg(long)]
        spool: Option<PathBuf>,
        /// With --spool: pick the newest denial under this predicate id.
        #[arg(long)]
        policy: Option<String>,
    },
    /// Promote a commit's structural facts into Quipu (Phase 4).
    Promote(cli_promote::PromoteArgs),
    /// Watch a tree and re-extract changed files, debounced and tiered (FR-17).
    Watch {
        /// Directory to watch (defaults to the current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Show base commit, tiers, and configuration.
    Status,
    /// Refresh the governed-policy cache from Quipu for edit hooks.
    #[cfg(feature = "quipu")]
    RefreshProjection,
    /// Agent-harness hook adapter (reads the hook payload on stdin).
    Hook {
        /// Which hook event to handle.
        event: HookEvent,
    },
    /// Generate shell completions.
    Completions {
        /// Target shell.
        shell: clap_complete::Shell,
    },
    /// Show the ed25519 public key of yupana's verdict-signing identity, to
    /// register in quipu as this verifier's `aegis:publicKey` (quipu feature).
    #[cfg(feature = "quipu")]
    Verifier {
        /// Path to the PKCS#8 signing key (created 0600 if absent).
        #[arg(long, default_value = "yupana-signing.pk8")]
        key_path: PathBuf,
    },
    /// Promote spooled verdicts into quipu (quipu feature).
    ///
    /// The guard signs and spools locally; this command promotes later so
    /// Quipu latency never enters the edit path.
    #[cfg(feature = "quipu")]
    Verdicts {
        /// Quipu base URL. Defaults to `[yupana.quipu] endpoint`.
        #[arg(long)]
        to: Option<String>,
        /// Spool file. Defaults to the same resolution the guard uses.
        #[arg(long)]
        spool: Option<PathBuf>,
    },
    #[cfg(feature = "quipu")]
    Certify(crate::action_certification::CertifyArgs),
    /// Pull a Quipu share into the graph yupana reads, see what policy it
    /// would contribute, and admit it deliberately. Never promotes implicitly.
    #[cfg(feature = "quipu")]
    Share(crate::share_pull::ShareArgs),
}
/// Output formats for `yupana export`.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum ExportFormat {
    /// RDF Turtle in the `bobbin:` code ontology.
    Turtle,
}
impl Cli {
    /// Whether `--verbose` was passed. Consulted by `main` to raise the default
    /// tracing verbosity (see [`init_tracing`]).
    #[must_use]
    pub fn verbose(&self) -> bool {
        self.verbose
    }

    /// Load configuration honouring the global `--config` override.
    ///
    /// Every command that reads config goes through here, so `--config` is
    /// honoured uniformly rather than silently ignored on all but a chosen few
    /// (aegis-ll3p).
    fn load_config(&self, root: &Path) -> anyhow::Result<YupanaConfig> {
        YupanaConfig::resolve(self.config.as_deref(), root).map_err(Into::into)
    }

    /// Run the parsed command.
    pub async fn run(self) -> anyhow::Result<()> {
        // DELIBERATE-USE metric (aegis-0nng): the leverage signal aegis-m9ln's
        // workflow half exists to move — is anyone running analyze/impact/refs
        // BEFORE a change, or does yupana only ever fire as the passive guard?
        // The hook is excluded (the guard spools its own richer line) and so is
        // shell completion (not a use). Fail-silent by the spool's contract.
        if let Some(cmd) = deliberate_use_name(&self.command) {
            crate::metrics::emit("command", &[("cmd", cmd.into())]);
        }
        match &self.command {
            Commands::Analyze { path, at } => self.analyze(path, at.as_deref()),
            Commands::Refs { symbol, path, at } => {
                self.refs(symbol.as_deref(), path, at.as_deref())
            }
            Commands::Watch { path } => self.watch(path).await,
            Commands::Status => self.status(),
            #[cfg(feature = "quipu")]
            Commands::RefreshProjection => self.refresh_projection(),
            Commands::Hook { event } => {
                cli_hook::run(*event, self.tenant.as_deref(), self.config.as_deref())
            }
            Commands::Completions { shell } => {
                let mut cmd = Cli::command();
                clap_complete::generate(*shell, &mut cmd, "yupana", &mut io::stdout());
                Ok(())
            }
            #[cfg(feature = "quipu")]
            Commands::Verifier { key_path } => {
                let keypair = crate::verdict::load_or_generate(key_path)?;
                println!("verifier: {}", crate::verdict::VERIFIER);
                println!("public_key: {}", crate::verdict::public_key_hex(&keypair));
                Ok(())
            }
            #[cfg(feature = "quipu")]
            Commands::Verdicts { to, spool } => {
                self.drain_verdicts(to.as_deref(), spool.as_deref())
            }
            #[cfg(feature = "quipu")]
            Commands::Certify(args) => args.run(),
            #[cfg(feature = "quipu")]
            Commands::Share(args) => Ok(args.run(self.quipu_endpoint().as_deref())?),
            Commands::Serve { http } => self.serve(*http).await,
            Commands::Daemon { port } => self.daemon(*port).await,
            Commands::Callers { symbol, path } => {
                cli_cmds::callers(self.json, self.quiet, symbol, path)
            }
            Commands::Communities { path } => cli_cmds::communities(self.json, self.quiet, path),
            Commands::Census { path } => cli_cmds::census(self.json, self.quiet, path),
            Commands::Impact {
                symbol,
                path,
                hops,
                cochange,
            } => cli_cmds::impact(
                self.json,
                self.quiet,
                symbol,
                path,
                *hops,
                cochange.as_deref(),
            ),
            Commands::Export {
                path,
                repo,
                format,
                to,
            } => self.export_arm(path, repo.as_deref(), format, to.as_deref()),
            Commands::Dataflow {
                function,
                path,
                var,
                forward,
                hops,
            } => cli_cmds::dataflow(
                self.json,
                self.quiet,
                function,
                path,
                var.as_deref(),
                *forward,
                *hops,
            ),
            Commands::Changed { base, to } => self.changed(base.as_deref(), to.as_deref()),
            Commands::Verify { file, buffer } => {
                cli_cmds::verify(self.json, self.quiet, file, buffer)
            }
            Commands::Exemplar {
                text,
                file,
                spool,
                policy,
            } => self.exemplar(
                text.as_deref(),
                file.as_deref(),
                spool.as_deref(),
                policy.as_deref(),
            ),
            Commands::Promote(a) => {
                let (commit, path) = (&a.commit, &a.path);
                // `--subset` and its scope travel as ONE value, so "list the keys
                // of a promote that is not a subset" and "a base with no subset"
                // are unrepresentable below rather than merely refused by clap.
                let subset = a.subset.then_some(cli_promote::Subset {
                    base: a.base.as_deref(),
                    list_keys: a.list_keys,
                });
                // THE WRITE GUARD, made real (aegis-ltjo). Promotion is the write
                // yupana performs, so `serve.read_only` must refuse it — BEFORE any
                // work, so the guard holds regardless of feature.
                //
                // `--dry-run` is still gated here, deliberately. A dry run writes
                // nothing and arguably should pass a read-only guard, but that is a
                // change to what `read_only` MEANS, and this arm is not the place to
                // make it silently. Tracked on aegis-o2h97.
                self.load_config(path)?.write_guard("promotion")?;
                // FR-19's trigger: `promote_on` decides whether the DECLARED
                // event promotes at all, before any tree is read.
                if !self.trigger_admits(path, commit, a.trigger)? {
                    return Ok(());
                }
                // `--subset` and its base travel as ONE value, so "subset with
                // no base" is unrepresentable below rather than merely refused
                // by clap. The two are meaningless apart.
                self.promote(
                    path,
                    commit,
                    a.to.as_deref(),
                    a.repo.as_deref(),
                    a.dry_run,
                    a.replace_snapshot,
                    // `--subset` and its scope travel as ONE value, so "list the
                    // keys of a promote that is not a subset" and "a base with no
                    // subset" are unrepresentable here rather than merely refused
                    // by clap.
                    subset.as_ref(),
                )
            }
        }
    }

    /// Print a notice for a command whose engine has not yet landed.
    // Used only by the feature-stub arms (serve/daemon without `mcp`, promote
    // without `quipu`); a build with both features enabled reaches none, so it is
    // legitimately dead there.
    #[allow(dead_code)]
    fn planned(&self, name: &str, phase: u8, detail: &str) {
        if !self.quiet {
            eprintln!(
                "{} `{name}` is planned for Phase {phase}: {detail}. See docs/yupana-spec.md.",
                "note:".yellow().bold()
            );
        }
    }
}
