//! Pure, time-injected scheduling for the file-watch (FR-17).
//!
//! [`Debouncer`] and [`TieredScheduler`] carry no filesystem or clock
//! dependency — time is passed in explicitly — so the debounce/tier logic is
//! deterministically testable without real events or sleeps. The `notify`
//! wiring that feeds them lives in the parent [`crate::watch`] module.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::FreshnessConfig;

/// A last-write-wins debouncer over paths.
///
/// Each observed change for a path (re)stamps that path's deadline at
/// `event_time + window`. [`flush_ready`](Self::flush_ready) returns and removes
/// the paths whose deadline has passed as of the supplied `now`. Repeated events
/// for the same path within the window coalesce into one flush — the point of
/// debouncing.
///
/// Time is passed in explicitly rather than read from a clock, so the whole
/// component is deterministically testable.
#[derive(Debug)]
pub struct Debouncer {
    window: Duration,
    /// path → instant at which it becomes ready to flush.
    deadlines: HashMap<PathBuf, Instant>,
}

impl Debouncer {
    /// Create a debouncer with the given quiet `window`.
    #[must_use]
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            deadlines: HashMap::new(),
        }
    }

    /// Record a change for `path` observed at `at`; resets its quiet window.
    pub fn record(&mut self, path: PathBuf, at: Instant) {
        self.deadlines.insert(path, at + self.window);
    }

    /// Remove and return every path whose quiet window elapsed by `now`.
    ///
    /// The returned paths are sorted for a stable, reproducible batch order.
    pub fn flush_ready(&mut self, now: Instant) -> Vec<PathBuf> {
        let mut ready: Vec<PathBuf> = self
            .deadlines
            .iter()
            .filter(|(_, &deadline)| deadline <= now)
            .map(|(path, _)| path.clone())
            .collect();
        for path in &ready {
            self.deadlines.remove(path);
        }
        ready.sort();
        ready
    }

    /// The earliest deadline still pending, if any — when the caller should next
    /// wake to flush.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.deadlines.values().copied().min()
    }

    /// Whether nothing is pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deadlines.is_empty()
    }
}

/// One tier's worth of ready paths produced by a scheduler poll.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TierBatch {
    /// Files ready for the cheap tree-sitter re-parse ([`crate::types::Tier::TreeSitter`]).
    pub tree_sitter: Vec<PathBuf>,
    /// Files ready for the deferred heavy recompute (graph/frontier; later LSP/CPG).
    pub heavy: Vec<PathBuf>,
}

impl TierBatch {
    /// Whether both tiers are empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tree_sitter.is_empty() && self.heavy.is_empty()
    }
}

/// Two-tier debounced scheduler: a short window drives the cheap tree-sitter
/// tier, a longer window defers the heavy tier (FR-17).
///
/// A change is recorded into both debouncers. Because each debouncer resets its
/// path's deadline on every event, a continuous edit burst keeps deferring the
/// heavy tier while the tree-sitter tier fires shortly after the last change —
/// exactly "structure updates on save / debounced keystroke; heavier facts
/// update on save / on-demand".
#[derive(Debug)]
pub struct TieredScheduler {
    fast: Debouncer,
    heavy: Debouncer,
}

impl TieredScheduler {
    /// Create a scheduler with a `fast_window` (tree-sitter) and a `heavy_window`
    /// (deferred recompute). `heavy_window` is expected to be ≥ `fast_window`.
    #[must_use]
    pub fn new(fast_window: Duration, heavy_window: Duration) -> Self {
        Self {
            fast: Debouncer::new(fast_window),
            heavy: Debouncer::new(heavy_window),
        }
    }

    /// Build a scheduler from the `[yupana.freshness]` config: `debounce_ms` is the
    /// tree-sitter window; `heavy_debounce_ms` the deferred window.
    #[must_use]
    pub fn from_config(freshness: &FreshnessConfig) -> Self {
        Self::new(
            Duration::from_millis(freshness.debounce_ms),
            Duration::from_millis(freshness.heavy_debounce_ms),
        )
    }

    /// Record a change for `path` observed at `at` into both tiers.
    pub fn record(&mut self, path: PathBuf, at: Instant) {
        self.fast.record(path.clone(), at);
        self.heavy.record(path, at);
    }

    /// Collect the paths whose windows have elapsed by `now`, per tier.
    pub fn poll(&mut self, now: Instant) -> TierBatch {
        TierBatch {
            tree_sitter: self.fast.flush_ready(now),
            heavy: self.heavy.flush_ready(now),
        }
    }

    /// The earliest instant at which either tier will next have work.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        match (self.fast.next_deadline(), self.heavy.next_deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Whether both tiers are idle.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.fast.is_empty() && self.heavy.is_empty()
    }
}

/// Whether a changed path is worth re-extracting: a Rust source file that is not
/// build output or VCS metadata. (Only Rust is wired today; the `langs-extra`
/// grammars widen this later.)
#[must_use]
pub fn is_watch_relevant(path: &Path) -> bool {
    if path.extension().is_none_or(|ext| ext != "rs") {
        return false;
    }
    !path.components().any(|c| {
        let s = c.as_os_str();
        s == "target" || s == ".git"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn debouncer_holds_until_window_elapses() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(Duration::from_millis(300));
        d.record(p("a.rs"), t0);

        // Before the window: nothing ready.
        assert!(d.flush_ready(t0 + Duration::from_millis(299)).is_empty());
        // At/after the window: the path flushes exactly once.
        assert_eq!(
            d.flush_ready(t0 + Duration::from_millis(300)),
            vec![p("a.rs")]
        );
        assert!(d.flush_ready(t0 + Duration::from_secs(10)).is_empty());
    }

    #[test]
    fn debouncer_coalesces_a_burst() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(Duration::from_millis(300));
        // A save-storm: same file several times inside the window.
        d.record(p("a.rs"), t0);
        d.record(p("a.rs"), t0 + Duration::from_millis(100));
        d.record(p("a.rs"), t0 + Duration::from_millis(250));

        // The window is measured from the *last* event, so 300ms after t0 is
        // still too early — the burst coalesced, deadline moved out.
        assert!(d.flush_ready(t0 + Duration::from_millis(300)).is_empty());
        // One flush, not three, once the tail settles.
        assert_eq!(
            d.flush_ready(t0 + Duration::from_millis(550)),
            vec![p("a.rs")]
        );
    }

    #[test]
    fn debouncer_next_deadline_tracks_earliest() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(Duration::from_millis(100));
        d.record(p("late.rs"), t0 + Duration::from_millis(50));
        d.record(p("early.rs"), t0);
        assert_eq!(d.next_deadline(), Some(t0 + Duration::from_millis(100)));
    }

    #[test]
    fn tiered_scheduler_fires_fast_before_heavy() {
        let t0 = Instant::now();
        let mut s = TieredScheduler::new(Duration::from_millis(300), Duration::from_millis(1500));
        s.record(p("x.rs"), t0);

        // After the fast window: tree-sitter fires, heavy still deferred.
        let batch = s.poll(t0 + Duration::from_millis(400));
        assert_eq!(batch.tree_sitter, vec![p("x.rs")]);
        assert!(batch.heavy.is_empty());

        // After the heavy window: heavy fires (and only once).
        let batch = s.poll(t0 + Duration::from_millis(1600));
        assert!(batch.tree_sitter.is_empty());
        assert_eq!(batch.heavy, vec![p("x.rs")]);
    }

    #[test]
    fn tiered_scheduler_defers_heavy_across_a_burst() {
        let t0 = Instant::now();
        let mut s = TieredScheduler::new(Duration::from_millis(300), Duration::from_millis(1000));
        // Continuous editing every 500ms keeps pushing the heavy deadline out.
        s.record(p("x.rs"), t0);
        s.record(p("x.rs"), t0 + Duration::from_millis(500));
        s.record(p("x.rs"), t0 + Duration::from_millis(1000));

        // Fast tier already fired for the earlier edits; heavy has NOT — its
        // deadline is now t0+2000, so at t0+1200 it is still pending.
        let batch = s.poll(t0 + Duration::from_millis(1200));
        assert!(batch.heavy.is_empty(), "heavy must defer during a burst");

        // Once edits stop, heavy finally fires.
        let batch = s.poll(t0 + Duration::from_millis(2100));
        assert_eq!(batch.heavy, vec![p("x.rs")]);
    }

    #[test]
    fn relevance_filter_accepts_only_source_rust() {
        assert!(is_watch_relevant(Path::new("src/lib.rs")));
        assert!(!is_watch_relevant(Path::new("README.md")));
        assert!(!is_watch_relevant(Path::new("target/debug/build.rs")));
        assert!(!is_watch_relevant(Path::new(".git/HEAD")));
    }
}
