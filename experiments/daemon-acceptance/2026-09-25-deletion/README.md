# Default daemon conversation deletion acceptance

This is an opt-in isolated macOS acceptance record, not a test that should run against a user's application data. Product code was unchanged. Service endpoint tokens are read locally by `client.py` and are never saved in evidence.

The workflow below records the historical soft-delete/restore contract. It is not the current permanent-deletion acceptance procedure. Raw captures stay local; see the [source evidence policy](../../../docs/implementation/source-evidence.md). Revalidate the current protocol and use fresh isolated data before adapting these scripts.

## Recorded workflow

1. Build the frontend, then build `desktop` with `custom-protocol` and a separate Tauri identifier (without `review-acceptance`). Copy the executable into independent temporary app bundles. Start each desktop with `DOGCODES_DESKTOP_DATA_DIR` pointing to the same isolated data directory; keep launch processes alive while testing.
2. Create a fresh isolated data directory. Use SQLite backup from the **existing isolated review fixture**, never the production database, to retain realistic immutable source/report evidence. `fixture.json` records the paths and identities used in this run; old PIDs and paths are evidence, not commands to reuse.
3. `prepare.py` registers a disposable project and four named tasks through normal RPC and captures history/report/source baselines. Run it once per fixture.
4. In native B/C windows, open the same ordinary task. Cancel a delete confirmation, then confirm deletion and observe both windows switch. Pin another task and delete it; archive another and delete from the archive list. Preferences belong to the originating UI storage.
5. `active_guards.py` starts one real Codex primary task and one Kimi independent review of the frozen fixture. It requires available authenticated installations and consumes Agent quota. Observe disabled delete buttons in both windows. Reject the primary test approval (no file changes), cancel the review with normal `cancel_turn_review`, and wait for terminal state before continuing. Do not retry by starting additional real runs automatically.
6. `delete_remaining.py` verifies normal RPC deletion, idempotency and live subscriptions, leaving one task. Delete that last task through the native UI; both windows show new-conversation state and six recently deleted tasks.
7. `restart.py` validates all six are deleted, closes only identified fixture UI processes, verifies service survival, and requests idle retirement. Relaunch the test desktops with the same data-directory environment. Verify a new service PID, the same six deleted IDs, and the new empty task made by existing startup initialization.
8. In B restore pinned/archived tasks and verify their previous groups, then restore the historical task. C receives it and opens the messages/review cards and historical `sample.txt` Diff.
9. `verify_restored.py` checks baseline equality, stale/missing versions, deleted search isolation, a deliberately lost delete reply, explicit same-key retry and subscription replay. It restores remaining deleted tasks and verifies project bytes and original blobs.
10. `cleanup.py` checks no active work, closes only fixture desktops, verifies service survival, requests idle retirement and stops descendants belonging to that service. It retains the disposable fixture and writes `cleanup.json`.

Native clicks were performed with CUA; Python scripts exercise the documented local service protocol. This is a mixed desktop/RPC acceptance, not a claim that every RPC assertion was also clicked in UI. `results.json` contains success assertions; `active-guards.json`, `rpc-delete-results.json` and `before.json` hold supporting fixture evidence. Build/test logs record the 9 passing service regressions. Windows, signing/notarization and all-provider matrices were not exercised.
