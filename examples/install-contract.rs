//! Compare a candidate executable with the CLI compiled from this source tree.
//! Recursive help checks include newly added nested verbs without a second list.
use clap::{CommandFactory, Parser};
use std::path::Path;
use std::process::Command;

fn main() -> anyhow::Result<()> {
    let candidate = std::env::args_os()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: install-contract <candidate>"))?;
    let count = verify(Path::new(&candidate))?;
    println!("Verified {count} command help surfaces against the compiled source");
    Ok(())
}

fn command_paths(command: &clap::Command, path: &[String], out: &mut Vec<Vec<String>>) {
    out.push(path.to_vec());
    for child in command.get_subcommands().filter(|c| c.get_name() != "help") {
        let mut next = path.to_vec();
        next.push(child.get_name().to_string());
        command_paths(child, &next, out);
    }
}

pub(crate) fn verify(candidate: &Path) -> anyhow::Result<usize> {
    let mut paths = Vec::new();
    command_paths(&yupana::cli::Cli::command(), &[], &mut paths);
    for path in &paths {
        let args: Vec<&str> = path.iter().map(String::as_str).chain(["--help"]).collect();
        let expected = yupana::cli::Cli::try_parse_from(
            std::iter::once(candidate.as_os_str()).chain(args.iter().map(std::ffi::OsStr::new)),
        )
        .expect_err("help must exit before executing a command")
        .to_string();
        let actual = Command::new(candidate)
            .args(&args)
            .env("NO_COLOR", "1")
            .output()?;
        anyhow::ensure!(
            actual.status.success() && actual.stdout == expected.as_bytes(),
            "candidate CLI differs from source at `yupana {}`",
            args.join(" ")
        );
    }
    Ok(paths.len())
}
