//! The `yupana` binary entrypoint. See the `yupana` library crate and
//! `docs/yupana-spec.md` for the design.

use clap::error::ErrorKind;
use clap::Parser;

fn main() -> anyhow::Result<()> {
    // Parse before initializing tracing so `--verbose` can set the default
    // level. Clap writes its own errors (and `--help`/`--version`) straight to
    // the terminal, not through tracing, so nothing is lost by initializing it
    // second; the hook fail-open path in `parse_or_fail_open` exits before this
    // point and needs no subscriber.
    let cli = parse_or_fail_open();
    yupana::cli::init_tracing(cli.verbose());
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(cli.run())
}

/// Parse argv, but never let a *hook* invocation exit `2`.
///
/// Clap exits `2` on an argument error, and exit `2` is Claude Code's
/// fail-**closed** channel. The guard's whole contract is that no failure can
/// block an edit — but "argument didn't parse" is a failure clap resolves
/// before any of our code runs, so the fail-open logic inside the guard never
/// gets a say.
///
/// This is not hypothetical. A `yupana` predating `hook pre-edit` answers that
/// command with clap's "invalid value" error and exit `2` — so rolling out the
/// hook against a stale binary would block every `Edit`/`Write` in the fleet,
/// which is precisely the outcome the fail-open clause exists to prevent.
/// Absence fails open (exit `127`); it was *staleness* that failed closed.
///
/// So an unparseable hook invocation degrades to a silent allow. Every other
/// command keeps clap's ordinary behaviour, including `--help`/`--version`,
/// which clap reports as errors but which must still print and exit `0`.
fn parse_or_fail_open() -> yupana::cli::Cli {
    match yupana::cli::Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let display_only = matches!(
                e.kind(),
                ErrorKind::DisplayHelp
                    | ErrorKind::DisplayVersion
                    | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            );
            if !display_only && is_hook_invocation() {
                // stderr, not stdout: stdout is the permission channel, and a
                // guard that cannot parse its own arguments must not appear to
                // have decided anything.
                eprintln!(
                    "yupana: policy guard failed open: this yupana does not understand the \
                     requested hook invocation ({}). Upgrade yupana; edits are UNGUARDED \
                     until you do.",
                    e.kind_str()
                );
                std::process::exit(0);
            }
            e.exit()
        }
    }
}

/// Whether argv looks like `yupana hook …`.
///
/// Deliberately a loose match rather than positional parsing: the argument that
/// fails to parse may be the subcommand itself, so there is nothing structured
/// left to inspect. A false positive (say, a path literally named `hook`) only
/// costs a malformed command exit `0` instead of exit `2`; a false negative
/// would let the fail-closed exit through, which is the outcome that matters.
fn is_hook_invocation() -> bool {
    std::env::args_os().skip(1).any(|arg| arg == "hook")
}

/// Clap's [`ErrorKind`] has no stable [`Display`](std::fmt::Display); render a
/// short tag so the stderr line says *why* without dumping clap's full usage
/// block into the agent's debug log.
trait KindStr {
    fn kind_str(&self) -> String;
}

impl KindStr for clap::Error {
    fn kind_str(&self) -> String {
        format!("{:?}", self.kind())
    }
}
