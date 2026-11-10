# SparkBox development quick start

This branch continues the public v0.1.0 baseline `3121d96`. The P3.8 offline round binds new downloads and their disk reviews to the repository, full commit and file manifest shown in Models → Hugging Face. It adds branch/tag selection, explicit revision inspection, visible configuration failures and stale-response protection.

Read [the handoff](HANDOFF.md), [building instructions](BUILDING.md), [security safeguards](../SECURITY.md) and [distribution privacy](DISTRIBUTION-PRIVACY.md). The published v0.1.0 assets remain the release baseline. This candidate is separate, signed for local review, and is not launched, notarized or published by this round.

## Offline browser inspection

From this source checkout:

```sh
npm ci
PORT=1533 npm run dev:mock
```

Open `http://localhost:1533/mock.html?reset=1&still=1&theme=light&open=models%3Atab%3Dhf`. This browser Demo uses synthetic nodes and performs no cluster work. In Hugging Face, select a recommended model, expand **Files to download**, and inspect **Commit**. Choose the synthetic **example-release** tag: its full commit changes to the displayed fixed version. Repeat with `theme=dark`.

To inspect the error state, add `&hf=config-error` to the Demo URL and select a model again. A simulated HTTP 403 configuration error replaces the fit table. Actual gated-access checks require separate native/live acceptance.

## Verification and build

Follow [BUILDING.md](BUILDING.md) for the complete offline checks. `scripts/build-candidate.sh` requires a clean committed tree and writes a new sibling candidate folder. `scripts/public-release.py build` selects Developer ID signing and calls that script without launching or submitting the app. The candidate's provenance identifies its exact source commit and checksum.

Do not push, publish, notarize or test against a real cluster incidentally. Keep the empty-install contract, saved settings, credentials and older app builds intact. [The handoff](HANDOFF.md) records limits and the recommended next bounded round.
