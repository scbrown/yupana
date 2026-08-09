#!/usr/bin/env bash
# check-code-shapes.sh — prove shapes/code-edges.ttl can ACCEPT and REFUSE (#13/#14).
#
# A SHACL shape that has never been observed rejecting anything is indistinguishable
# from no shape at all, and FR-20's whole promise ("Yupana never writes to Quipu
# without passing SHACL") rests on these firing. So this runs BOTH directions:
#   fixtures/conforming.ttl  MUST validate clean
#   fixtures/violating.ttl   MUST be refused, naming the violated constraints
# Exit 1 if either outcome is wrong — including if the "bad" fixture passes, which
# is the failure that would otherwise ship silently.
#
# Uses Quipu's /validate endpoint, which takes shapes inline. NOTE: that is NOT
# the same as Quipu's persistent shape registry — a deployment can have zero
# shapes registered server-side and still answer /validate correctly for shapes
# you hand it. Do not read a green run here as "shapes are enforced on every
# write"; it means "these shapes accept and reject what we expect", which is
# exactly the property yupana needs before promoting.
#
# Once the `quipu` feature lands, rudof_lib does this in-process and this script
# becomes the cross-check that yupana and Quipu agree about the same shapes.
set -uo pipefail
Q="${QUIPU_URL:?set QUIPU_URL to your Quipu endpoint, e.g. http://localhost:8080}"
D="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

validate() {  # $1 = data file -> prints "true"/"false"
  python3 - "$D/shapes/code-edges.ttl" "$1" "$Q" <<'PY'
import json,sys,urllib.request
shapes,data,q = open(sys.argv[1]).read(), open(sys.argv[2]).read(), sys.argv[3]
req=urllib.request.Request(q+"/validate",data=json.dumps({"shapes":shapes,"data":data}).encode(),
                           headers={"Content-Type":"application/json"})
o=json.loads(urllib.request.urlopen(req,timeout=25).read().decode())
print(json.dumps({"conforms":o["conforms"],"violations":o["violations"],
                  "issues":[i["message"][:70] for i in o.get("issues",[])[:4]]}))
PY
}

rc=0
good=$(validate "$D/shapes/fixtures/conforming.ttl") || { echo "BLOCKED: could not reach $Q/validate"; exit 2; }
bad=$(validate  "$D/shapes/fixtures/violating.ttl")  || { echo "BLOCKED: could not reach $Q/validate"; exit 2; }

if [ "$(printf '%s' "$good" | python3 -c 'import json,sys;print(json.load(sys.stdin)["conforms"])')" = "True" ]; then
  echo "  PASS  conforming fixture validates clean"
else
  echo "  FAIL  conforming fixture was REFUSED — the shapes are too strict: $good"; rc=1
fi

if [ "$(printf '%s' "$bad" | python3 -c 'import json,sys;print(json.load(sys.stdin)["conforms"])')" = "False" ]; then
  echo "  PASS  violating fixture refused: $(printf '%s' "$bad" | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d["violations"],"violation(s):"," | ".join(d["issues"]))')"
else
  echo "  FAIL  violating fixture PASSED — the shapes cannot fire, which is the"
  echo "        same as having none (FR-20 would be decorative)"; rc=1
fi
exit $rc
