"""Read-only verifier for the isolated desktop review-closure acceptance database.

Usage: python3 scripts/verify-review-closure.py /absolute/path/app-v5.db
No Agent calls, database writes, or production-state discovery.
"""
import hashlib
import json
import sqlite3
import sys
from pathlib import Path

path = Path(sys.argv[1]).resolve(strict=True)
db = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)
events = [(kind, json.loads(payload)) for kind, payload in db.execute(
    "select event_type,payload_json from events order by sequence"
)]
runs, packets = {}, {}
for kind, payload in events:
    if kind in ("review.started", "review.updated"):
        run = payload["review"]
        runs[run["request"]["runId"]] = run
        if kind == "review.started":
            packets[run["request"]["runId"]] = payload["packet"]
for key, run in runs.items():
    assert hashlib.sha256(packets[key].encode()).hexdigest() == run["inputSha256"]

project = next(r for r in runs.values() if r["request"].get("sourceSnapshot"))
original_id = project["request"]["runId"]
fixes = [p for kind, p in events if kind == "turn.queued" and
         (p.get("reviewFix") or {}).get("runId") == original_id]
assert len(fixes) == 1, "expected exactly one submitted repair"
fix = fixes[0]
assert fix["reviewFix"]["findingIndices"] == [0]
assert fix["reviewFix"]["inputSha256"] == project["inputSha256"]
assert any(k == "turn.completed" and p.get("turnId") == fix["turnId"] for k, p in events)

rechecks = [r for r in runs.values() if
            (r["request"].get("recheckOf") or {}).get("runId") == original_id]
assert rechecks and rechecks[-1]["status"] == "completed", "latest real recheck not complete"
latest = rechecks[-1]
assert latest["request"]["turnId"] == fix["turnId"]
assert latest["request"]["recheckOf"]["repairTurnId"] == fix["turnId"]
assert latest["inputSha256"] != project["inputSha256"]
assert latest["report"]["structured"] and all(f["status"] == "reported" for f in latest["report"]["files"])
assert not latest["report"]["findings"], "fixed sample still has reported problems"

packet_text = packets[latest["request"]["runId"]]
packet = json.loads(packet_text)
assert packet["repairReview"]["runId"] == original_id
assert "AFTER_RECHECK_STARTED" not in packet_text
assert "LATER_LIVE_EDIT_KEEP_THIS_COMMENT" in packet_text
changed = packet["changes"]["files"]
assert len(changed) == 1 and changed[0]["path"] == "reports/last.py"
after = changed[0]["after"]
if isinstance(after, dict):
    after = after["text"]
assert "return values[-1]" in after and "values[len(values)]" not in after

retries = [r for r in runs.values() if r["request"].get("retryOf")]
assert len(retries) >= 2
for retry in retries:
    original = runs[retry["request"]["retryOf"]]
    assert retry["inputSha256"] == original["inputSha256"]
    assert retry["request"].get("recheckOf") == original["request"].get("recheckOf")
assert any(r["status"] == "cancelled" for r in runs.values())

summary = {
    "passed": True,
    "database": str(path),
    "originalProjectReview": original_id,
    "repairTurn": fix["turnId"],
    "newVersionReview": latest["request"]["runId"],
    "oldInputSha256": project["inputSha256"],
    "newInputSha256": latest["inputSha256"],
    "selectedIndices": fix["reviewFix"]["findingIndices"],
    "newVersionFile": changed[0]["path"],
    "newVersionStructured": latest["report"]["structured"],
    "newVersionFindings": len(latest["report"]["findings"]),
    "originalSnapshotRetries": len(retries),
    "laterLiveEditExcluded": True,
    "runs": [{"id": r["request"]["runId"], "request": r["request"],
              "status": r["status"], "inputSha256": r["inputSha256"],
              "groups": [{"id": g["id"], "status": g["status"], "sessions": g["sessions"]}
                         for g in (r.get("workflow") or {}).get("groups", [])]}
             for r in runs.values()],
}
print(json.dumps(summary, ensure_ascii=False, indent=2))
