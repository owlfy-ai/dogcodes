#!/usr/bin/env python3
"""Read content-free startup timing JSONL; no database or credentials required."""
import argparse
import collections
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("log", type=Path)
parser.add_argument("--last", type=int, default=5)
args = parser.parse_args()
traces = collections.OrderedDict()
for line in args.log.read_text().splitlines():
    try:
        row = json.loads(line)
        traces.setdefault(row.get("traceId", row.get("runId") or row["taskId"]), []).append(row)
    except (ValueError, KeyError):
        continue  # A concurrent writer can leave an incomplete final line.

for trace, rows in list(traces.items())[-max(1, args.last):]:
    print(f"\n{rows[0]['recordedAt']} task={rows[0]['taskId']} trace={trace}")
    marks = {r["stage"]: r["elapsedMs"] for r in rows if r["event"] == "mark"}
    for row in rows:
        if row["event"] == "end":
            print(f"  {row['stage']:34s} {row['durationMs']:9.2f} ms")
    for start, end, label in [
        ("prompt_accepted", "first_driver_event", "accepted_to_first_driver_event"),
        ("prompt_accepted", "first_text", "accepted_to_first_text"),
        ("first_text", "first_text_published", "first_text_persist_and_publish"),
    ]:
        if start in marks and end in marks:
            print(f"  {label:34s} {marks[end] - marks[start]:9.2f} ms")
    for stage in ["first_text", "first_text_published", "run_failed", "run_stream_finished"]:
        if stage in marks:
            print(f"  total_to_{stage:25s} {marks[stage]:9.2f} ms")
