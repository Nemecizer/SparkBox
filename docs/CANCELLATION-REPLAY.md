# Offline cancellation replay

`scripts/cancel_replay.py` evaluates saved UI-shaped observations without opening an application or connecting to a node. Its output is a sequence of proposals, not executed actions. It has no live mode. The default cell labels are synthetic fixture labels, not saved connection settings.

## Tests

From the repository root, using the documented Python toolchain:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p cancel_replay_test.py -v
```

The tests generate minimal synthetic Accessibility snapshots in a temporary directory from `scripts/fixtures/cancel-replay-trace.json`. That reduced trace preserves the timing/phase regression but contains no raw application state. Tests cover an 80-frame sequence with no eligible window and simulated positive-progress sequences, exact selection/revision, modal checks, single-confirmation latching and refusal paths. Process creation and network sockets are forbidden in tests.

## Replay input

For a separately prepared private capture, run `python3 scripts/cancel_replay.py --help`. Supply a capture directory, repository, immutable revision, exact chain ID and a new output filename. The directory contains `events.jsonl` with snapshot events and numbered `<number>-<stage>.json` files. Each event supplies elapsed/read seconds; each snapshot supplies a `rows` list with Accessibility roles, paths and relevant labels/values. Existing output files are never overwritten. Keep real captures and reports outside this source checkout.

The policy can propose selecting the exact source before copying begins. It cannot propose the cancellation dialog until the selected detail matches the chain/revision, source is serving, target is copying, and one finite numeric progress value is strictly between zero and 100. Admin, all expected nodes online, enabled controls and exact normal dialog text remain required. The observer stops after either member becomes terminal, identity changes, stale observations or its 15-minute bound. The active-observation budget includes snapshot-read time. It proposes at most one final confirmation; any failure latches it closed.

A modal may hide the background rows, so even a recent valid observation can race node completion. Proposed confirmation is never proof of cancellation or cleanup. The tool does not authenticate, start or cancel jobs, follow replacements, retry actions, change polling or modify copied files.

## Separately authorized live preparation

A future adapter must remain disarmed by default and use only the normal uniquely observed UI controls. Bind a fresh reviewed plan and new chain ID, reject existing IDs, include adapter setup/binding time in the observation bound, and test action mapping offline before starting one chain. Preserve the normal Admin/Touch ID, Look only, trust, lease, ownership and debug safeguards.

Use truly empty source/target destinations and an explicitly reviewed pinned artifact and installed tool. A larger artifact can offer more reporting cycles but cannot guarantee an active window; no particular repository or payload budget is authorized by this guide. Preserve every completed/partial copy and existing record. Do not throttle the network, add artificial production delays, install tools, pull images or relax progress checks incidentally.

The observation deadline does not stop node jobs. Review any still-running owned jobs through the normal cleanup flow without conflating cleanup cancellation with active-copy acceptance. Never retry an ambiguous confirmation. If completion wins, retain the files and report cancellation unproven without a second attempt.

Independent final checks must prove terminal cancellation during copying with positive incomplete byte progress, exact nonempty ownership/participant plan, unchanged old records and files, retained partial identity, recorded worker/child/process-group cleanup, the exact reviewed listener and owned-container absence, and cleared temporary secrets/server paths. Missing process identity remains unknown. No direct signals, secret reads or arbitrary deletion are allowed. Restore any changed settings through normal safeguards, lock Admin, record the result and stop.
