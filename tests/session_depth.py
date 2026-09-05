"""Discrimination for observed depth, signal loss and configuration loss."""
from contextlib import redirect_stdout
from datetime import datetime, timezone
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from session_depth import evaluate, main

STAMP = "2026-01-01T00:00:00Z"
NOW = datetime(2026, 1, 1, tzinfo=timezone.utc).timestamp()
# Synthetic test policy, explicitly not a recommended production threshold.
POLICY = {"handoff_tokens": 100, "max_age_seconds": 60, "threshold_evidence": "test fixture"}
CONFIG = {"harnesses": {"claude": POLICY, "codex": POLICY}}


def claude(tokens=100, **extra):
    return {"type": "assistant", "timestamp": STAMP, "sessionId": "test-session",
            "message": {"usage": {"input_tokens": 2, "cache_read_input_tokens": tokens - 3,
                                  "cache_creation_input_tokens": 1}}, **extra}


def codex(tokens=100):
    return [{"type": "session_meta", "payload": {"id": "test-session"}},
            {"type": "event_msg", "timestamp": STAMP,
             "payload": {"type": "token_count", "info": {
                 "last_token_usage": {"input_tokens": tokens, "cached_input_tokens": tokens - 3},
                 "total_token_usage": {"input_tokens": 999999}}}}]


class DepthTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / "session.jsonl"

    def run_case(self, records, harness="claude", config=CONFIG, now=NOW):
        self.path.write_text("".join(json.dumps(r) + "\n" for r in records))
        return evaluate(self.path, harness, "test-session", config, now)

    def unknown(self, result):
        self.assertEqual(result["status"], "UNKNOWN")
        self.assertEqual(result["action"], "DO_NOT_ACT")

    def test_claude_cache_fields_are_depth_not_lifetime_usage(self):
        result = self.run_case([claude(900), claude(100)])
        self.assertEqual(result["context_tokens"], 100)
        self.assertEqual(result["status"], "HANDOFF_ADVISED")

    def test_codex_cache_is_not_added_twice_or_cumulative(self):
        result = self.run_case(codex(99), harness="codex")
        self.assertEqual(result["context_tokens"], 99)
        self.assertEqual(result["status"], "BELOW_THRESHOLD")

    def test_threshold_is_configured_and_boundary_is_inclusive(self):
        for threshold, status in [(99, "HANDOFF_ADVISED"), (100, "HANDOFF_ADVISED"),
                                  (101, "BELOW_THRESHOLD")]:
            with self.subTest(threshold=threshold):
                config = {"harnesses": {"claude": {**POLICY, "handoff_tokens": threshold}}}
                self.assertEqual(self.run_case([claude()], config=config)["status"], status)

    def test_missing_and_malformed_threshold_have_no_default(self):
        for config in (None, {}, [], {"harnesses": {"claude": None}}):
            with self.subTest(config=config):
                self.unknown(self.run_case([claude()], config=config))
        for field, values in {"handoff_tokens": [None, 0, -1, True, "100"],
                              "max_age_seconds": [None, 0, float("nan"), float("inf"), True],
                              "threshold_evidence": [None, "", " "]}.items():
            for value in values:
                with self.subTest(field=field, value=value):
                    policy = {**POLICY, field: value}
                    self.unknown(self.run_case([claude()], config={"harnesses": {"claude": policy}}))

    def test_missing_stale_future_and_invalid_time_are_unknown(self):
        self.unknown(self.run_case([]))
        self.unknown(evaluate(self.path.with_name("absent"), "claude", "test-session", CONFIG, NOW))
        for now in (NOW + 61, NOW - 1):
            self.unknown(self.run_case([claude()], now=now))
        for timestamp in (None, "broken", "2026-01-01T00:00:00"):
            self.unknown(self.run_case([claude(timestamp=timestamp)]))

    def test_invalid_latest_usage_does_not_reuse_old_high(self):
        for usage in (None, {}, {"input_tokens": 100}, {"input_tokens": True}):
            self.unknown(self.run_case([claude(), claude(message={"usage": usage})]))
        events = codex()
        events.append({"type": "event_msg", "payload": {"type": "token_count", "info": None}})
        self.unknown(self.run_case(events, harness="codex"))

    def test_compaction_requires_a_new_measurement_both_harnesses(self):
        for marker in ({"isCompactSummary": True}, {"subtype": "compact_boundary"},
                       {"type": "compacted"}):
            self.unknown(self.run_case([claude(), marker]))
            self.unknown(self.run_case(codex() + [marker], harness="codex"))
            self.assertEqual(self.run_case([claude(), marker, claude(50)])["status"], "BELOW_THRESHOLD")

    def test_wrong_session_cannot_license_handoff(self):
        self.unknown(self.run_case([claude(), claude(sessionId="other")]))
        events = codex()
        events[0]["payload"]["id"] = "other"
        self.unknown(self.run_case(events, harness="codex"))
        self.unknown(self.run_case(codex()[1:], harness="codex"))

    def test_malformed_tail_invalidates_a_good_reading(self):
        self.run_case([claude()])
        with self.path.open("a") as stream:
            stream.write('{"partial":')
        self.unknown(evaluate(self.path, "claude", "test-session", CONFIG, NOW))

    def test_cli_missing_config_is_loud_json_and_nonzero(self):
        self.run_case([claude()])
        output = io.StringIO()
        with redirect_stdout(output):
            rc = main([str(self.path), "--harness", "claude", "--session-id", "test-session",
                       "--config", str(self.path.with_name("absent.json"))])
        self.assertEqual(rc, 2)
        self.unknown(json.loads(output.getvalue()))


if __name__ == "__main__":
    unittest.main()
