# SparkBox development quick start

This branch continues completed capacity suggestions `e11350c`, following P3.8 `a4ebb2a` and public v0.1.0 `3121d96`. This P3.2 round adds **Then copy to**: one explicitly approved download → verify → copy sequence with saved node-side jobs.

Read [the handoff](HANDOFF.md), [building instructions](BUILDING.md), [security safeguards](../SECURITY.md) and [distribution privacy](DISTRIBUTION-PRIVACY.md). Published v0.1.0, the private repository, settings and all older builds remain intact. This candidate is separate and is not launched, notarized or published by this round.

## Offline browser inspection

From this source checkout:

```sh
npm ci
PORT=1533 npm run dev:mock
```

Open `http://localhost:1533/mock.html?reset=1&still=1&theme=light&open=models%3Atab%3Dhf`. This browser Demo uses synthetic data and performs no cluster work.

1. Select **openai/gpt-oss-120b**. Leave **Download node** on cell1, expand **Then copy to** and select cell2 and cell3. Selection alone starts nothing.
2. Click **Download to cell1**. Complete Demo's simulated Admin unlock if asked. Choose the existing hf tool and click **Continue**. Read every destination, disk check, fabric address, commit and temporary port. Cancel once and confirm Jobs has no chain.
3. Repeat and click **Start download and copies**. Jobs shows the source downloading and targets waiting. After about eight seconds, the source serves and targets copy. After about twenty seconds all rows finish. Select a chain row to inspect its commit, deadline and recovery instructions. Library shows only verified copies.
4. Reload with `&chain=source-fail` for simulated source verification failure: no target becomes verified. Repeat with `&chain=copy-fail`: the source and one target report failure; the verified source and successful target remain in Library.
5. Reload, start another chain and select a row. **Cancel chain…** asks first, then stops pending members and retains records. **Resume download…** stays dimmed for chain rows.
6. Repeat in Dark with `theme=dark`. Use `disk=low` to confirm space refusal starts nothing. Existing Qwen Demo copies cannot be chosen as fresh chain destinations.

Stop the local server with Control-C afterward. No native launch or real Admin unlock is needed. Browser reload resets this simulation; native node records persist independently.

## Verification and build

Follow [BUILDING.md](BUILDING.md) for offline checks. Run the native suite once at a time; overlapping fake-node runs can trigger process timing failures. `scripts/build-candidate.sh` requires a clean commit and a new sibling candidate folder. `scripts/public-release.py build` selects Developer ID signing and calls that script without launching or submitting the app. Provenance identifies the exact source and checksum.

Do not push, publish, notarize or operate a real cluster incidentally. [The handoff](HANDOFF.md) records remaining limits and the next bounded round.
