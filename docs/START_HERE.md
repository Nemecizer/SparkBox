# SparkBox development quick start

This bounded P3.4 round continues the completed download → verify → copy chain at `5f732f84a46814ff65e54a745b16425b25e64ca6`. Library now keeps per-node scan results and explicitly labels last-known and unknown copies. The earlier work is recorded in the [P3.2 chain handoff](P3.2-CHAIN-HANDOFF.md).

Read [the handoff](HANDOFF.md), [building instructions](BUILDING.md), [security safeguards](../SECURITY.md) and [distribution privacy](DISTRIBUTION-PRIVACY.md). Published v0.1.0, the private repository, settings and older builds remain intact. This candidate is separate and is not launched, notarized or published by this round.

## Offline browser inspection

From this source checkout:

```sh
npm ci
PORT=1533 npm run dev:mock
```

Open `http://localhost:1533/mock.html?reset=1&still=1&theme=light&open=models%3Atab%3Dlibrary&library=scan-failure`. This browser-only fixture uses synthetic nodes and performs no cluster work.

1. Read **Library scan results** above the table: cell1 succeeds, cell2 reports a simulated scan error, cell3 has last-known offline copies and the fourth node has no successful scan. Other models remain visible. Hover over **last known** and **unknown**; neither claims a finished copy or an empty disk.
2. Select a row. Inspect Copy/Delete targets using Demo's simulated Admin unlock if needed. Failed/offline targets cannot be selected. Load uses only a current finished copy; selecting a stale-only row does not enable it. Cancel without starting a copy or deletion.
3. Reload with `library=progress`. Select **example/library-progress**: cell1 shows **downloading 37%**, cell2 **verifying**, and cell3 **incomplete**. Load is dimmed even though verification reports all bytes transferred.
4. Remove the `library` query parameter and reload. Normal Demo scans and verified copies return. This is a separate synthetic fixture, not proof of real-node reconnection. Automated fake-node tests cover retaining and replacing the same cached observation after failure.
5. Repeat both fixtures with `theme=dark`. Resize the Models window and check that scan errors wrap and the table and action controls remain usable.

Stop the local server with Control-C afterward. No native app launch or real Admin unlock is needed. Last-known observations in the native app last only until it quits; no files or settings are created for this cache.

## Verification and build

Follow [BUILDING.md](BUILDING.md) for offline checks. Run one native suite at a time; overlapping fake-node runs can trigger process timing failures. `scripts/build-candidate.sh` requires a clean commit and a new sibling candidate folder. `scripts/public-release.py build` selects Developer ID signing and calls that script without launching or submitting the app. Provenance identifies the exact source and checksum.

The bounded round does not implement HF snapshot grouping, disk columns or 60-second scan reuse. Those are remaining P3.4 work. Real node scan/reconnect acceptance requires separate authorization when cluster access returns. Do not push, publish, notarize or operate a real cluster incidentally.
