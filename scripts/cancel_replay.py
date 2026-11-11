"""Offline-only cancellation policy and replay. Emits proposals; cannot control an app.

No subprocess, UI, network, authentication or node adapter exists in this module.
A future live adapter requires separate authorization/review and fresh preflight.
The default cell names are synthetic test labels, not connection settings.
"""
import argparse
from dataclasses import dataclass
import json
import math
from pathlib import Path
import re

BOUND = 900.0
MAX_READ = 2.0
MAX_ACTIVE_AGE = 1.5
ACTIVE = {'starting', 'checking the selected hf tool', 'downloading', 'verifying', 'hashing', 'serving', 'waiting', 'copying'}
TERMINAL = {'done', 'failed', 'canceled', 'cancelled'}

class Refused(ValueError):
    pass

@dataclass(frozen=True)
class Plan:
    repo: str
    revision: str
    chain_id: str
    source: str = 'cell2'
    target: str = 'cell1'
    def __post_init__(self):
        if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', self.repo) or not re.fullmatch(r'[a-f0-9]{40}', self.revision) or not re.fullmatch(r'dc-[a-f0-9]{8}', self.chain_id) or any(part in ('.', '..') for part in self.repo.split('/')) or self.source == self.target:
            raise Refused('Invalid exact reviewed plan')


def labels(row):
    return {row[k] for k in ('title', 'description', 'value') if isinstance(row.get(k), str) and row[k]}


def below(row, parent):
    a, b = row['path'], parent['path']
    return len(a) > len(b) and a[:len(b)] == b


def rows(snapshot):
    rs = snapshot.get('rows')
    if not isinstance(rs, list) or len(rs) > 4000:
        raise Refused('Invalid snapshot')
    for r in rs:
        p = r.get('path') if isinstance(r, dict) else None
        if not isinstance(p, list) or not p or any(type(i) is not int or i < 0 for i in p):
            raise Refused('Invalid AX path')
    return rs


def online_admin(rs):
    return (any(r.get('role') == 'AXCheckBox' and r.get('enabled') is True and r.get('value') == 1 and any(t.startswith('Admin · ') for t in labels(r)) for r in rs)
            and any('4/4 online' in labels(r) for r in rs))


def chain_rows(rs, plan):
    found = {}
    for parent in rs:
        if parent.get('role') != 'AXRow':
            continue
        children = [r for r in rs if below(r, parent)]
        texts = set().union(*(labels(r) for r in children if r.get('role') == 'AXStaticText'))
        for kind, title, node in [('source', 'Download, verify and share ' + plan.repo, plan.source), ('target', 'Wait, then copy ' + plan.repo, plan.target)]:
            if title not in texts:
                continue
            ids = {t.split(' · ')[0] for t in texts if re.fullmatch(r'dc-[a-f0-9]{8} · ' + re.escape(plan.source + ' → ' + plan.target), t)}
            phases = texts & (ACTIVE | TERMINAL)
            if kind in found or ids != {plan.chain_id} or node not in texts or len(phases) != 1:
                raise Refused('Ambiguous or changed chain/node/phase')
            found[kind] = {'phase': next(iter(phases)), 'progress': [r.get('value') for r in children if r.get('role') == 'AXProgressIndicator']}
    if len(found) != 2:
        return None
    progress = found['target']['progress']
    found['active'] = (found['source']['phase'] == 'serving' and found['target']['phase'] == 'copying'
                       and len(progress) == 1 and type(progress[0]) in (int, float)
                       and math.isfinite(progress[0]) and 0 < progress[0] < 100)
    return found


def selected(rs, plan):
    texts = set().union(*(labels(r) for r in rs if r.get('role') == 'AXStaticText'))
    return plan.repo + ' · chain ' + plan.chain_id in texts and 'Commit ' + plan.revision in texts


def enabled(rs, title):
    matches = [r for r in rs if r.get('role') == 'AXButton' and title in labels(r)]
    return bool(matches) and all(r.get('enabled') is True for r in matches)


def validate_modal(rs, plan):
    dialogs = [r for r in rs if r.get('subrole') == 'AXApplicationAlertDialog']
    if len(dialogs) != 1 or 'Cancel chain ' + plan.chain_id + '?' not in labels(dialogs[0]):
        raise Refused('Wrong or ambiguous confirmation dialog')
    children = [r for r in rs if below(r, dialogs[0])]
    body = ('Stop the owned jobs on ' + plan.source + ', ' + plan.target + ', including the temporary file server. '
            'Verified files and partial files remain. Disconnected nodes need cancellation '
            "after reconnection; waiting jobs expire within the chain's 24-hour deadline. "
            'Reopening Jobs never starts a replacement chain.')
    if not any(r.get('role') == 'AXStaticText' and body in labels(r) for r in children):
        raise Refused('Wrong participants or dialog text')
    for title in ('Keep going', 'Cancel chain'):
        matches = [r for r in children if r.get('role') == 'AXButton' and title in labels(r)]
        if len(matches) != 1 or matches[0].get('enabled') is not True:
            raise Refused('Missing, ambiguous or disabled dialog control')


class Policy:
    """Single-use proposals only; all refusals and terminal outcomes latch closed."""
    def __init__(self, plan):
        self.plan = plan
        self.state = 'waiting'
        self.last_time = -1.0
        self.selection_proposed = False
        self.selection_verified = False
        self.last_active_start = None
        self.last_chain = None

    def step(self, snapshot, elapsed, read_seconds):
        if self.state == 'stopped':
            return {'intent': 'none', 'reason': 'already-stopped'}
        try:
            return self._step(snapshot, elapsed, read_seconds)
        except Refused as e:
            self.state = 'stopped'
            return {'intent': 'stop', 'reason': str(e)}

    def _step(self, snapshot, elapsed, read_seconds):
        if any(type(t) not in (int, float) or not math.isfinite(t) for t in (elapsed, read_seconds)) or not 0 <= read_seconds <= MAX_READ or elapsed <= self.last_time or elapsed < read_seconds or elapsed >= BOUND:
            raise Refused('Invalid, stale, slow or expired observation')
        self.last_time = elapsed
        rs = rows(snapshot)
        ds = [r for r in rs if r.get('subrole') == 'AXApplicationAlertDialog']
        if self.state == 'modal' and ds:
            validate_modal(rs, self.plan)
            # A visible revoked Admin/offline state must never be ignored. Modal
            # may hide background rows; the preceding observation is age-bounded.
            admins = [r for r in rs if r.get('role') == 'AXCheckBox' and any(t == 'Read-only' or t.startswith('Admin · ') for t in labels(r))]
            if admins and not online_admin(rs):
                raise Refused('Visible Admin/connection state changed')
            if elapsed - self.last_active_start > MAX_ACTIVE_AGE:
                raise Refused('Active observation too old; leave dialog for review')
            self.state = 'stopped'  # latch BEFORE proposing any final action
            return {'intent': 'would-confirm-once', 'reason': 'exact-modal-and-fresh-active-observation'}
        if ds or not online_admin(rs):
            raise Refused('Unexpected dialog, locked Admin or offline node')
        chain = chain_rows(rs, self.plan)
        self.last_chain = chain
        if chain is None:
            if self.selection_proposed:
                raise Refused('Expected chain rows disappeared')
            return {'intent': 'wait', 'reason': 'await-exact-members'}
        if any(chain[k]['phase'] in TERMINAL for k in ('source', 'target')):
            self.state = 'stopped'
            return {'intent': 'stop', 'reason': 'member-terminal-before-confirmation'}
        if self.state == 'modal':
            if not chain['active']:
                raise Refused('Active phase lost before modal')
            if elapsed - self.last_active_start > MAX_ACTIVE_AGE:
                raise Refused('Dialog did not arrive within active observation budget')
            return {'intent': 'wait', 'reason': 'await-modal-without-refreshing-active-age'}
        if not selected(rs, self.plan):
            if self.selection_verified:
                raise Refused('Selected chain or revision changed')
            if not self.selection_proposed:
                self.selection_proposed = True
                return {'intent': 'would-select-source', 'reason': 'preselect-exact-chain-before-active-window'}
            return {'intent': 'wait', 'reason': 'await-exact-selected-chain-and-revision'}
        self.selection_verified = True
        if not chain['active']:
            return {'intent': 'wait', 'reason': 'positive-copy-progress-required'}
        if not enabled(rs, 'Cancel chain…'):
            raise Refused('Cancel chain control is disabled or missing')
        if read_seconds > MAX_ACTIVE_AGE:
            raise Refused('Active snapshot already too old')
        self.last_active_start = elapsed - read_seconds
        self.state = 'modal'
        return {'intent': 'would-open-cancel-dialog', 'reason': 'exact-active-chain-selected'}


def replay(directory, plan):
    policy = Policy(plan)
    results, phases = [], []
    previous = None
    events = [json.loads(line) for line in (directory / 'events.jsonl').read_text().splitlines()]
    for e in events:
        if e['event'] != 'snapshot':
            continue
        name = '%03d-%s.json' % (e['number'], e['stage'])
        if Path(name).name != name:
            raise Refused('Invalid snapshot name')
        snapshot = json.loads((directory / name).read_text())
        decision = policy.step(snapshot, e['elapsedSeconds'], e['readSeconds'])
        results.append({'snapshot': name, 'elapsedSeconds': e['elapsedSeconds'], **decision})
        c = chain_rows(rows(snapshot), plan)
        phase = None if c is None else [c['source']['phase'], c['target']['phase'], c['target']['progress']]
        if phase != previous:
            phases.append({'snapshot': name, 'elapsedSeconds': e['elapsedSeconds'], 'phases': phase})
            previous = phase
    return {'offlineOnly': True, 'uiActions': 0, 'snapshots': len(results), 'decisions': results, 'phaseTransitions': phases,
            'proposedConfirmations': sum(r['intent'] == 'would-confirm-once' for r in results),
            'proposedDialogs': sum(r['intent'] == 'would-open-cancel-dialog' for r in results)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('capture', type=Path)
    parser.add_argument('--repo', required=True)
    parser.add_argument('--revision', required=True)
    parser.add_argument('--chain-id', required=True)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    result = replay(args.capture, Plan(args.repo, args.revision, args.chain_id))
    with args.output.open('x') as f:
        json.dump(result, f, indent=2)
        f.write('\n')
    print(json.dumps({k: v for k, v in result.items() if k not in ('decisions', 'phaseTransitions')}))

if __name__ == '__main__':
    main()
