# SparkBox development quick start

Continue on `codex/p3-active-copy-cancel`, following runtime commit `92dfacf`. Read [the current handoff](HANDOFF.md), [building instructions](BUILDING.md) and [security safeguards](../SECURITY.md). The [preceding source-identity fix](ACTIVE-COPY-HANDOFF.md) and [earlier chain recovery handoff](CHAIN-RECOVERY-HANDOFF.md) remain historical records.

Ordinary Library active-copy cancellation and partial-copy continuation passed subsequent bounded live tests with the signed `92dfacf` candidate. Active download-chain cancellation remains unproven: its separate attempt completed before an eligible cancellation window. Both completed new copies and all older copies/history remain preserved.

The offline replay controller is now versioned with synthetic fixtures and 22 regression tests. It has no live adapter and only emits proposals. The original 80-observation replay exposed no positive-progress copying window. Read [the replay guide](CANCELLATION-REPLAY.md) before preparing a separately authorized attempt. Do not repeat transfers, reuse occupied destinations, weaken guards or publish raw evidence.

Run the targeted offline tests:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p cancel_replay_test.py -v
python3 -m unittest discover -s scripts -p distribution_audit_test.py
python3 -m unittest discover -s scripts -p public_release_test.py
python3 scripts/audit-distribution.py
```

Use the installed Homebrew toolchain as described in BUILDING.md. `scripts/check.sh` includes the replay suite. No product runtime changed in this wrap-up; prior app test results and limitations remain in the handoffs. No new signed binary, live test or publication follows from this commit.

Next: a separately scoped live chain-cancellation attempt with a fresh reviewed artifact, destinations and offline-tested normal-UI adapter, or offline P3.4 HF cache revision grouping. Other recovery/installation acceptance remains separate. Stop after the current round.
