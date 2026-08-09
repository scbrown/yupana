//! action — resolve a shell command line to (verb, target, `target_class`), or ABSTAIN.
//!
//! The trace half of the work-scoped governance epic (aegis-368cu.1, gap 2). The
//! spool records `command {cmd: "<raw string>"}`, and a raw string is a thing no
//! policy can be written against: "who restarted quipu" and "who touched web.example"
//! are not answerable by grepping command lines, because the same intent has a
//! dozen spellings and the same spelling has a dozen intents.
//!
//! ABSTAIN, NEVER GUESS. This is the load-bearing rule and it is a stated
//! constraint of the bead, not a preference. Phase 2 REPLAYS these records to
//! derive rules, and Phase 4+ may enforce them. A resolver that guesses a target
//! produces a record that justifies a rule that denies the wrong action later —
//! and the denial will cite this record as its evidence, which makes a wrong
//! guess worse than a missing one. `Unknown` is a first-class, honest answer and
//! it is the default for everything not on the small recognised list below.
//!
//! WHY A SMALL LIST IS THE FEATURE. It would be easy to keep adding patterns
//! until "most" commands resolve. That is the wrong direction: coverage bought
//! with fuzzy matching converts silence (which is visibly incomplete) into
//! confident noise (which is not). The recognised set is deliberately the set of
//! forms whose target is UNAMBIGUOUS from syntax alone:
//!
//!   ssh/scp     -> host      the host is a positional operand, not a heuristic
//!   ansible -l  -> host      an explicit limit flag
//!   systemctl   -> service   the unit is the operand of a known verb
//!   git remote  -> repo      a URL/path with a knowable name
//!   pct/docker  -> container the id/name is the operand of a known verb
//!
//! Anything else — a pipeline, a shell function, a script that does any of the
//! above internally — is `Unknown`. That is correct: yupana cannot see inside a
//! script, and pretending otherwise is how a trace stops being evidence.

/// What kind of thing an action targets. Kept deliberately coarse: these are the
/// classes policy is written against, and a class nobody can write a rule for is
/// a class that should not exist yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetClass {
    /// A machine: the operand of ssh/scp, or an explicit ansible `--limit`.
    Host,
    /// A systemd unit, named as the operand of a known systemctl verb.
    Service,
    /// A git repository, named by a remote URL or path.
    Repo,
    /// An LXC/Docker/Podman container, by id or name.
    Container,
    /// We could not tell. NOT an error, and not a lesser answer — see the
    /// module note: an honest abstention is what keeps the trace usable as
    /// evidence.
    Unknown,
}

impl TargetClass {
    /// The wire form. One vocabulary shared by records and rules (the epic's
    /// whole premise), so these strings are an interface — changing one
    /// silently invalidates every rule derived from older records.
    pub fn as_str(self) -> &'static str {
        match self {
            TargetClass::Host => "host",
            TargetClass::Service => "service",
            TargetClass::Repo => "repo",
            TargetClass::Container => "container",
            TargetClass::Unknown => "unknown",
        }
    }
}

/// One resolved action. `verb` is the actor's intent, `target` the thing acted
/// on. Both are `None`-shaped as "unknown" rather than empty strings so a
/// caller cannot accidentally serialise a blank that reads as a real value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// The actor's intent (`restart`, `push`, `ssh`). `None` when unresolved.
    pub verb: Option<String>,
    /// The thing acted on (`web.example`, `quipu`). `None` when unresolved — never
    /// an empty string, which a replayer would read as a real value.
    pub target: Option<String>,
    /// What KIND of thing `target` is; `Unknown` is the honest default.
    pub target_class: TargetClass,
}

impl Action {
    /// The abstention. Used for everything not positively recognised.
    pub fn unknown() -> Self {
        Action {
            verb: None,
            target: None,
            target_class: TargetClass::Unknown,
        }
    }

    fn new(verb: &str, target: &str, class: TargetClass) -> Self {
        Action {
            verb: Some(verb.to_string()),
            target: Some(target.to_string()),
            target_class: class,
        }
    }

    /// Did we actually resolve anything? Cheap predicate so a caller can decide
    /// whether to record the structured fields at all.
    pub fn is_known(&self) -> bool {
        self.target_class != TargetClass::Unknown
    }
}

/// Split a command line into bare words. Deliberately NOT a shell parser: it
/// strips only simple surrounding quotes, and any command containing shell
/// metacharacters is refused upstream by `resolve`. A half-correct shell parser
/// is exactly the kind of thing that resolves a target confidently and wrongly.
fn words(cmd: &str) -> Vec<&str> {
    cmd.split_whitespace()
        .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
        .filter(|w| !w.is_empty())
        .collect()
}

/// True if this line does more than one thing, or hides what it does. Pipelines,
/// chains, substitutions and redirects all mean the observable command is not
/// the whole action, so the honest answer for the line as a whole is Unknown.
fn is_compound(cmd: &str) -> bool {
    cmd.contains('|')
        || cmd.contains(';')
        || cmd.contains("&&")
        || cmd.contains("||")
        || cmd.contains('`')
        || cmd.contains("$(")
        || cmd.contains('>')
        || cmd.contains('<')
}

/// A `host`, `user@host`, or scp `user@host:/path` operand -> the host part.
///
/// The colon is split FIRST, because `root@web.example:/opt/x` is the ordinary scp
/// remote spelling and it carries both a host and a path. Splitting on the colon
/// isolates the host side, so a slash in the PATH no longer disqualifies an
/// operand whose host is perfectly unambiguous. (First cut rejected any word
/// containing a slash and so abstained on the single most common deploy shape
/// there is — caught by the aegis-0jv06 known-answer test, which is exactly what
/// that test is for.)
///
/// A LOCAL path is still refused: `./target/release/x` has a slash on the HOST
/// side of the split, which no host ever does.
fn host_operand(word: &str) -> Option<&str> {
    if word.starts_with('-') {
        return None;
    }
    // scp remote spec: everything before the FIRST colon is the host side.
    let host_side = word.split(':').next()?;
    if host_side.is_empty() || host_side.contains('/') {
        return None;
    }
    let host = host_side.rsplit('@').next()?;
    if host.is_empty() {
        return None;
    }
    // A bare word with neither an explicit user@ nor a dotted name is too weak
    // to claim — `ssh myhost` is real, but so is a stray flag value.
    if host_side.contains('@') || host.contains('.') {
        Some(host)
    } else {
        None
    }
}

/// Resolve a command line, or abstain.
///
/// EVERY BRANCH THAT IS NOT CERTAIN RETURNS `Action::unknown()`. Read any edit
/// to this function against that rule: a new pattern is only worth adding if the
/// target is unambiguous from syntax, never from likelihood.
pub fn resolve(cmd: &str) -> Action {
    let cmd = cmd.trim();
    if cmd.is_empty() || is_compound(cmd) {
        return Action::unknown();
    }
    let w = words(cmd);
    let Some(argv0) = w.first() else {
        return Action::unknown();
    };
    // The binary, not the path it was invoked by.
    let prog = argv0.rsplit('/').next().unwrap_or(argv0);

    match prog {
        "ssh" | "scp" => {
            // The first operand that looks like a host. For scp this skips the
            // LOCAL path (a slash on the host side of the colon split) and
            // finds the remote spec, whichever side of the line it is on —
            // `scp a.bin host:/p` and `scp host:/p ./a.bin` both resolve.
            w.iter()
                .skip(1)
                .find_map(|x| host_operand(x))
                .map_or_else(Action::unknown, |h| Action::new(prog, h, TargetClass::Host))
        }
        "ansible" | "ansible-playbook" => {
            // An EXPLICIT limit only. `ansible <group>` without -l names a
            // group, not a host, and resolving a group to a host would invent
            // a target the operator never wrote.
            let mut it = w.iter().skip(1);
            while let Some(x) = it.next() {
                if *x == "-l" || *x == "--limit" {
                    if let Some(v) = it.next() {
                        return Action::new(prog, v.trim_matches(','), TargetClass::Host);
                    }
                    return Action::unknown();
                }
                if let Some(v) = x.strip_prefix("--limit=") {
                    return Action::new(prog, v.trim_matches(','), TargetClass::Host);
                }
            }
            Action::unknown()
        }
        "systemctl" => {
            // A known state verb plus a unit operand. `systemctl status` is a
            // read and still worth attributing; `systemctl` with no verb, or a
            // verb we do not know, abstains.
            let verbs = [
                "start", "stop", "restart", "reload", "enable", "disable", "mask", "unmask",
                "kill", "status",
            ];
            let mut rest = w.iter().skip(1).filter(|x| !x.starts_with('-'));
            let Some(verb) = rest.next() else {
                return Action::unknown();
            };
            if !verbs.contains(verb) {
                return Action::unknown();
            }
            rest.next().map_or_else(Action::unknown, |unit| {
                Action::new(
                    verb,
                    unit.trim_end_matches(".service"),
                    TargetClass::Service,
                )
            })
        }
        "git" => {
            // Only the forms that NAME a remote repo. A bare `git commit` acts
            // on the cwd, which yupana cannot resolve to a repo from the command
            // line alone — that is a different lookup and not this function's.
            let sub = w.get(1).copied().unwrap_or("");
            if !matches!(sub, "clone" | "push" | "pull" | "fetch") {
                return Action::unknown();
            }
            w.iter()
                .skip(2)
                .find(|x| !x.starts_with('-'))
                .and_then(|u| repo_name(u))
                .map_or_else(Action::unknown, |r| Action::new(sub, &r, TargetClass::Repo))
        }
        "pct" | "docker" | "podman" => {
            let verbs = [
                "start", "stop", "restart", "exec", "destroy", "rm", "kill", "reboot",
            ];
            let mut rest = w.iter().skip(1).filter(|x| !x.starts_with('-'));
            let Some(verb) = rest.next() else {
                return Action::unknown();
            };
            if !verbs.contains(verb) {
                return Action::unknown();
            }
            rest.next().map_or_else(Action::unknown, |id| {
                Action::new(verb, id, TargetClass::Container)
            })
        }
        _ => Action::unknown(),
    }
}

/// `git@host:owner/name.git` / `https://host/owner/name.git` / a path -> `name`.
/// Returns None for anything without a recognisable final segment.
fn repo_name(url: &str) -> Option<String> {
    let s = url.trim_end_matches('/').trim_end_matches(".git");
    let last = s.rsplit(['/', ':']).next()?;
    if last.is_empty() || last.contains(' ') {
        return None;
    }
    Some(last.to_string())
}

// The test names here CAPITALISE the word under test (…an_EXPLICIT_limit,
// …that_NAME_a_remote), which is this repo's house emphasis and is load-bearing
// for reading a failure line. rustc's non_snake_case fires on it, and CI runs
// clippy with `-D warnings`, so it was a hard error on the `mcp` leg — the only
// leg that compiles this module — and had been for as long as the names existed.
//
// ALLOW rather than rename: the capitals are the author's intent, and renaming
// to satisfy a lint would trade a readable failure line for a green tick. This
// is the narrowest scope that works (the test module, not the crate).
#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    fn r(cmd: &str) -> Action {
        resolve(cmd)
    }

    #[test]
    fn ssh_and_scp_resolve_a_host() {
        assert_eq!(
            r("ssh root@web.example"),
            Action::new("ssh", "web.example", TargetClass::Host)
        );
        assert_eq!(
            r("ssh -o BatchMode=yes braino@build.example"),
            Action::new("ssh", "build.example", TargetClass::Host)
        );
        assert_eq!(
            r("scp ./binary root@web.example:/opt/x"),
            Action::new("scp", "web.example", TargetClass::Host)
        );
    }

    #[test]
    fn ansible_resolves_only_an_EXPLICIT_limit() {
        assert_eq!(
            r("ansible-playbook -i inventory.ini site.yml --limit web.example"),
            Action::new("ansible-playbook", "web.example", TargetClass::Host)
        );
        assert_eq!(
            r("ansible all -i inventory.ini -l web.example -m ping"),
            Action::new("ansible", "web.example", TargetClass::Host)
        );
        // A GROUP is not a host. Resolving `quipu_servers` to a machine would
        // invent a target the operator never wrote — the exact guess this
        // module exists to refuse.
        assert!(!r("ansible quipu_servers -m ping").is_known());
    }

    #[test]
    fn systemctl_resolves_a_service_and_strips_the_suffix() {
        assert_eq!(
            r("systemctl restart quipu"),
            Action::new("restart", "quipu", TargetClass::Service)
        );
        assert_eq!(
            r("systemctl stop rsyslog.service"),
            Action::new("stop", "rsyslog", TargetClass::Service)
        );
        // An unknown verb abstains rather than treating the next word as a unit.
        assert!(!r("systemctl frobnicate quipu").is_known());
        assert!(!r("systemctl").is_known());
    }

    #[test]
    fn git_resolves_only_forms_that_NAME_a_remote() {
        assert_eq!(
            r("git clone https://github.com/scbrown/bobbin.git"),
            Action::new("clone", "bobbin", TargetClass::Repo)
        );
        assert_eq!(
            r("git push git@github.com:scbrown/shantytown.git main"),
            Action::new("push", "shantytown", TargetClass::Repo)
        );
        // `git commit` acts on the cwd. That is a different lookup and this
        // function must not pretend to do it.
        assert!(!r("git commit -m x").is_known());
    }

    #[test]
    fn containers_resolve_by_known_verb_plus_id() {
        assert_eq!(
            r("pct restart 236"),
            Action::new("restart", "236", TargetClass::Container)
        );
        assert_eq!(
            r("docker stop bobbin"),
            Action::new("stop", "bobbin", TargetClass::Container)
        );
        assert!(!r("docker ps").is_known());
    }

    #[test]
    fn a_COMPOUND_line_abstains_because_it_is_not_one_action() {
        // Each of these CONTAINS a resolvable command, and resolving the line to
        // that command's target would attribute the whole pipeline to it. The
        // observable command is not the whole action.
        for cmd in [
            "ssh root@web.example | tee log",
            "systemctl stop rsyslog && journalctl --vacuum-size=200M",
            "echo $(ssh root@web.example hostname)",
            "ssh root@web.example > out.txt",
        ] {
            assert!(!r(cmd).is_known(), "resolved a compound line: {cmd}");
        }
    }

    #[test]
    fn everything_unrecognised_abstains_rather_than_guessing() {
        for cmd in [
            "",
            "   ",
            "make deploy",
            "./scripts/deploy-cutover.sh /tmp/bobbin",
            "curl -s http://graph.example/query",
            "rm -rf /var/log/journal",
        ] {
            let a = r(cmd);
            assert!(!a.is_known(), "guessed a target for: {cmd}");
            assert_eq!(a.target_class, TargetClass::Unknown);
            assert!(a.target.is_none() && a.verb.is_none());
        }
    }

    #[test]
    fn unknown_serialises_as_a_word_not_an_empty_string() {
        // A blank target_class in a record reads as a real value to a replayer.
        assert_eq!(TargetClass::Unknown.as_str(), "unknown");
        assert!(!Action::unknown().is_known());
    }

    /// KNOWN-ANSWER TEST — the acceptance case on the bead.
    ///
    /// aegis-0jv06: the graph service was redeployed to web.example repeatedly and the audit
    /// trail could not say WHO, because every agent reaches that host as the
    /// same root over the same key. The question "would a record have
    /// identified the actor" splits in two, and only one half is this module's:
    /// the ACTOR comes from `agent` (already on every spool line), and the
    /// TARGET is what was missing. These are the command shapes that incident
    /// actually ran.
    #[test]
    fn replaying_the_0jv06_deploy_shapes_names_the_target_host() {
        let deploy_shapes = [
            "ansible-playbook -i inventory.ini site.yml --tags quipu --limit web.example",
            "scp target/release/quipu-server root@web.example:/opt/quipu/quipu-server",
            "ssh root@web.example systemctl restart quipu",
        ];
        for cmd in deploy_shapes {
            let a = resolve(cmd);
            assert!(a.is_known(), "deploy-shaped action did not resolve: {cmd}");
            assert_eq!(
                a.target.as_deref(),
                Some("web.example"),
                "wrong target for: {cmd}"
            );
            assert_eq!(a.target_class, TargetClass::Host);
        }
    }
}
