"""Read-only handoff advice from the last harness-reported request depth.

No default threshold: configuration must carry the operator's chosen threshold
and its evidence source. Missing configuration or signal means UNKNOWN, not low.
This command never cycles, clears, or blocks a session.
"""
import argparse
from datetime import datetime, timezone
import json
import math

from session_trajectory import is_compaction, records


def nonnegative_integer(value):
    return type(value) is int and value >= 0


def observed_time(value):
    if not isinstance(value, str):
        raise ValueError("missing measurement timestamp")
    time = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if time.tzinfo is None:
        raise ValueError("measurement timestamp has no timezone")
    return time.timestamp()


def measurement(path, harness, session_id):
    """Last request input, not cumulative billed tokens or transcript size.

    Claude splits input across uncached/cache-read/cache-write fields. Codex's
    last input_tokens already includes cached input; adding it would double-count.
    Neither adapter promises tokens accumulated since that request was measured.
    """
    latest = None
    codex_session = None
    for record in records(path):
        if record is None or is_compaction(record):
            latest = None
            continue
        usage = None
        if harness == "claude":
            if record.get("type") != "assistant":
                continue
            if record.get("sessionId") != session_id:
                latest = None
                continue
            message = record.get("message")
            usage = message.get("usage") if isinstance(message, dict) else None
            fields = ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")
        else:
            payload = record.get("payload")
            if record.get("type") == "session_meta":
                codex_session = payload.get("id") if isinstance(payload, dict) else None
                latest = None
                continue
            if (record.get("type") != "event_msg" or not isinstance(payload, dict)
                    or payload.get("type") != "token_count"):
                continue
            if codex_session != session_id:
                latest = None
                continue
            info = payload.get("info")
            usage = info.get("last_token_usage") if isinstance(info, dict) else None
            fields = ("input_tokens",)
        # Never fall back to an older good sample after a missing/broken one.
        latest = None
        if not isinstance(usage, dict) or not all(nonnegative_integer(usage.get(k)) for k in fields):
            continue
        try:
            timestamp = observed_time(record.get("timestamp"))
        except (ValueError, OverflowError):
            continue
        latest = {"context_tokens": sum(usage[k] for k in fields), "observed_at": timestamp}
    return latest


def evaluate(path, harness, session_id, config, now=None):
    """UNKNOWN never requests a handoff; a valid high reading advises only."""
    verdict = {"status": "UNKNOWN", "action": "DO_NOT_ACT", "harness": harness,
               "session_id": session_id, "basis": "last_request_input_tokens"}
    if harness not in ("claude", "codex") or not session_id:
        return {**verdict, "reason": "missing or unsupported harness/session identity"}
    try:
        sample = measurement(path, harness, session_id)
    except (OSError, UnicodeError, ValueError, TypeError):
        return {**verdict, "reason": "unreadable transcript"}
    if sample is None:
        return {**verdict, "reason": "missing valid depth since the last context boundary"}
    verdict.update(sample)
    try:
        policy = config["harnesses"][harness]
        threshold = policy["handoff_tokens"]
        max_age = policy["max_age_seconds"]
        evidence = policy["threshold_evidence"]
        if not nonnegative_integer(threshold) or threshold == 0:
            raise ValueError("invalid threshold")
        if type(max_age) not in (int, float) or not math.isfinite(max_age) or max_age <= 0:
            raise ValueError("invalid freshness window")
        if not isinstance(evidence, str) or not evidence.strip():
            raise ValueError("missing threshold evidence")
    except (KeyError, TypeError, ValueError):
        return {**verdict, "reason": "missing/invalid configured threshold, evidence, or freshness window"}
    if now is None:
        now = datetime.now(timezone.utc).timestamp()
    age = now - sample["observed_at"]
    verdict.update(age_seconds=age, handoff_tokens=threshold, threshold_evidence=evidence)
    if not math.isfinite(age) or age < 0 or age > max_age:
        return {**verdict, "reason": "stale or future-dated depth measurement"}
    high = sample["context_tokens"] >= threshold
    return {**verdict, "status": "HANDOFF_ADVISED" if high else "BELOW_THRESHOLD",
            "action": "CHECKPOINT_AND_HANDOFF" if high else "DO_NOT_ACT",
            "reason": "last measured request crosses threshold" if high else
                      "last measured request is below threshold; subsequent growth is unmeasured"}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("transcript")
    parser.add_argument("--harness", required=True, choices=("claude", "codex"))
    parser.add_argument("--session-id", required=True)
    parser.add_argument("--config", required=True)
    args = parser.parse_args(argv)
    try:
        with open(args.config, encoding="utf-8") as source:
            config = json.load(source)
    except (OSError, ValueError, UnicodeError):
        config = None
    result = evaluate(args.transcript, args.harness, args.session_id, config)
    print(json.dumps(result, allow_nan=False))
    return 2 if result["status"] == "UNKNOWN" else 0


if __name__ == "__main__":
    raise SystemExit(main())
