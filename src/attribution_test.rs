//! Attribution-tuple tests. Size-exempt (`*_test.rs`).

use super::*;

fn field<'a>(fields: &'a [(&'static str, Value)], key: &str) -> Option<&'a Value> {
    fields.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

#[test]
fn an_undeclared_chain_is_absent_not_a_one_element_chain() {
    // The whole reason `P` stayed out of the record until a chain mechanism
    // existed: "[weaver]" asserted where a real chain belongs reads as "this
    // action had one principal" to the auditor the field exists for.
    let a = Attribution::compose(None, None, Some("weaver"), Some("Edit"));
    assert!(a.chain.is_empty());
    assert!(field(&a.fields(), "principal_chain").is_none());
    assert_eq!(field(&a.fields(), "executor").unwrap(), "weaver");
}

#[test]
fn a_declared_chain_is_recorded_caller_first() {
    let a = Attribution::compose(
        Some("orchestrator, worker"),
        None,
        Some("worker"),
        Some("Edit"),
    );
    assert_eq!(a.chain, vec!["orchestrator", "worker"]);
    let fields = a.fields();
    assert_eq!(
        field(&fields, "principal_chain").unwrap(),
        &Value::from(vec!["orchestrator", "worker"])
    );
}

#[test]
fn a_trailing_separator_does_not_mint_an_anonymous_link() {
    // An empty link would be a principal nobody can name, and it would silently
    // change the chain's length — which is what an intersection walks.
    assert_eq!(parse_chain("a, b,"), vec!["a", "b"]);
    assert_eq!(parse_chain(" , "), Vec::<String>::new());
    assert!(parse_chain("").is_empty());
}

#[test]
fn a_chain_whose_tail_is_not_the_executor_is_flagged() {
    // SARC §9.5 constraint laundering, made observable: the dispatch record says
    // one agent is acting and the running process is another.
    let a = Attribution::compose(
        Some("orchestrator,worker"),
        None,
        Some("someone-else"),
        Some("Edit"),
    );
    assert!(a.conflict);
    assert_eq!(
        field(&a.fields(), "attribution_conflict").unwrap(),
        &Value::Bool(true)
    );
}

#[test]
fn the_control_an_agreeing_chain_is_not_flagged() {
    // Without this the test above proves only that the flag can be set — not
    // that it means anything.
    let a = Attribution::compose(Some("orchestrator,worker"), None, Some("worker"), None);
    assert!(!a.conflict);
    assert!(field(&a.fields(), "attribution_conflict").is_none());
}

#[test]
fn an_undeclared_chain_never_conflicts() {
    // Flagging every unattributed edit would bury the real signal under the
    // ordinary case.
    let a = Attribution::compose(None, None, Some("weaver"), None);
    assert!(!a.conflict);
    let b = Attribution::compose(Some("weaver"), None, None, None);
    assert!(!b.conflict, "no executor to disagree with");
}

#[test]
fn the_planner_is_declared_never_derived_from_position() {
    // Reading planner off the chain's head would be an inference wearing a
    // record's clothes: which link deliberated is a fact about the dispatch.
    let a = Attribution::compose(Some("orchestrator,worker"), None, Some("worker"), None);
    assert!(
        a.planner.is_none(),
        "the head is not automatically a planner"
    );

    let b = Attribution::compose(
        Some("orchestrator,worker"),
        Some("orchestrator"),
        Some("worker"),
        None,
    );
    assert_eq!(field(&b.fields(), "planner").unwrap(), "orchestrator");
}

#[test]
fn authority_is_not_recorded_by_yupana() {
    // The effective authority is the INTERSECTION of every link's grant, the
    // grants live in quipu, and yupana cannot read them inside the pre-edit
    // budget. Recording `P` is what lets the checker derive `auth` from the
    // authoritative source; a locally-guessed `auth` would put a number in the
    // field the grant store never agreed to.
    let a = Attribution::compose(
        Some("orchestrator,worker"),
        None,
        Some("worker"),
        Some("Edit"),
    );
    assert!(
        field(&a.fields(), "auth").is_none(),
        "yupana must not assert an authority it did not compute"
    );
}

#[test]
fn an_empty_tuple_emits_nothing_rather_than_a_row_of_nulls() {
    let a = Attribution::compose(None, None, None, None);
    assert!(a.is_empty());
    assert!(a.fields().is_empty());
}

#[test]
fn a_tool_alone_is_still_worth_recording() {
    let a = Attribution::compose(None, None, None, Some("Write"));
    assert!(!a.is_empty());
    assert_eq!(field(&a.fields(), "tool").unwrap(), "Write");
}
