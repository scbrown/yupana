//! Literal argv routing for shell execution wrappers. No input is executed.
use super::{Command, Parsed};

const MAX_DEPTH: usize = 16;

pub(super) fn expand(parsed: &mut Parsed, depth: usize) {
    let commands = std::mem::take(&mut parsed.commands);
    for command in commands {
        route(command, parsed, depth);
    }
}

fn route(mut command: Command, out: &mut Parsed, depth: usize) {
    if depth >= MAX_DEPTH {
        out.incomplete = true;
        out.unresolved.push(command);
        return;
    }
    let Some(program) = command.words.first() else {
        return;
    };
    let program = program.rsplit('/').next().unwrap_or(program).to_owned();
    let mut at = 1;
    match program.as_str() {
        "env" | "sudo" | "command" | "timeout" | "xargs" => {
            // xargs supplies argv from input; even a literal template is only
            // partial evidence. It must never certify a protected landing.
            out.incomplete |= program == "xargs";
            while let Some(word) = command.words.get(at) {
                if word == "--" {
                    at += 1;
                    break;
                }
                if program == "env" && word.contains('=') && !word.starts_with('-') {
                    at += 1;
                    continue;
                }
                if !word.starts_with('-') || word == "-" {
                    break;
                }
                // Introspection does not execute its operand.
                if (program == "command"
                    && word.starts_with('-')
                    && !word.starts_with("--")
                    && (word.contains('v') || word.contains('V')))
                    || (program == "sudo"
                        && matches!(word.as_str(), "-l" | "--list" | "-v" | "--validate"))
                    || matches!(word.as_str(), "--help" | "--version")
                {
                    return;
                }
                let attached_directory = if program == "env" {
                    word.strip_prefix("--chdir=")
                        .or_else(|| word.strip_prefix("-C").filter(|v| !v.is_empty()))
                } else if program == "sudo" {
                    word.strip_prefix("--chdir=")
                        .or_else(|| word.strip_prefix("-D").filter(|v| !v.is_empty()))
                } else {
                    None
                };
                if let Some(directory) = attached_directory {
                    let directory = directory.to_owned();
                    command.scope.extend([usize::MAX - 1, out.commands.len()]);
                    out.commands.push(Command {
                        text: command.text.clone(),
                        words: vec!["cd".into(), directory],
                        scope: command.scope.clone(),
                    });
                    at += 1;
                    continue;
                }
                if program == "env" && matches!(word.as_str(), "-S" | "--split-string") {
                    let Some(value) = command.words.get(at + 1) else {
                        out.incomplete = true;
                        out.unresolved.push(command);
                        return;
                    };
                    match shell_words::split(value) {
                        Ok(mut split) if !value.contains('$') => {
                            split.extend(command.words.iter().skip(at + 2).cloned());
                            command.words = split;
                            route(command, out, depth + 1);
                        }
                        _ => {
                            out.incomplete = true;
                            out.unresolved.push(command);
                        }
                    }
                    return;
                }
                if (program == "env" && matches!(word.as_str(), "-C" | "--chdir"))
                    || (program == "sudo" && matches!(word.as_str(), "-D" | "--chdir"))
                {
                    if let Some(directory) = command.words.get(at + 1) {
                        command.scope.extend([usize::MAX - 1, out.commands.len()]);
                        out.commands.push(Command {
                            text: command.text.clone(),
                            words: vec!["cd".into(), directory.clone()],
                            scope: command.scope.clone(),
                        });
                        at += 2;
                        continue;
                    }
                }
                let takes_value = match program.as_str() {
                    "env" => matches!(
                        word.as_str(),
                        "-u" | "--unset" | "-C" | "--chdir" | "-S" | "--split-string"
                    ),
                    "sudo" => matches!(
                        word.as_str(),
                        "-u" | "--user"
                            | "-g"
                            | "--group"
                            | "-h"
                            | "--host"
                            | "-p"
                            | "--prompt"
                            | "-C"
                            | "--close-from"
                            | "-T"
                            | "--command-timeout"
                            | "-D"
                            | "--chdir"
                            | "-R"
                            | "--chroot"
                    ),
                    "timeout" => matches!(word.as_str(), "-s" | "--signal" | "-k" | "--kill-after"),
                    "xargs" => matches!(
                        word.as_str(),
                        "-a" | "--arg-file"
                            | "-d"
                            | "--delimiter"
                            | "-E"
                            | "-I"
                            | "--replace"
                            | "-L"
                            | "--max-lines"
                            | "-n"
                            | "--max-args"
                            | "-P"
                            | "--max-procs"
                            | "-s"
                            | "--max-chars"
                    ),
                    _ => false,
                };
                let known_switch = match program.as_str() {
                    "env" => matches!(
                        word.as_str(),
                        "-i" | "--ignore-environment" | "-0" | "--null"
                    ),
                    "sudo" => matches!(
                        word.as_str(),
                        "-n" | "--non-interactive"
                            | "-E"
                            | "--preserve-env"
                            | "-H"
                            | "--set-home"
                            | "-S"
                            | "--stdin"
                    ),
                    "command" => word == "-p",
                    "timeout" => matches!(
                        word.as_str(),
                        "--preserve-status" | "--foreground" | "-v" | "--verbose"
                    ),
                    "xargs" => matches!(
                        word.as_str(),
                        "-0" | "--null" | "-r" | "--no-run-if-empty" | "-t" | "--verbose"
                    ),
                    _ => false,
                };
                if !takes_value && !known_switch {
                    out.incomplete = true;
                    out.unresolved.push(command.clone());
                }
                // Options that alter cwd or interpret another command string
                // are uncertain until their semantics have been resolved.
                if matches!(
                    word.as_str(),
                    "-C" | "--chdir"
                        | "-D"
                        | "-R"
                        | "--chroot"
                        | "-S"
                        | "--split-string"
                        | "-s"
                        | "-i"
                ) {
                    out.incomplete = true;
                    out.unresolved.push(command.clone());
                }
                at += if takes_value { 2 } else { 1 };
            }
            if program == "timeout" {
                at += 1;
            } // duration
            if at >= command.words.len() {
                out.incomplete = true;
                return;
            }
            command.words.drain(..at);
            route(command, out, depth + 1);
        }
        "bash" | "sh" | "eval" => {
            let code = if program == "eval" {
                command.words.get(1..).unwrap_or_default().join(" ")
            } else {
                let mut code = None;
                while let Some(word) = command.words.get(at) {
                    if word == "--" || !word.starts_with('-') {
                        break;
                    }
                    if matches!(word.as_str(), "--help" | "--version")
                        || (!word.starts_with("--") && word[1..].contains('n'))
                    {
                        return;
                    }
                    if matches!(word.as_str(), "-o" | "-O" | "--rcfile" | "--init-file") {
                        at += 2;
                        continue;
                    }
                    if !word.starts_with("--") && word[1..].contains('c') {
                        code = command.words.get(at + 1).cloned();
                        break;
                    }
                    at += 1;
                }
                let Some(code) = code else { return };
                code
            };
            let mut inner = super::parse_at_depth(&code, depth + 1);
            out.incomplete |= inner.incomplete;
            for mut unresolved in inner.unresolved {
                unresolved.scope = command.scope.clone();
                unresolved.text = command.text.clone();
                out.unresolved.push(unresolved);
            }
            let shell_scope = out.commands.len();
            for mut child in inner.commands.drain(..) {
                let mut scope = command.scope.clone();
                if program != "eval" {
                    // A shell's cd cannot change its caller. Use a namespace
                    // outside byte offsets for the synthetic execution scope.
                    scope.extend([usize::MAX, shell_scope]);
                }
                scope.extend(child.scope);
                child.scope = scope;
                child.text = command.text.clone();
                out.commands.push(child);
            }
        }
        _ => out.commands.push(command),
    }
}
