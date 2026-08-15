#!/usr/bin/env python3
"""End-to-end eval harness for the Yupana <-> Quipu grounding integration.

Stands up a real quipu-server on a seeded store, points a real yupana
pre-edit guard at it, and drives the hallucination-prevention scenarios
from the grounding cluster disclosure (camayoc
docs/patents/provisional-grounding-cluster.md, aspects 19 and 23-26):

  S1  structural hallucination — an edit calling an identifier that exists
      nowhere in the composed graph is DENIED at a declared tier, with the
      unchecked violation classes enumerated (aspect 19)
  S2  clean, cited edit — allowed silently (the guard must not cry wolf)
  S3  fabricated work-item reference — an id-shaped citation resolving to
      no WorkItem in quipu is DENIED as its own violation class (aspect 26)
  S4  uncited edit — the must-ground discipline denies untracked work
  S5  verdict freshness — a fresh projection says so in the verdict body
      (aspect 25/16)
  S6  quipu down, cache warm — the guard still ENFORCES last-known policy
      and DECLARES the cache age in the verdict (aspect 24)
  S7  quipu down, no cache — the guard fails OPEN with a loud notice,
      never a silent allow (typed non-answer, aspect 18)
  S8  verdict return — spooled verdicts drain into quipu as signed facts
      carrying tier + projection freshness (aspect 25)

Every scenario's stdout/stderr, the metrics spool, the verdict spool, and
quipu's /metrics are captured under --workdir; the eval report (JSON +
markdown) scores expected-vs-actual per scenario and exits nonzero on any
failure.

Usage: scripts/e2e/harness.py [--workdir DIR] [--keep] [--profile release]
Run via `just e2e run`.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

YUPANA_ROOT = Path(__file__).resolve().parents[2]
QUIPU_ROOT = YUPANA_ROOT.parent / "quipu"
CAMAYOC_ROOT = YUPANA_ROOT.parent / "camayoc"
ENDPOINT = "http://127.0.0.1:3041"  # not 3030: leave the default free for dev
BIND = "127.0.0.1:3041"

REAL_ITEM = "aegis-e2e01"
REAL_ITEM_2 = "aegis-r34l"
FAKE_ITEM = "aegis-zz999"  # id-shaped (shares the data-derived prefix), unreal
PRIOR_ITEM = "aegis-pr10r"  # finished (outcome done), semantically similar

WORKITEMS_TTL = f"""\
@prefix aegis: <http://aegis.gastown.local/ontology/> .
@prefix rdfs:  <http://www.w3.org/2000/01/rdf-schema#> .

aegis:wi_e2e_grounding a aegis:WorkItem ;
    rdfs:label "e2e: prove the grounded edit boundary" ;
    aegis:identifier "{REAL_ITEM}" ;
    aegis:sourceKind "declared" .

aegis:wi_e2e_bench a aegis:WorkItem ;
    rdfs:label "e2e: shared-yupana concurrency bench" ;
    aegis:identifier "{REAL_ITEM_2}" ;
    aegis:sourceKind "declared" .

# A finished item with a SIMILAR description — the reuse exemplar the
# briefing must surface ("successful prior work: study and reuse").
aegis:wi_e2e_prior a aegis:WorkItem ;
    rdfs:label "prove the grounded edit boundary for tallies" ;
    aegis:identifier "{PRIOR_ITEM}" ;
    aegis:outcome "done" ;
    aegis:sourceKind "declared" .

# The provenance chain the OBSERVED scope rung projects
# (Bead <-implements- Commit -modifies-> entity, quipu shapes/provenance.ttl):
# prior work on {REAL_ITEM} touched exactly src/lib.rs.
aegis:ent_e2e_lib a aegis:CodeModule ;
    rdfs:label "lib.rs" ;
    aegis:filePath "src/lib.rs" .

aegis:commit_e2e_1 a aegis:GitCommit ;
    aegis:implements aegis:wi_e2e_grounding ;
    aegis:modifies aegis:ent_e2e_lib .

aegis:commit_e2e_2 a aegis:GitCommit ;
    aegis:implements aegis:wi_e2e_prior ;
    aegis:modifies aegis:ent_e2e_lib .
"""

FIXTURE_LIB = """\
//! e2e fixture crate — the small, known world the guard verifies against.

/// Sum of the first `n` naturals.
pub fn tally(n: u32) -> u32 {
    (1..=n).sum()
}

/// Interleave two strands.
pub fn weave(a: &str, b: &str) -> String {
    format!("{a}{b}")
}

pub fn caller() -> u32 {
    tally(3)
}
"""

FIXTURE_CONFIG_RS = """\
//! e2e fixture: a second module, deliberately untouched by any prior work.

pub const LIMIT: u32 = 10;
"""

FIXTURE_README = "# e2e fixture\n\nThe small, known world the guard verifies against.\n"


def bobbin_config(work_item_scope: str | None = None, declared_scope: bool = False) -> str:
    """The fixture repo's `.bobbin/config.toml`, staged per scenario group."""
    text = f"""\
[yupana]
base_ref = "main"

[yupana.policy]
mode = "enforce"
deadline_ms = 15000
verify = true
"""
    if work_item_scope:
        text += f'work_item_scope = "{work_item_scope}"\n'
    if declared_scope:
        text += """
[yupana.policy.scopes.polecat-declared]
allow_paths = ["src/**"]
"""
    text += f"""
[yupana.quipu]
enabled = true
endpoint = "{ENDPOINT}"
projection_cache_ttl_secs = 3600

[yupana.metrics]
record_paths = "relative"
"""
    return text


def log(msg: str) -> None:
    print(f"[e2e] {msg}", flush=True)


class Rig:
    """The running pair: seeded quipu-server + a configured yupana workspace."""

    def __init__(self, work: Path, profile: str):
        # Session ids are salted per run: the guard's once-per-session notices
        # persist as marker files in the system temp dir, so a reused session
        # id would swallow notices on the second harness run.
        self.nonce = str(int(time.time()))
        self.work = work
        self.repo = work / "repo"
        self.state = work / "state"
        self.logs = work / "logs"
        self.db = work / "store.db"
        self.server: subprocess.Popen | None = None
        target = "release" if profile == "release" else "debug"
        self.yupana = YUPANA_ROOT / "target" / target / "yupana"
        self.quipu = QUIPU_ROOT / "target" / target / "quipu"
        self.quipu_server = QUIPU_ROOT / "target" / target / "quipu-server"
        for binary in (self.yupana, self.quipu, self.quipu_server):
            if not binary.exists():
                sys.exit(f"missing binary {binary} — build it first (see justfile e2e)")

    # -- environment ---------------------------------------------------------

    def hook_env(self) -> dict[str, str]:
        env = dict(os.environ)
        env.update(
            YUPANA_METRICS_PATH=str(self.state / "metrics.jsonl"),
            YUPANA_VERDICT_PATH=str(self.state / "verdicts.jsonl"),
            YUPANA_PROJECTION_CACHE_PATH=str(self.state / "projection-cache.json"),
            RUST_LOG="debug",
        )
        return env

    def setup(self) -> None:
        for d in (self.repo / "src", self.state, self.logs):
            d.mkdir(parents=True, exist_ok=True)
        (self.repo / "src" / "lib.rs").write_text(FIXTURE_LIB)
        (self.repo / "src" / "config.rs").write_text(FIXTURE_CONFIG_RS)
        (self.repo / "README.md").write_text(FIXTURE_README)
        (self.repo / "Cargo.toml").write_text(
            '[package]\nname = "e2e-fixture"\nversion = "0.1.0"\nedition = "2021"\n'
        )
        (self.repo / ".bobbin").mkdir(exist_ok=True)
        self.write_config()
        run(["git", "init", "-q", "-b", "main"], cwd=self.repo)
        run(["git", "add", "-A"], cwd=self.repo)
        run(
            ["git", "-c", "user.email=e2e@local", "-c", "user.name=e2e", "commit", "-qm", "fixture"],
            cwd=self.repo,
        )

    def write_config(self, work_item_scope: str | None = None, declared_scope: bool = False) -> None:
        (self.repo / ".bobbin" / "config.toml").write_text(
            bobbin_config(work_item_scope, declared_scope)
        )

    def session_start(
        self, name: str, tenant: str, agent: str, extra_env: dict | None = None
    ) -> dict:
        """Invoke `yupana hook session-start` — the assignment-time briefing."""
        env = self.hook_env()
        env["BOBBIN_ROLE"] = tenant
        env["SHANTY_ROOT"] = str(self.work / "shanty")
        env["SHANTY_AGENT"] = agent
        env.update(extra_env or {})
        payload = json.dumps({"session_id": f"{name}-{self.nonce}", "cwd": str(self.repo)})
        started = time.time()
        proc = subprocess.run(
            [self.yupana, "hook", "session-start"],
            input=payload,
            capture_output=True,
            text=True,
            cwd=self.repo,
            env=env,
            timeout=120,
        )
        elapsed_ms = (time.time() - started) * 1000
        (self.logs / f"{name}.stdout.json").write_text(proc.stdout)
        (self.logs / f"{name}.stderr.log").write_text(proc.stderr)
        context = ""
        if proc.stdout.strip():
            body = json.loads(proc.stdout)
            hso = body.get("hookSpecificOutput", {})
            if hso.get("hookEventName") == "SessionStart":
                context = hso.get("additionalContext", "")
        return {
            "name": name,
            "outcome": "briefing" if context else "silent",
            "reason": context,
            "stderr": proc.stderr,
            "exit": proc.returncode,
            "ms": round(elapsed_ms, 1),
        }

    def publish_plate(self, agent: str, item: str) -> None:
        """Publish the tracker's plate: `agent` is working `item` right now."""
        crew = self.work / "shanty" / "crew" / agent
        crew.mkdir(parents=True, exist_ok=True)
        (crew / "plate.json").write_text(
            json.dumps({"item": item, "at": int(time.time()), "session": None})
        )

    def seed(self) -> None:
        """Load the camayoc policy pack, the work items, and the verifier key
        into the store — all BEFORE the server starts, via the quipu CLI."""
        pack = CAMAYOC_ROOT / "shapes" / "policies" / "edit-grounding.ttl"
        if not pack.exists():
            sys.exit(f"camayoc policy pack not found at {pack}")
        items = self.work / "workitems.ttl"
        items.write_text(WORKITEMS_TTL)
        # cwd is always the workdir: some subcommands drop state (e.g. a
        # default signing key) relative to their cwd, and that must never
        # land in a checkout.
        run([self.quipu, "knot", str(pack), "--db", str(self.db)], cwd=self.work)
        run([self.quipu, "knot", str(items), "--db", str(self.db)], cwd=self.work)

        # Mint (or reuse) yupana's signing identity and register its public key
        # — the human act the VerifierRegistration deliberately leaves out.
        key = self.repo / "yupana-signing.pk8"
        out = run([self.yupana, "verifier", "--key-path", str(key)], cwd=self.work).stdout
        match = re.search(r"public_key:\s*([0-9a-f]+)", out)
        if not match:
            sys.exit(f"could not read public key from `yupana verifier`:\n{out}")
        reg = self.work / "verifier-key.ttl"
        reg.write_text(
            "@prefix aegis: <http://aegis.gastown.local/ontology/> .\n"
            f'aegis:reg_yupana_grounding aegis:publicKey "{match.group(1)}" .\n'
        )
        run([self.quipu, "knot", str(reg), "--db", str(self.db)], cwd=self.work)

    # -- server lifecycle ----------------------------------------------------

    def start_server(self) -> None:
        log_file = open(self.logs / "quipu-server.log", "a")
        # cwd matters: quipu-server mints its signing identity (.quipu/) in
        # its cwd, and that state belongs in the workdir, not this checkout.
        self.server = subprocess.Popen(
            [self.quipu_server, "--db", str(self.db), "--bind", BIND],
            stdout=log_file,
            stderr=subprocess.STDOUT,
            cwd=self.work,
        )
        deadline = time.time() + 30
        while time.time() < deadline:
            try:
                with urllib.request.urlopen(f"{ENDPOINT}/health", timeout=1):
                    log(f"quipu-server up at {ENDPOINT}")
                    return
            except (urllib.error.URLError, ConnectionError, OSError):
                time.sleep(0.3)
        sys.exit("quipu-server did not become healthy in 30s")

    def stop_server(self) -> None:
        if self.server:
            self.server.terminate()
            try:
                self.server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.server.kill()
                self.server.wait()
            self.server = None

    def query(self, sparql: str) -> dict:
        req = urllib.request.Request(
            f"{ENDPOINT}/query",
            data=json.dumps({"query": sparql}).encode(),
            headers={
                "Content-Type": "application/json",
                "Accept": "application/sparql-results+json",
            },
        )
        with urllib.request.urlopen(req, timeout=10) as resp:
            return json.load(resp)

    def scrape_metrics(self, name: str) -> None:
        try:
            with urllib.request.urlopen(f"{ENDPOINT}/metrics", timeout=5) as resp:
                (self.logs / f"quipu-metrics-{name}.prom").write_bytes(resp.read())
        except (urllib.error.URLError, OSError):
            pass

    # -- the guard -----------------------------------------------------------

    def guard(
        self,
        name: str,
        old: str,
        new: str,
        session: str,
        file: str = "src/lib.rs",
        tenant: str | None = None,
        agent: str | None = None,
        spool: str | None = None,
    ) -> dict:
        """Invoke `yupana hook pre-edit` the way the agent harness would.

        `tenant` becomes `BOBBIN_ROLE` (the capability identity); `agent`
        wires `SHANTY_ROOT`/`SHANTY_AGENT` so the guard can read the
        tracker's plate for the work-item scope ladder.
        """
        payload = json.dumps(
            {
                "session_id": f"{session}-{self.nonce}",
                "cwd": str(self.repo),
                "tool_name": "Edit",
                "tool_input": {
                    "file_path": str(self.repo / file),
                    "old_string": old,
                    "new_string": new,
                },
            }
        )
        env = self.hook_env()
        if tenant:
            env["BOBBIN_ROLE"] = tenant
        if agent:
            env["SHANTY_ROOT"] = str(self.work / "shanty")
            env["SHANTY_AGENT"] = agent
        if spool:
            # A scenario-private verdict spool: keeps the recurrence advisory
            # (which mines the spool for similar DENIED edits) from coloring
            # scenarios that assert on other planes.
            env["YUPANA_VERDICT_PATH"] = str(self.state / f"verdicts-{spool}.jsonl")
        started = time.time()
        proc = subprocess.run(
            [self.yupana, "hook", "pre-edit"],
            input=payload,
            capture_output=True,
            text=True,
            cwd=self.repo,
            env=env,
            timeout=120,
        )
        elapsed_ms = (time.time() - started) * 1000
        (self.logs / f"{name}.stdout.json").write_text(proc.stdout)
        (self.logs / f"{name}.stderr.log").write_text(proc.stderr)
        outcome, reason = "allow", ""
        if proc.stdout.strip():
            body = json.loads(proc.stdout)
            if "hookSpecificOutput" in body:
                outcome = body["hookSpecificOutput"]["permissionDecision"]
                reason = body["hookSpecificOutput"]["permissionDecisionReason"]
            elif "systemMessage" in body:
                outcome = "notify"
                reason = body["systemMessage"]
        return {
            "name": name,
            "outcome": outcome,
            "reason": reason,
            "stderr": proc.stderr,
            "exit": proc.returncode,
            "ms": round(elapsed_ms, 1),
        }


def run(cmd: list, cwd: Path | None = None) -> subprocess.CompletedProcess:
    proc = subprocess.run(
        [str(c) for c in cmd], cwd=cwd, capture_output=True, text=True, timeout=300
    )
    if proc.returncode != 0:
        sys.exit(
            f"command failed ({proc.returncode}): {' '.join(str(c) for c in cmd)}\n"
            f"stdout: {proc.stdout}\nstderr: {proc.stderr}"
        )
    return proc


# -- scenarios ---------------------------------------------------------------

ANCHOR = "pub fn caller() -> u32 {\n    tally(3)\n}"


def scenarios(rig: Rig) -> list[dict]:
    checks: list[dict] = []

    def check(result: dict, expect: str, *must_contain: str, aspect: str) -> None:
        haystack = result["reason"] + result["stderr"]
        missing = [m for m in must_contain if m.lower() not in haystack.lower()]
        ok = result["outcome"] == expect and not missing and result["exit"] == 0
        checks.append(
            {
                **result,
                "expected": expect,
                "must_contain": list(must_contain),
                "missing": missing,
                "aspect": aspect,
                "ok": ok,
            }
        )
        status = "PASS" if ok else "FAIL"
        log(f"  {status} {result['name']}: {result['outcome']} ({result['ms']}ms)")
        if not ok:
            log(f"       expected {expect}, missing {missing}")
            log(f"       reason: {result['reason'][:400]}")

    # S2 — clean, cited edit: calls an existing symbol, cites a real item.
    # Runs FIRST: the recurrence advisory mines the verdict spool for similar
    # DENIED edits, so the clean edit must be judged before any denial exists
    # to resemble.
    check(
        rig.guard(
            "s2-clean-cited",
            ANCHOR,
            "pub fn caller() -> u32 {\n"
            f"    // implements {REAL_ITEM}\n"
            "    tally(4) + weave(\"a\", \"b\").len() as u32\n"
            "}",
            session="e2e-s2",
        ),
        "allow",
        aspect="19: a bounded pass — the guard does not cry wolf",
    )

    # S1 — structural hallucination: a call to a function that exists nowhere.
    check(
        rig.guard(
            "s1-hallucinated-call",
            ANCHOR,
            "pub fn caller() -> u32 {\n"
            f"    // implements {REAL_ITEM}\n"
            "    frobnicate_the_widget(3, 4)\n"
            "}",
            session="e2e-s1",
        ),
        "deny",
        "frobnicate_the_widget",
        "tree-sitter",
        aspect="19: nonexistent reference rejected at a declared tier",
    )

    # S3 — fabricated work-item reference: id-shaped, resolves to nothing.
    check(
        rig.guard(
            "s3-fabricated-ref",
            ANCHOR,
            "pub fn caller() -> u32 {\n"
            f"    // implements {FAKE_ITEM}\n"
            "    tally(5)\n"
            "}",
            session="e2e-s3",
        ),
        "deny",
        "FABRICATED REFERENCE",
        FAKE_ITEM,
        aspect="26: fabricated reference is its own violation class",
    )

    # S4 — uncited edit: must-ground demands a tracked work item.
    s4 = rig.guard(
        "s4-uncited",
        ANCHOR,
        "pub fn caller() -> u32 {\n    tally(6)\n}",
        session="e2e-s4",
    )
    check(
        s4,
        "deny",
        "must reference a tracked work item",
        aspect="23: projected governed policy enforced synchronously",
    )

    # S5 — freshness declared on a live projection.
    check(
        {**s4, "name": "s5-freshness-fresh"},
        "deny",
        "verdict freshness: fresh",
        aspect="16/25: projection freshness declared, never fabricated",
    )

    # ---- Work-item scope isolation (the provenance ladder) -----------------

    # S9 — DECLARED rung: the static scope table hard-denies outside itself.
    rig.write_config(declared_scope=True)
    check(
        rig.guard(
            "s9-declared-deny",
            "The small, known world",
            f"The small, known world (see {REAL_ITEM})",
            session="e2e-s9",
            file="README.md",
            tenant="polecat-declared",
            spool="s9",
        ),
        "deny",
        "outside the writable capability scope",
        aspect="21: declared scope constrains — deny outside the boundary",
    )

    # S10 — OBSERVED rung, in scope: prior work on the item touched src/lib.rs,
    # so the item's own ground admits the edit silently.
    rig.write_config(work_item_scope="advise")
    rig.publish_plate("polecat-obs", REAL_ITEM)
    cited_clean = (
        "pub fn caller() -> u32 {\n"
        f"    // implements {REAL_ITEM}\n"
        "    weave(\"warp\", \"weft\").len() as u32\n"
        "}"
    )
    check(
        rig.guard(
            "s10-observed-in-scope",
            ANCHOR,
            cited_clean,
            session="e2e-s10",
            tenant="polecat-obs",
            agent="polecat-obs",
            spool="s10",
        ),
        "allow",
        aspect="21: observed scope admits the item's own ground",
    )

    # S11 — OBSERVED rung, out of scope, at advise: the INFLUENCE half — the
    # guard names the item, its paths, and the right move, without blocking.
    check(
        rig.guard(
            "s11-observed-advise",
            "pub const LIMIT: u32 = 10;",
            f"// implements {REAL_ITEM}\npub const LIMIT: u32 = 11;",
            session="e2e-s11",
            file="src/config.rs",
            tenant="polecat-obs",
            agent="polecat-obs",
            spool="s11",
        ),
        "notify",
        "OBSERVED",
        "update your tracked item",
        aspect="21: observed scope advises — guidance without constraint",
    )

    # S12 — UNKNOWN scope: an identified tenant with no scope on any rung is
    # told once per session, never silently read as within-scope.
    check(
        rig.guard(
            "s12-unknown-scope",
            ANCHOR,
            cited_clean,
            session="e2e-s12",
            tenant="polecat-lost",
            spool="s12",
        ),
        "notify",
        "UNGUARDED by scope",
        aspect="21: unknown scope advises rather than blocks",
    )
    check(
        rig.guard(
            "s12b-unknown-quiet",
            ANCHOR,
            cited_clean,
            session="e2e-s12",
            tenant="polecat-lost",
            spool="s12",
        ),
        "allow",
        aspect="21: the unknown-scope advisory fires once per session",
    )

    # S13 — OBSERVED rung at enforce: the CONSTRAINT half — the same boundary
    # now denies, and the deny still teaches the right move.
    rig.write_config(work_item_scope="enforce")
    check(
        rig.guard(
            "s13-observed-enforce-deny",
            "pub const LIMIT: u32 = 10;",
            f"// implements {REAL_ITEM}\npub const LIMIT: u32 = 12;",
            session="e2e-s13",
            file="src/config.rs",
            tenant="polecat-obs",
            agent="polecat-obs",
            spool="s13",
        ),
        "deny",
        "OBSERVED",
        "update your tracked item",
        aspect="21+: work_item_scope=enforce holds the item's boundary",
    )

    # S15 — the assignment-time briefing: before any edit, the agent is handed
    # the item's ground, the reusable SUCCESSFUL similar item, the central
    # entity, the enforce posture, and the citation rules — context BEFORE the
    # first mistake, via the suite (scope projection, /context, /project).
    check(
        rig.session_start("s15-briefing", tenant="polecat-obs", agent="polecat-obs"),
        "briefing",
        REAL_ITEM,
        "src/lib.rs",
        "SUCCESSFUL prior work",
        PRIOR_ITEM,
        "Central entities",
        "edit-cites-work-item",
        "DENIED",
        aspect="context: the graph briefs the agent at assignment time",
    )

    # S14 — and enforce does not over-block the item's own ground.
    check(
        rig.guard(
            "s14-observed-enforce-allow",
            ANCHOR,
            cited_clean,
            session="e2e-s14",
            tenant="polecat-obs",
            agent="polecat-obs",
            spool="s14",
        ),
        "allow",
        aspect="21+: enforce admits in-boundary work",
    )
    rig.write_config()  # restore defaults for the store-down scenarios

    # S6 — quipu down, cache warm: still enforced, age declared.
    rig.scrape_metrics("before-down")
    rig.stop_server()
    check(
        rig.guard(
            "s6-store-down-cached",
            ANCHOR,
            "pub fn caller() -> u32 {\n"
            f"    // implements {FAKE_ITEM}\n"
            "    tally(7)\n"
            "}",
            session="e2e-s6",
        ),
        "deny",
        "FABRICATED REFERENCE",
        "cached",
        aspect="24: cache enforces last-known policy and declares its age",
    )

    # S7 — quipu down, cache gone: loud fail-open, never a silent allow.
    cache = rig.state / "projection-cache.json"
    if cache.exists():
        cache.unlink()
    check(
        rig.guard(
            "s7-store-down-no-cache",
            ANCHOR,
            "pub fn caller() -> u32 {\n"
            f"    // implements {FAKE_ITEM}\n"
            "    tally(8)\n"
            "}",
            session="e2e-s7",
        ),
        "notify",
        "could not project governed policy",
        aspect="18: ungoverned is loud — a typed non-answer, not a green light",
    )

    return checks


def drain_and_audit(rig: Rig, checks: list[dict]) -> None:
    """S8: restart quipu, drain the signed verdict spool into it, and read the
    verdicts back out of the governed store."""
    rig.start_server()
    spool = rig.state / "verdicts.jsonl"
    spooled = spool.read_text().count("\n") if spool.exists() else 0
    proc = subprocess.run(
        [str(rig.yupana), "verdicts", "--to", ENDPOINT, "--spool", str(spool)],
        capture_output=True,
        text=True,
        cwd=rig.repo,
        env=rig.hook_env(),
        timeout=120,
    )
    (rig.logs / "s8-drain.log").write_text(proc.stdout + proc.stderr)

    results = rig.query(
        "PREFIX aegis: <http://aegis.gastown.local/ontology/> "
        "SELECT ?v ?tier ?fresh ?sig ?outcome WHERE { "
        "?v a aegis:Verdict . "
        "OPTIONAL { ?v aegis:tier ?tier } "
        "OPTIONAL { ?v aegis:freshness ?fresh } "
        "OPTIONAL { ?v aegis:signature ?sig } "
        "OPTIONAL { ?v aegis:outcome ?outcome } }"
    )
    rows = results.get("results", {}).get("bindings", [])
    tiers = {r["tier"]["value"] for r in rows if "tier" in r}
    signed = sum(1 for r in rows if "sig" in r)
    ok = proc.returncode == 0 and spooled > 0 and len(rows) >= 1
    checks.append(
        {
            "name": "s8-verdict-return",
            "outcome": f"{len(rows)} verdicts in quipu ({signed} signed, tiers {sorted(tiers)}), {spooled} spooled",
            "reason": "",
            "expected": "verdicts present in the governed store",
            "must_contain": [],
            "missing": [] if ok else ["verdict rows in quipu"],
            "aspect": "25: verdicts return as signed facts with tier + freshness",
            "ms": 0,
            "exit": proc.returncode,
            "ok": ok,
        }
    )
    log(
        f"  {'PASS' if ok else 'FAIL'} s8-verdict-return: {spooled} spooled -> "
        f"{len(rows)} in quipu, {signed} signed, tiers {sorted(tiers)}"
    )
    rig.scrape_metrics("after-drain")


def spool_summary(rig: Rig) -> dict:
    """What the observability channels recorded — the tracing half of the eval."""
    counts: dict[str, int] = {}
    metrics = rig.state / "metrics.jsonl"
    if metrics.exists():
        for line in metrics.read_text().splitlines():
            try:
                kind = json.loads(line).get("kind", "?")
            except json.JSONDecodeError:
                continue
            counts[kind] = counts.get(kind, 0) + 1
    return counts


def write_report(work: Path, checks: list[dict], counts: dict) -> bool:
    passed = sum(1 for c in checks if c["ok"])
    report = {
        "passed": passed,
        "total": len(checks),
        "metrics_events": counts,
        "checks": checks,
    }
    (work / "report.json").write_text(json.dumps(report, indent=2))
    lines = [
        "# Yupana <-> Quipu grounding integration — e2e eval",
        "",
        f"**{passed}/{len(checks)} checks passed.**",
        "",
        "| scenario | aspect | expected | outcome | latency | ok |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for c in checks:
        lines.append(
            f"| {c['name']} | {c['aspect']} | {c['expected']} | "
            f"{c['outcome']} | {c['ms']}ms | {'PASS' if c['ok'] else 'FAIL'} |"
        )
    lines += ["", f"Metrics spool events: `{json.dumps(counts)}`", ""]
    (work / "report.md").write_text("\n".join(lines))
    log(f"report: {work / 'report.md'}")
    return passed == len(checks)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--workdir", type=Path, default=None)
    ap.add_argument("--profile", choices=["release", "debug"], default="release")
    ap.add_argument("--keep", action="store_true", help="keep the workdir on success")
    args = ap.parse_args()

    # Absolute, because every subprocess runs with the workdir as its cwd.
    work = (args.workdir or YUPANA_ROOT / "target" / "e2e").resolve()
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)

    rig = Rig(work, args.profile)
    rig.setup()
    log("seeding quipu store (camayoc policy pack + work items + verifier key)")
    rig.seed()
    rig.start_server()
    try:
        log("running scenarios")
        checks = scenarios(rig)
        drain_and_audit(rig, checks)
    finally:
        rig.stop_server()

    counts = spool_summary(rig)
    ok = write_report(work, checks, counts)
    if ok and not args.keep:
        log("all green")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
