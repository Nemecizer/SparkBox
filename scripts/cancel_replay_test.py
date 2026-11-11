import copy
import json
from pathlib import Path
import socket
import tempfile
import subprocess
import unittest
from unittest.mock import patch
import cancel_replay as w

HERE = Path(__file__).resolve().parent
TRACE = HERE/'fixtures/cancel-replay-trace.json'
PLAN = w.Plan('FixtureOrg/ExampleModel', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dc-1234abcd')


def fixture(source='serving', target='copying', pct=25, selected=True):
    rs = [{'path':[0,0], 'role':'AXCheckBox', 'title':'Admin · fixture', 'enabled':True, 'value':1},
          {'path':[0,1], 'role':'AXButton', 'title':'4/4 online'},
          {'path':[0,2], 'role':'AXButton', 'title':'Cancel chain…', 'enabled':True}]
    for i,(label,node,phase) in enumerate([('Download, verify and share '+PLAN.repo,PLAN.source,source),('Wait, then copy '+PLAN.repo,PLAN.target,target)]):
        path=[1,i];rs.append({'path':path,'role':'AXRow'})
        for j,v in enumerate([label,node,phase,PLAN.chain_id+' · cell2 → cell1']):
            rs.append({'path':path+[j],'role':'AXStaticText','value':v})
        if i==1:rs.append({'path':path+[4],'role':'AXProgressIndicator','value':pct})
    if selected:
        rs += [{'path':[2,0],'role':'AXStaticText','value':PLAN.repo+' · chain '+PLAN.chain_id},
               {'path':[2,1],'role':'AXStaticText','value':'Commit '+PLAN.revision}]
    return {'rows':rs}


def modal():
    return {'rows':[
        {'path':[0],'role':'AXWindow','subrole':'AXApplicationAlertDialog','title':'Cancel chain '+PLAN.chain_id+'?'},
        {'path':[0,0],'role':'AXStaticText','value':"Stop the owned jobs on cell2, cell1, including the temporary file server. Verified files and partial files remain. Disconnected nodes need cancellation after reconnection; waiting jobs expire within the chain's 24-hour deadline. Reopening Jobs never starts a replacement chain."},
        {'path':[0,1],'role':'AXButton','title':'Keep going','enabled':True},
        {'path':[0,2],'role':'AXButton','title':'Cancel chain','enabled':True}]}

def make_capture(directory):
    """Build minimal synthetic AX snapshots; contains no captured app state."""
    trace = json.loads(TRACE.read_text())
    assert trace['synthetic'] is True
    events = []
    for frame in trace['frames']:
        n, elapsed, duration, source, target, pct = frame
        snapshot = fixture(source, target, pct, selected=False)
        if source is None:
            snapshot['rows'] = snapshot['rows'][:3]
        if pct is None:
            snapshot['rows'] = [r for r in snapshot['rows'] if r['role'] != 'AXProgressIndicator']
        name = '%03d-waiting-for-members.json' % n
        (directory/name).write_text(json.dumps(snapshot))
        events.append({'event':'snapshot','number':n,'stage':'waiting-for-members',
                       'elapsedSeconds':elapsed,'readSeconds':duration})
    (directory/'events.jsonl').write_text(''.join(json.dumps(e)+'\n' for e in events))


class ReplayTests(unittest.TestCase):
    def setUp(self):
        # Every case is forbidden from starting processes, opening sockets or
        # sleeping; simulated clock values drive the complete controller.
        self.guards=[patch.object(subprocess,'Popen',side_effect=AssertionError('process forbidden')),
                     patch.object(socket,'socket',side_effect=AssertionError('network forbidden'))]
        for guard in self.guards:guard.start();self.addCleanup(guard.stop)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.capture = Path(self.temp.name)
        make_capture(self.capture)

    def test_synthetic_trace_has_no_safe_window(self):
        result=w.replay(self.capture,PLAN)
        self.assertEqual(result['snapshots'],80)
        self.assertEqual(result['proposedDialogs'],0)
        self.assertEqual(result['proposedConfirmations'],0)
        self.assertEqual(sum(r['intent']=='would-select-source' for r in result['decisions']),1)
        self.assertEqual(result['decisions'][-1]['reason'],'member-terminal-before-confirmation')
        copying=[p for p in result['phaseTransitions'] if p['phases'] and p['phases'][1]=='copying']
        self.assertEqual(len(copying),1);self.assertEqual(copying[0]['phases'][2],[0])

    def test_synthetic_positive_progress_proposes_confirmation(self):
        # Explicitly synthetic: the saved test trace remains unchanged at 0%.
        s=json.loads((self.capture/'066-waiting-for-members.json').read_text())
        rs=s['rows'];target=next(r for r in rs if r.get('role')=='AXRow' and any(c.get('value')=='Wait, then copy '+PLAN.repo and w.below(c,r) for c in rs))
        for r in rs:
            if r.get('role')=='AXProgressIndicator' and w.below(r,target):r['value']=25
        rs += [{'path':[99,0],'role':'AXStaticText','value':PLAN.repo+' · chain '+PLAN.chain_id},
               {'path':[99,1],'role':'AXStaticText','value':'Commit '+PLAN.revision},
               {'path':[99,2],'role':'AXButton','title':'Cancel chain…','enabled':True}]
        p=w.Policy(PLAN)
        self.assertEqual(p.step(s,3,.2)['intent'],'would-open-cancel-dialog')
        self.assertEqual(p.step(modal(),3.5,.2)['intent'],'would-confirm-once')

    def test_preselection_then_one_confirmation(self):
        p=w.Policy(PLAN)
        self.assertEqual(p.step(fixture('downloading','waiting',0,False),1,.2)['intent'],'would-select-source')
        self.assertEqual(p.step(fixture('downloading','waiting',0),2,.2)['intent'],'wait')
        self.assertEqual(p.step(fixture(),3,.2)['intent'],'would-open-cancel-dialog')
        self.assertEqual(p.step(modal(),3.4,.2)['intent'],'would-confirm-once')
        self.assertEqual(p.step(modal(),3.5,.1)['intent'],'none')
        self.assertEqual(p.step(fixture(),4,.1)['intent'],'none')

    def test_progress_refusals(self):
        for value in [0,100,-1,101,True,'25',None,float('nan'),float('inf')]:
            with self.subTest(value=value):
                self.assertEqual(w.Policy(PLAN).step(fixture(pct=value),1,.1)['intent'],'wait')

    def test_missing_or_duplicate_progress(self):
        for duplicate in [False,True]:
            s=fixture();r=s['rows'][-3]
            self.assertEqual(r['role'],'AXProgressIndicator')
            if duplicate:s['rows'].append(copy.deepcopy(r))
            else:s['rows'].remove(r)
            self.assertEqual(w.Policy(PLAN).step(s,1,.1)['intent'],'wait')

    def test_positive_edges(self):
        for pct in [1,99]:self.assertEqual(w.Policy(PLAN).step(fixture(pct=pct),1,.1)['intent'],'would-open-cancel-dialog')

    def test_early_and_verifying_phases(self):
        for source,target in [('downloading','waiting'),('hashing','waiting'),('serving','verifying'),('downloading','copying')]:
            self.assertEqual(w.Policy(PLAN).step(fixture(source,target),1,.1)['intent'],'wait')

    def test_either_member_terminal_latches(self):
        for phase in w.TERMINAL:
            for source,target in [(phase,'waiting'),('serving',phase)]:
                p=w.Policy(PLAN);self.assertEqual(p.step(fixture(source,target),1,.1)['intent'],'stop')
                self.assertEqual(p.step(fixture(),2,.1)['intent'],'none')

    def test_wrong_chain_or_duplicate_rows(self):
        for change in ['id','node','duplicate','extra-phase']:
            s=fixture()
            if change=='id':s['rows'][7]['value']='dc-deadbeef · cell2 → cell1'
            elif change=='node':s['rows'][5]['value']='cell3'
            elif change=='duplicate':s['rows'].append(copy.deepcopy(s['rows'][3]))
            else:s['rows'].append({'role':'AXStaticText','path':[1,0,9],'value':'done'})
            self.assertEqual(w.Policy(PLAN).step(s,1,.1)['intent'],'stop')

    def test_revision_and_selection_required(self):
        for change in ['revision','chain','missing']:
            s=fixture()
            if change=='revision':s['rows'][-1]['value']='Commit '+'b'*40
            elif change=='chain':s['rows'][-2]['value']=PLAN.repo+' · chain dc-deadbeef'
            else:s['rows']=s['rows'][:-2]
            p=w.Policy(PLAN);self.assertEqual(p.step(s,1,.1)['intent'],'would-select-source')
            self.assertEqual(p.step(s,2,.1)['intent'],'wait')

    def test_changed_selected_revision_stops(self):
        p=w.Policy(PLAN);p.step(fixture(pct=0),1,.1)
        s=fixture();s['rows'][-1]['value']='Commit '+'b'*40
        self.assertEqual(p.step(s,2,.1)['intent'],'stop')

    def test_equal_timestamps_refuse(self):
        p=w.Policy(PLAN);p.step(fixture(pct=0),1,.1)
        self.assertEqual(p.step(fixture(),1,.1)['intent'],'stop')

    def test_admin_online_and_control_gates(self):
        for change in ['admin','offline','disabled','missing']:
            s=fixture()
            if change=='admin':s['rows'][0]['value']=0
            elif change=='offline':s['rows'][1]['title']='3/4 online'
            elif change=='disabled':s['rows'][2]['enabled']=False
            else:s['rows'].pop(2)
            self.assertEqual(w.Policy(PLAN).step(s,1,.1)['intent'],'stop')

    def test_time_bounds(self):
        for elapsed,duration in [(900,.1),(-1,.1),(1,2.1),(1,-1),(1,True),(float('nan'),.1),(1,float('inf')),(1,1.1)]:
            with self.subTest(elapsed=elapsed,duration=duration):self.assertEqual(w.Policy(PLAN).step(fixture(),elapsed,duration)['intent'],'stop')
        p=w.Policy(PLAN);p.step(fixture(pct=0),2,.1)
        self.assertEqual(p.step(fixture(),1,.1)['intent'],'stop')

    def test_slow_active_snapshot_refuses_before_dialog(self):
        self.assertEqual(w.Policy(PLAN).step(fixture(),3,1.6)['intent'],'stop')

    def test_dialog_age_includes_capture_time(self):
        p=w.Policy(PLAN);p.step(fixture(),3,.4)
        self.assertEqual(p.step(modal(),4.2,.2)['intent'],'stop')

    def test_waiting_for_modal_cannot_refresh_age(self):
        p=w.Policy(PLAN);p.step(fixture(),3,.2)
        self.assertEqual(p.step(fixture(),3.8,.1)['intent'],'wait')
        self.assertEqual(p.step(modal(),4.31,.1)['intent'],'stop')

    def test_completion_and_verification_races(self):
        for phase in ['done','verifying']:
            p=w.Policy(PLAN);p.step(fixture(),3,.2)
            self.assertEqual(p.step(fixture(target=phase),3.2,.1)['intent'],'stop')
            self.assertEqual(p.step(modal(),3.4,.1)['intent'],'none')

    def test_wrong_disabled_or_duplicate_modal(self):
        for change in ['title','body','disabled','duplicate','extra-dialog','missing-button']:
            m=modal()
            if change=='title':m['rows'][0]['title']='Cancel chain dc-deadbeef?'
            elif change=='body':m['rows'][1]['value']=m['rows'][1]['value'].replace('cell1','cell3')
            elif change=='disabled':m['rows'][-1]['enabled']=False
            elif change=='duplicate':m['rows'].append(copy.deepcopy(m['rows'][-1]))
            elif change=='extra-dialog':m['rows'].append(copy.deepcopy(m['rows'][0]))
            else:m['rows'].pop()
            p=w.Policy(PLAN);p.step(fixture(),3,.2)
            self.assertEqual(p.step(m,3.3,.1)['intent'],'stop')

    def test_visible_relock_in_modal_refuses(self):
        m=modal();m['rows'].append({'path':[2],'role':'AXCheckBox','title':'Read-only','value':0})
        p=w.Policy(PLAN);p.step(fixture(),3,.2)
        self.assertEqual(p.step(m,3.3,.1)['intent'],'stop')

    def test_unexpected_modal_and_disappeared_rows(self):
        self.assertEqual(w.Policy(PLAN).step(modal(),1,.1)['intent'],'stop')
        p=w.Policy(PLAN);p.step(fixture(selected=False),1,.1)
        self.assertEqual(p.step({'rows':fixture()['rows'][:3]},2,.1)['intent'],'stop')

    def test_invalid_snapshot_and_plan(self):
        for s in [{'rows':None},{'rows':[{'path':[]}]},{'rows':[{'path':[True]}]}]:
            self.assertEqual(w.Policy(PLAN).step(s,1,.1)['intent'],'stop')
        with self.assertRaises(w.Refused):w.Plan('../escape','a'*40,'dc-deadbeef')

if __name__=='__main__':unittest.main(verbosity=2)
