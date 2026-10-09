use super::*;

fn snapshot() -> Snapshot {
    let entries = [
        ("build-01.example", "Host", "ex:host"),
        ("bobbin-webhook", "SystemdService", "ex:svc"),
        ("bobbin-webhook-index", "Script", "ex:script"),
        ("preflight-monitoring-secrets", "Script", "ex:preflight"),
        ("quipu-recycle", "SystemdService", "ex:a1"),
        ("quipu-recycle", "AlertRule", "ex:a2"),
        ("issue-42", "Incident", "ex:issue-42"),
    ]
    .into_iter()
    .map(|(label, kind, iri)| Entry {
        label: label.into(),
        kind: kind.into(),
        iri: iri.into(),
    })
    .collect();
    Snapshot {
        generated_at: 100,
        entries,
        edges: vec![Edge {
            source: "ex:svc".into(),
            predicate: "https://example.org/runs_on".into(),
            target: "ex:host".into(),
        }],
    }
}

fn req(text: &str, remaining_bytes: usize) -> Request {
    Request {
        text: text.into(),
        session_id: "s".into(),
        reference: "notes.md".into(),
        remaining_bytes,
    }
}

#[test]
fn dp_precision_and_overlap_cases() {
    let index = KeywordIndex::compile(snapshot(), &BTreeSet::new()).unwrap();
    for (text, keys) in [
        (
            "deployed on BUILD-01.EXAMPLE today",
            vec!["build-01.example"],
        ),
        ("xbuild-01.example", vec![]),
        ("build-01.examplex", vec![]),
        ("pre-build-01.example", vec![]),
        (
            "restart bobbin-webhook-index now",
            vec!["bobbin-webhook-index"],
        ),
        ("run preflight-monitoring-secrets.sh first", vec![]),
        (
            "ran preflight-monitoring-secrets.",
            vec!["preflight-monitoring-secrets"],
        ),
        (
            "build-01.example bobbin-webhook build-01.example",
            vec!["build-01.example", "bobbin-webhook"],
        ),
    ] {
        assert_eq!(
            index.render(&req(text, 600), &BTreeSet::new()).shown,
            keys,
            "{text}"
        );
    }
    assert_eq!(
        index
            .render(&req("quipu-recycle", 600), &BTreeSet::new())
            .context,
        "Quipu entities here: quipu-recycle (ambiguous 2)"
    );
}

#[test]
fn stoplist_dictionary_and_identifier_precision() {
    let dict = ["review", "code", "server", "build"]
        .into_iter()
        .map(str::to_string)
        .collect();
    for (label, expected) in [
        ("build-01.example", true),
        ("build", false),
        ("abc", false),
        ("GitHub.com", false),
        ("code-review", false),
        ("deploy.yml", false),
        ("code review", false),
        ("quipu server", true),
        ("plainword", false),
        ("camelCase", true),
    ] {
        assert_eq!(admissible(label, &dict), expected, "{label}");
    }
}

#[test]
fn one_budget_bounds_entities_edges_and_utf8_bytes() {
    let index = KeywordIndex::compile(snapshot(), &BTreeSet::new()).unwrap();
    let text = "build-01.example bobbin-webhook quipu-recycle preflight-monitoring-secrets issue-42 bobbin-webhook-index";
    for budget in 0..=700 {
        let reply = index.render(&req(text, budget), &BTreeSet::new());
        assert!(reply.context.len() <= budget.min(600));
        assert!(reply.shown.len() <= 5);
    }
    let reply = index.render(
        &req("build-01.example bobbin-webhook", 600),
        &BTreeSet::new(),
    );
    assert!(reply
        .context
        .ends_with("; bobbin-webhook --runs_on--> build-01.example"));
    let reply = index.render(
        &req("build-01.example quipu-recycle", 600),
        &BTreeSet::new(),
    );
    assert!(
        !reply.context.contains("--"),
        "only observed, unambiguous relations"
    );
}

#[test]
fn session_dedup_and_atomic_label_replacement_are_shared_resident_data() {
    let resident = crate::daemon::keywords::ResidentKeywords::default();
    assert!(resident.query(&req("build-01.example", 600), 100).is_err());
    resident.replace(snapshot(), &BTreeSet::new()).unwrap();
    assert!(!resident
        .query(&req("build-01.example", 600), 100)
        .unwrap()
        .context
        .is_empty());
    assert!(resident
        .query(&req("build-01.example", 600), 100)
        .unwrap()
        .context
        .is_empty());
    let mut s = snapshot();
    s.entries[0].label = "build-02.example".into();
    resident.replace(s, &BTreeSet::new()).unwrap();
    assert!(!resident
        .query(&req("build-02.example", 600), 100)
        .unwrap()
        .context
        .is_empty());
    let mut r = req("build-01.example", 600);
    r.session_id = "s2".into();
    assert!(resident.query(&r, 100).unwrap().context.is_empty());
    assert!(resident.query(&r, 701).is_err());
    assert!(resident.query(&r, 99).is_err());
}

#[test]
fn self_exclusion_and_silent_calls_do_not_spend_dedup() {
    let resident = crate::daemon::keywords::ResidentKeywords::default();
    resident.replace(snapshot(), &BTreeSet::new()).unwrap();
    let mut r = req("issue-42 build-01.example", 0);
    r.reference = "issue-42".into();
    assert!(resident.query(&r, 100).unwrap().shown.is_empty());
    r.remaining_bytes = 600;
    assert_eq!(resident.query(&r, 100).unwrap().shown, ["build-01.example"]);
}
