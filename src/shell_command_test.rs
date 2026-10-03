use super::*;

#[test]
fn quoted_heredoc_marker_does_not_hide_a_real_push() {
    let parsed = parse("echo 'text << EOF'\ngit push origin main");
    assert!(!parsed.incomplete);
    assert_eq!(parsed.commands.len(), 2);
    assert_eq!(parsed.commands[1].words, ["git", "push", "origin", "main"]);
}

#[test]
fn quoted_semicolon_is_data_but_pipeline_commands_are_selected() {
    let parsed = parse("echo 'text; git push origin main' | cat");
    assert!(!parsed.incomplete);
    assert_eq!(parsed.commands.len(), 2);
    assert_eq!(
        parsed.commands[0].words,
        ["echo", "text; git push origin main"]
    );
    assert_eq!(parsed.commands[1].words, ["cat"]);
}

#[test]
fn literal_heredoc_body_is_not_command_position() {
    let parsed = parse("cat <<'EOF'\ngit push origin main\nEOF\ngh pr merge 3");
    assert!(!parsed.incomplete);
    assert_eq!(parsed.commands.len(), 2);
    assert_eq!(parsed.commands[1].words, ["gh", "pr", "merge", "3"]);
}

#[test]
fn quoted_directory_and_assignment_prefix_keep_argument_boundaries() {
    let parsed = parse("cd 'a b' && X=1 git push origin main");
    assert!(!parsed.incomplete);
    assert_eq!(parsed.commands[0].words, ["cd", "a b"]);
    assert_eq!(parsed.commands[1].words, ["git", "push", "origin", "main"]);
}

#[test]
fn subshell_directory_changes_have_a_distinct_scope() {
    let parsed = parse("(cd elsewhere); git push origin main");
    assert!(!parsed.commands[0].scope.is_empty());
    assert!(parsed.commands[1].scope.is_empty());
}

#[test]
fn malformed_input_is_explicitly_incomplete() {
    assert!(parse("echo '").incomplete);
    let parsed = parse("git push origin main; echo '");
    assert!(parsed.incomplete);
    assert_eq!(parsed.commands[0].words, ["git", "push", "origin", "main"]);
}

#[test]
fn expansions_remain_uncertain_but_single_quoted_dollars_are_literal() {
    assert!(parse("git push \"$REMOTE\" main").incomplete);
    assert!(parse("git push \"$(echo origin)\" main").incomplete);
    assert!(!parse("echo '$REMOTE'").incomplete);
}
