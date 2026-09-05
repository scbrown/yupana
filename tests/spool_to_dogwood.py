#!/usr/bin/env python3
"""Tests for scripts/spool-to-dogwood.py — the replay converter.

The property under test is not "it converts". It is that the DENOMINATOR
survives: a converter that silently discarded records would let one covering 5%
of traffic report the same false-positive rate as one covering 95%, and that
number is what gates the advise -> enforce promotion.
"""
from __future__ import annotations

import importlib.util
import io
import sys
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "spool_to_dogwood",
    Path(__file__).resolve().parent.parent / "scripts" / "spool-to-dogwood.py",
)
mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(mod)


def convert(lines):
    """Return (`.log` event lines, drop tally)."""
    out = io.StringIO()
    tally = mod.convert(lines, out)
    events = [line for line in out.getvalue().splitlines() if line.strip()]
    return events, tally


class ConversionTests(unittest.TestCase):
    def test_a_guard_record_becomes_one_edit_event(self):
        events, tally = convert([
            '{"kind":"guard","ts":1,"agent":"a","session":"s","item":"i","path":"src/a.rs"}'
        ])
        self.assertEqual(tally["written"], 1)
        self.assertIn('Yupana::Action::"Edit"::request', events[0])
        self.assertEqual(events[0].count('session: "s"'), 2)
        self.assertEqual(events[0].count('item: "i"'), 2)

    def test_exposure_and_repo_survive_replay_without_inventing_legacy_values(self):
        for exposure in ("public", "internal", "unknown", "n/a"):
            events, tally = convert([
                '{"kind":"guard","ts":1,"session":"s","rule":"r",'
                '"exposure":"%s","repo":"artifact"}' % exposure
            ])
            self.assertEqual(tally["written"], 1)
            self.assertIn('exposure: "%s"' % exposure, events[0])
            self.assertIn('repo: "artifact"', events[0])
        events, _ = convert(['{"kind":"guard","ts":1,"session":"s"}'])
        self.assertNotIn("exposure:", events[0])
        self.assertNotIn("repo:", events[0])

    def test_a_resolved_action_carries_its_verb(self):
        events, _ = convert([
            '{"kind":"action","ts":1,"agent":"a","session":"s","verb":"ssh",'
            '"target":"build-01","target_class":"host"}'
        ])
        self.assertIn('Yupana::Action::"Ssh"::request', events[0])
        self.assertIn('resource: Yupana::Target::"build-01"', events[0])


class DenominatorTests(unittest.TestCase):
    """Every drop is counted, and each reason is counted SEPARATELY."""

    def test_a_record_with_no_session_is_dropped_and_COUNTED(self):
        _, tally = convert(['{"kind":"guard","ts":1,"agent":"a","path":"src/a.rs"}'])
        self.assertEqual(tally["written"], 0)
        self.assertEqual(tally["no-session"], 1)

    def test_an_abstention_is_never_replayed_under_an_invented_verb(self):
        """ABSTAIN, NEVER GUESS, carried through from the resolver. A rule
        derived from a record whose verb nobody resolved would cite evidence
        that does not exist."""
        _, tally = convert([
            '{"kind":"action","ts":1,"agent":"a","session":"s","target_class":"unknown"}'
        ])
        self.assertEqual(tally["written"], 0)
        self.assertEqual(tally["resolver-abstained"], 1)

    def test_each_drop_reason_is_counted_separately(self):
        """'No session' and 'unparseable' need OPPOSITE fixes — one is a spool
        that predates a field, the other is corruption. A single `skipped`
        number would hide which is happening, which is the shape this repo
        refuses everywhere else."""
        _, tally = convert([
            '{"kind":"guard","ts":1,"agent":"a"}',                 # no session
            'not json',                                            # unparseable
            '{"kind":"fail_open","ts":1,"session":"s"}',           # not an action
            '{"kind":"guard","agent":"a","session":"s"}',          # no timestamp
        ])
        self.assertEqual(tally["no-session"], 1)
        self.assertEqual(tally["unparseable"], 1)
        self.assertEqual(tally["not-an-action"], 1)
        self.assertEqual(tally["no-timestamp"], 1)
        self.assertEqual(tally["written"], 0)

    def test_guard_state_records_are_not_replayed_as_agent_actions(self):
        """`fail_open`, `served_from_cache`, `scope` and `governed` describe the
        GUARD's own state, not something an agent did. Replaying them as actions
        would invent traffic that never happened and inflate the denominator a
        false-positive rate is measured against."""
        for kind in ("fail_open", "served_from_cache", "scope", "governed"):
            _, tally = convert([
                '{"kind":"%s","ts":1,"agent":"a","session":"s"}' % kind
            ])
            self.assertEqual(tally["written"], 0, kind)
            self.assertEqual(tally["not-an-action"], 1, kind)


class OmissionTests(unittest.TestCase):
    def test_an_unresolved_item_is_ABSENT_not_blank(self):
        """The spool's omit-never-fake rule, carried into the trace. A rule that
        keyed on item == "" would be reasoning about a value nobody wrote."""
        events, _ = convert([
            '{"kind":"guard","ts":1,"agent":"a","session":"s","path":"src/a.rs"}'
        ])
        self.assertNotIn("item:", events[0])

    def test_the_control_an_item_that_IS_present_appears(self):
        """Without this the assertion above would pass against a converter that
        had simply stopped emitting the field."""
        events, _ = convert([
            '{"kind":"guard","ts":1,"agent":"a","session":"s","item":"work-9"}'
        ])
        self.assertEqual(events[0].count('item: "work-9"'), 2)

    def test_strings_are_escaped_as_cedar_literals(self):
        events, _ = convert([
            '{"kind":"guard","ts":1,"agent":"a\\\"b","session":"s",'
            '"path":"src/a.rs"}'
        ])
        self.assertIn('Yupana::Agent::"a\\"b"', events[0])


if __name__ == "__main__":
    unittest.main()
