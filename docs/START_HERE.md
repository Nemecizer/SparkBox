# SparkBox development quick start

This branch continues completed P3.8 `a4ebb2a` from public v0.1.0 `3121d96`. P3.2 adds capacity-based download-node suggestions in Models → Hugging Face. The comparison uses the inspected files and destination filesystem, and changes the download target only when you click **Use**.

Read [the handoff](HANDOFF.md), [building instructions](BUILDING.md), [security safeguards](../SECURITY.md) and [distribution privacy](DISTRIBUTION-PRIVACY.md). Published v0.1.0 and the previous candidate remain intact. This candidate is separate and is not launched, notarized or published by this round.

## Offline browser inspection

From this source checkout:

```sh
npm ci
PORT=1533 npm run dev:mock
```

Open `http://localhost:1533/mock.html?reset=1&still=1&theme=light&open=models%3Atab%3Dhf&disk=capacity`. This browser Demo uses synthetic data and performs no cluster work.

1. Select **openai/gpt-oss-120b**, then expand **Suggest a download node**.
2. Leave the comparison nodes selected and click **Compare destination capacity**. Cell1 is short of space; cell2 is suggested. **Download node** must still show cell1.
3. Click **Use cell2**. Only the target changes; no tool or job starts. Deselect cell2 from the comparison and compare again; another qualifying node is suggested without changing the target.
4. Wait one minute after a comparison. Its result expires and cannot be applied without a fresh comparison. Type a different **Revision** without inspecting it: comparison and download are dimmed.
5. Repeat with `theme=dark`. Replace `disk=capacity` with `disk=low` and then `disk=unknown`; neither case should suggest a node or change the target. Qwen's finished Demo copies are excluded from comparison.

Stop the local server with Control-C afterward. No native launch or Admin unlock is needed for this inspection.

## Verification and build

Follow [BUILDING.md](BUILDING.md) for offline checks. Run the native suite once at a time; overlapping fake-node runs can trigger process timing failures. `scripts/build-candidate.sh` requires a clean commit and a new sibling candidate folder. `scripts/public-release.py build` selects Developer ID signing and calls that script without launching or submitting the app. Provenance identifies the exact source and checksum.

Do not push, publish, notarize or operate a real cluster incidentally. Preserve the empty-install contract, saved settings, credentials and all older app builds. [The handoff](HANDOFF.md) records remaining limits and the next bounded round.
