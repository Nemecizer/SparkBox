# SparkBox development quick start

The current round diagnoses a live Library scan failure from missing files behind Hugging Face cache links. It continues `4879869` on `codex/p3-live-download`. Read [the handoff](HANDOFF.md), [building instructions](BUILDING.md) and [security safeguards](../SECURITY.md).

## Inspect offline

From this checkout, run `npm ci`, then `PORT=1533 npm run dev:mock`. Open:

`http://localhost:1533/mock.html?reset=1&still=1&theme=light&open=models%3Atab%3Dlibrary&library=missing-cache`

1. All four synthetic nodes show **Scanned**. Healthy model rows remain visible beside **example/damaged**.
2. Select **example/damaged**. Its affected copy reads **incomplete (2 missing)**. The details explain that Size counts files still present and that scanning repairs or deletes nothing.
3. Verify **Load…** is dimmed. The offline tests also verify that this incomplete-only model cannot dispatch a copy.
4. Repeat with `theme=dark`. Stop the server afterward. Browser Demo never contacts your nodes.

The earlier `library=scan-failure` and `library=progress` fixtures remain available. Missing cache links are different from permission failures: unreadable folders must still report **Scan failed**.

## Live acceptance boundary

The owner authorized this round's live inspection. Use the separately signed candidate, preserve the saved inventory and normal authentication, and start read-only. Find models on every connected node, inspect missing-cache counts and healthy copies, and scan again. Do not repair/delete cache data or start a model as part of this inspection. The handoff records the observed outcome and limitations.

Read-only capacity comparison is separate from starting a transfer. The next download/verify/copy acceptance requires review of one small public model, its exact files, tool, empty destinations and fabric route through the normal app safeguards.

## Build and checks

Use `scripts/check.sh` for the required offline checks. Build from a clean commit with `scripts/public-release.py build` and a Developer ID Application identity; it invokes `scripts/build-candidate.sh`. Keep old candidates, the private repository and the published release intact. Do not publish, push or submit for notarization incidentally.
