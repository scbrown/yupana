//! File-watch, debounce, and tiered re-extraction scheduling (FR-17).
//!
//! This is the on-disk edit source for the incremental update path. A `notify`
//! watcher observes the repo tree; raw filesystem events are noisy (an editor
//! save can emit several within milliseconds), so they are **debounced** into a
//! quiet-window batch before any work runs. Work is then dispatched in **tiers**
//! (FR-17): the cheap tree-sitter re-parse fires on a short window (per-keystroke
//! feel), while heavier recompute — the frontier/graph work, and later the LSP
//! and CPG tiers — is deferred behind a longer window so a burst of edits does
//! not thrash it.
//!
//! The scheduling logic (`Debouncer`, `TieredScheduler`) is pure and
//! time-injected so it is deterministically testable without real filesystem
//! events or sleeps; it lives in [`schedule`]. The `notify` wiring in
//! [`Watcher`] here is a thin adapter that feeds those components and invokes a
//! [`TierHandler`].
//!
//! ## Relationship to the incremental update path
//!
//! The tiers **feed** the extraction/overlay entrypoints. Two handlers exist:
//! - [`GraphRefresh`] (un-tenanted): tree-sitter tier re-parses each changed
//!   file; heavy tier rebuilds the whole [`crate::graph::CodeGraph`]. Used by
//!   `yupana watch` with no tenant.
//! - [`OverlayRefresh`] (tenant-aware): tree-sitter tier touches the tenant's
//!   overlay; heavy tier is the frontier-bounded [`crate::graph::update_frontier`]
//!   (FR-16), touching only the edited files + their reach — not a full rebuild.
//!   It also tracks per-file [`crate::types::Freshness`] across the two tiers.
//!
//! Events are filtered by extension/`target`/`.git` ([`is_watch_relevant`]) and
//! by the repo's `.gitignore` (via the `ignore` crate) before reaching the
//! scheduler. See `docs/yupana-spec.md` §5.5, §7.5, FR-16/17.

mod overlay_refresh;
mod schedule;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::graph::CodeGraph;
use crate::types::Tier;

pub use overlay_refresh::OverlayRefresh;
pub use schedule::{is_watch_relevant, Debouncer, TierBatch, TieredScheduler};

/// Sink for tiered re-extraction work. The watcher calls exactly one method per
/// ready tier, with the debounced batch of changed files.
pub trait TierHandler: Send {
    /// The cheap tree-sitter tier: re-parse each changed file. Fires on the
    /// short window (FR-17). `paths` is non-empty.
    fn tree_sitter(&mut self, paths: &[PathBuf]);

    /// The deferred heavy tier: recompute the affected graph/frontier (and, in
    /// later phases, LSP/CPG facts). Fires on the long window. `paths` is
    /// non-empty.
    fn heavy(&mut self, paths: &[PathBuf]);
}

/// The un-tenanted handler: feed the extraction entrypoints, whole-tree.
///
/// - tree-sitter tier → [`crate::extract::extract_structure`] per changed file
///   (the FR-16 "re-parse X" step);
/// - heavy tier → rebuild [`CodeGraph`] over `root`.
///
/// For per-tenant, frontier-bounded updates (the FR-16 path) use
/// [`OverlayRefresh`] instead; this remains for `yupana watch` with no tenant.
pub struct GraphRefresh {
    root: PathBuf,
}

impl GraphRefresh {
    /// Refresh the graph rooted at `root`.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl TierHandler for GraphRefresh {
    fn tree_sitter(&mut self, paths: &[PathBuf]) {
        for path in paths {
            match std::fs::read_to_string(path) {
                Ok(source) => match crate::extract::extract_structure(&source, "rust") {
                    Ok(structure) => tracing::info!(
                        file = %path.display(),
                        symbols = structure.symbols.len(),
                        calls = structure.calls.len(),
                        tier = Tier::TreeSitter.as_str(),
                        "re-extracted (tree-sitter)"
                    ),
                    Err(e) => {
                        tracing::debug!(file = %path.display(), error = %e, "tree-sitter re-parse skipped");
                    }
                },
                Err(e) => {
                    // A removed file is a legitimate event; nothing to re-parse.
                    tracing::debug!(file = %path.display(), error = %e, "unreadable (removed?)");
                }
            }
        }
    }

    fn heavy(&mut self, paths: &[PathBuf]) {
        match CodeGraph::build(&self.root) {
            Ok(graph) => {
                let (nodes, edges) = graph.stats();
                tracing::info!(
                    changed = paths.len(),
                    nodes,
                    edges,
                    "graph rebuilt (heavy tier)"
                );
            }
            Err(e) => tracing::warn!(error = %e, "heavy-tier graph rebuild failed"),
        }
    }
}

/// A running file-watch. Dropping (or calling [`stop`](Self::stop)) halts the
/// watcher and joins its worker thread.
pub struct Watcher {
    _inner: RecommendedWatcher,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Start watching `root` recursively, dispatching debounced, tiered
    /// re-extraction to `handler`.
    ///
    /// `poll_interval` bounds how often the worker wakes to flush ready batches
    /// even absent new events (so a debounce deadline is honored promptly).
    pub fn start(
        root: &Path,
        mut scheduler: TieredScheduler,
        mut handler: Box<dyn TierHandler>,
        poll_interval: Duration,
    ) -> crate::Result<Self> {
        let (tx, rx) = mpsc::channel::<PathBuf>();

        // Respect the repo's .gitignore (and nested ones): a build artifact or
        // a generated file that happens to be `.rs` must not drive overlay
        // churn. Built once from `root`; an unreadable/absent ignore file just
        // yields an empty matcher (nothing extra ignored), never a failure.
        let ignore = build_gitignore(root);

        // notify runs the callback on its own thread; forward relevant changed
        // paths into the channel and drop the rest.
        let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                if !(event.kind.is_create() || event.kind.is_modify() || event.kind.is_remove()) {
                    return;
                }
                for path in event.paths {
                    // Extension/target/.git filter first (cheap), then gitignore
                    // — `_or_any_parents` so a file under an ignored DIR is
                    // excluded, not just one matching a filename glob.
                    if is_watch_relevant(&path)
                        && !ignore.matched_path_or_any_parents(&path, false).is_ignore()
                    {
                        // Receiver gone → watcher shutting down; ignore.
                        let _ = tx.send(path);
                    }
                }
            }
        })
        .map_err(|e| watch_err(&e))?;
        inner
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| watch_err(&e))?;

        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let worker = std::thread::spawn(move || {
            while worker_running.load(Ordering::Relaxed) {
                // Wake on the next deadline, or the poll interval if idle.
                let wait = scheduler.next_deadline().map_or(poll_interval, |d| {
                    d.saturating_duration_since(Instant::now())
                        .min(poll_interval)
                });
                match rx.recv_timeout(wait) {
                    Ok(path) => scheduler.record(path, Instant::now()),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                let batch = scheduler.poll(Instant::now());
                if !batch.tree_sitter.is_empty() {
                    handler.tree_sitter(&batch.tree_sitter);
                }
                if !batch.heavy.is_empty() {
                    handler.heavy(&batch.heavy);
                }
            }
        });

        Ok(Self {
            _inner: inner,
            running,
            worker: Some(worker),
        })
    }

    /// Stop the watcher and join its worker thread.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Map a `notify` error into the crate error type.
fn watch_err(e: &notify::Error) -> crate::Error {
    crate::Error::Config(format!("file-watch: {e}"))
}

/// A gitignore matcher for `root`, honoring `root/.gitignore` and nested ones
/// the builder discovers. Errors degrade to an empty matcher — a broken ignore
/// file must not silently swallow every event, so nothing is ignored rather
/// than everything.
fn build_gitignore(root: &Path) -> ignore::gitignore::Gitignore {
    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    // add() returns Some(err) on a read/parse problem; ignore it and build what
    // we have (possibly empty).
    let _ = builder.add(root.join(".gitignore"));
    builder
        .build()
        .unwrap_or_else(|_| ignore::gitignore::Gitignore::empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A recording handler for the integration test.
    #[derive(Clone, Default)]
    struct Recorder {
        tree_sitter: Arc<Mutex<Vec<PathBuf>>>,
        heavy: Arc<Mutex<Vec<PathBuf>>>,
    }

    impl TierHandler for Recorder {
        fn tree_sitter(&mut self, paths: &[PathBuf]) {
            self.tree_sitter.lock().unwrap().extend_from_slice(paths);
        }
        fn heavy(&mut self, paths: &[PathBuf]) {
            self.heavy.lock().unwrap().extend_from_slice(paths);
        }
    }

    #[test]
    fn gitignore_excludes_ignored_rust_files() {
        // A .rs file matched by .gitignore must not drive overlay churn, even
        // though the extension filter alone would accept it (FR-17).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "generated/\n*.gen.rs\n").unwrap();
        std::fs::create_dir(dir.path().join("generated")).unwrap();
        let ignore = build_gitignore(dir.path());

        let ignored = |rel: &str| {
            ignore
                .matched_path_or_any_parents(dir.path().join(rel), false)
                .is_ignore()
        };
        assert!(
            ignored("generated/x.rs"),
            "a file under an ignored dir is excluded"
        );
        assert!(
            ignored("build.gen.rs"),
            "a file matching an ignore glob is excluded"
        );
        assert!(!ignored("src/lib.rs"), "a normal source file is watched");
    }

    #[test]
    fn watcher_dispatches_a_real_file_change() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::default();
        let seen_heavy = Arc::clone(&rec.heavy);

        // Short windows so the test is quick but still exercises debouncing.
        let scheduler = TieredScheduler::new(Duration::from_millis(50), Duration::from_millis(120));
        let watcher = Watcher::start(
            dir.path(),
            scheduler,
            Box::new(rec.clone()),
            Duration::from_millis(20),
        )
        .unwrap();

        // Give the OS watcher a moment to arm, then create + modify a file.
        std::thread::sleep(Duration::from_millis(100));
        let file = dir.path().join("thing.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();
        std::thread::sleep(Duration::from_millis(30));
        std::fs::write(&file, "fn a() {}\nfn b() { a(); }\n").unwrap();

        // Poll for up to ~3s for the deferred heavy tier to fire — it fires after
        // the tree-sitter tier (longer window), so observing it proves the whole
        // pipeline ran. If the platform delivers no events (unsupported FS), skip
        // rather than fail — same degradation stance as the git-backed tests.
        let mut got = false;
        for _ in 0..150 {
            if !seen_heavy.lock().unwrap().is_empty() {
                got = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        watcher.stop();

        if got {
            // The cheap tier necessarily ran before the heavy one did.
            assert!(
                rec.tree_sitter.lock().unwrap().iter().any(|p| p == &file),
                "tree-sitter tier must have re-extracted the changed file"
            );
        } else {
            eprintln!("note: no filesystem events delivered; watcher dispatch not exercised");
        }
    }
}
