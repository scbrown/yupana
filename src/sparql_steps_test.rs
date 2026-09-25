use super::*;

#[test]
fn full_iri_expands_the_aegis_prefix_and_leaves_absolute_iris_alone() {
    let absolute = format!("{}aegis-l50p", crate::export::ONTO);
    assert_eq!(full_iri("aegis:aegis-l50p"), absolute);
    assert_eq!(full_iri(&absolute), absolute);
    assert_eq!(full_iri("urn:x:y"), "urn:x:y");
}

#[test]
fn bound_union_binds_each_subject_expands_prefixes_and_drops_unsafe_values() {
    let union = bound_union(
        &[
            "aegis:a".to_string(),
            "http://x/b".to_string(),
            "bad iri".to_string(),
            "evil>".to_string(),
            String::new(),
        ],
        "c",
        "?c aegis:modifies ?e",
    );
    let a = format!("<{}a>", crate::export::ONTO);
    assert_eq!(
        union,
        format!(
            "{{ BIND({a} AS ?c) {a} aegis:modifies ?e }} UNION \
             {{ BIND(<http://x/b> AS ?c) <http://x/b> aegis:modifies ?e }}"
        )
    );
}

#[test]
fn bound_union_replaces_the_variable_only_as_a_whole_term() {
    // `?c` must not rewrite the prefix of `?c2`.
    let union = bound_union(&["http://x/a".to_string()], "c", "?c2 aegis:implements ?c");
    assert_eq!(
        union,
        "{ BIND(<http://x/a> AS ?c) ?c2 aegis:implements <http://x/a> }"
    );
}

#[test]
fn literal_escapes_quotes_backslashes_and_newlines() {
    assert_eq!(literal("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    // A dotted child id is an ordinary literal: the dot must survive.
    assert_eq!(literal("aegis-4hhqoe.3"), "aegis-4hhqoe.3");
}
