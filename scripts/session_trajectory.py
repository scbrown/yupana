"""Shared transcript boundaries for session advisories (never inference of retention)."""
import json


def records(path):
    """Yield records in file order; None marks unreadable evidence, never health."""
    with open(path, encoding="utf-8", errors="strict") as source:
        for line in source:
            try:
                record = json.loads(line)
            except (ValueError, TypeError):
                yield None
                continue
            yield record if isinstance(record, dict) else None


def is_compaction(record):
    """Recorded context boundaries invalidate earlier observations in both guards."""
    if record.get("isCompactSummary") or record.get("compact"):
        return True
    if record.get("type") == "compacted":
        return True
    if record.get("subtype") in ("compact_boundary", "compaction", "compact"):
        return True
    message = record.get("message")
    return isinstance(message, dict) and bool(message.get("isCompactSummary"))
