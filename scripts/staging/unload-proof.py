#!/usr/bin/env python3
"""Local Linux PID-1/resource-tracker regression; no model, GPU or cluster connection.

Requires the existing sparkbox-staging:24.04 image (setup.sh). Every test container
has --network none and --pull never. A unique, private Docker volume stands in for
shared /dev/shm across container lifetimes; the Docker VM's own IPC is untouched.
Only this invocation's containers and volume are removed. No image is changed.
"""
import hashlib
import json
from pathlib import Path
import secrets
import subprocess
import tempfile
import time
import uuid

REPO = Path(__file__).resolve().parents[2]
IMAGE = "sparkbox-staging:24.04"
VLLM = r'''#!/usr/bin/python3
import hashlib,json,multiprocessing,os,signal,time
from pathlib import Path
root = Path('/record')
sem = multiprocessing.get_context('spawn').Semaphore(1)
if os.fork() == 0:
    def end(*_):
        time.sleep(.7)  # The parent exits first; the tracker still has a writer.
        (root/'child-finished').touch()
        os._exit(0)
    signal.signal(signal.SIGTERM, end)
    (root/'child-ready').touch()
    while True: time.sleep(.05)
while not (root/'child-ready').exists(): time.sleep(.01)
signal.signal(signal.SIGTERM, lambda *_: os._exit(0))
key = os.environ.get('VLLM_API_KEY')
(root/'ready.json').write_text(json.dumps({'pid':os.getpid(), 'parent':os.getppid(),
    'name':'sem.'+sem._semlock.name.lstrip('/'),
    'keyHash':hashlib.sha256(key.encode()).hexdigest() if key else None}))
while True: time.sleep(.05)
'''
# The previous lifecycle, intentionally a control: replacing PID 1 kills the tracker
# at engine exit. No checked-in historical helper or running container is modified.
EXEC_CONTROL = "import os\nos.execvp('vllm', ['vllm', 'serve', '/model'])\n"


def main():
    prefix = 'sparkbox-staging-unload-' + uuid.uuid4().hex[:12]
    containers = []
    volume = prefix + '-shm'
    made_volume = False

    def docker(*args, timeout=45):
        p = subprocess.run(['docker', '--context', 'desktop-linux', *args],
                           text=True, capture_output=True, timeout=timeout)
        if p.returncode:
            raise RuntimeError('Docker fixture failed: ' + p.stderr[-1000:])
        return p.stdout.strip()

    def shared(code):
        return docker('run', '--rm', '--pull', 'never', '--network', 'none',
                      '--label', 'sparkbox.staging=1',
                      '--mount', f'type=volume,source={volume},target=/dev/shm',
                      '--entrypoint', 'python3', IMAGE, '-c', code)

    def names():
        return json.loads(shared("import os,json; print(json.dumps(sorted(os.listdir('/dev/shm'))))"))

    try:
        docker('image', 'inspect', IMAGE)  # Refuse instead of pulling an image.
        docker('volume', 'create', '--label', 'sparkbox.staging=1', volume)
        made_volume = True
        shared("from pathlib import Path; Path('/dev/shm/unrelated-owner-file').write_text('keep')")
        with tempfile.TemporaryDirectory(prefix=prefix+'-') as temp:
            root = Path(temp)
            (root/'vllm').write_text(VLLM)
            (root/'vllm').chmod(0o755)
            (root/'exec.py').write_text(EXEC_CONTROL)
            key = 'sbk-fixture-' + secrets.token_hex(20)

            def start(role, old=False, with_key=False):
                record = root/role
                record.mkdir()
                (record/'launch.py').touch()  # Mountpoint inside the read-only parent mount.
                deploy = record/'deploy'
                deploy.mkdir()
                (deploy/'argv.json').write_text(json.dumps(['vllm', 'serve', '/model']))
                (deploy/'env.json').write_text('{}')
                if with_key:
                    (record/'key').write_text(key)
                    (record/'key').chmod(0o400)
                name = prefix+'-'+role
                containers.append(name)
                launcher = root/'exec.py' if old else REPO/'src-tauri/src/live/launch.py'
                # Staging normally starts systemd (SIGRTMIN+3); model images stop with TERM.
                args = ['run', '-d', '--pull', 'never', '--network', 'none', '--stop-signal', 'SIGTERM', '--name', name,
                        '--label', 'sparkbox.staging=1', '--label', 'sparkbox.instance=dev',
                        '--label', 'sparkbox.deployment='+prefix,
                        '--mount', f'type=volume,source={volume},target=/dev/shm',
                        '--mount', f'type=bind,source={root}/vllm,target=/usr/local/bin/vllm,readonly',
                        '--mount', f'type=bind,source={record},target=/record',
                        '--mount', f'type=bind,source={record},target=/run/sparkbox,readonly',
                        '--mount', f'type=bind,source={launcher},target=/run/sparkbox/launch.py,readonly',
                        '--entrypoint', 'python3', IMAGE, '/run/sparkbox/launch.py']
                if with_key:
                    args.append('--key')
                docker(*args)
                limit = time.monotonic()+15
                while not (record/'ready.json').exists():
                    if time.monotonic() > limit:
                        raise RuntimeError('Fixture never became ready: '+docker('logs', name))
                    time.sleep(.05)
                facts = json.loads((record/'ready.json').read_text())
                assert facts['keyHash'] == (hashlib.sha256(key.encode()).hexdigest() if with_key else None)
                assert key not in docker('inspect', name)
                return name, record, facts

            def stop(name):
                began = time.monotonic()
                docker('stop', '-t', '30', name)
                status = json.loads(docker('inspect', '--format', '{{json .State}}', name))
                assert status['Status']=='exited' and status['ExitCode']==0, status
                return round(time.monotonic()-began, 3)

            old, old_record, old_facts = start('old', old=True)
            assert old_facts['pid']==1
            stop(old)
            assert not (old_record/'child-finished').exists()
            assert names()==sorted(['unrelated-owner-file', old_facts['name']]), 'Control must reproduce the leak'
            print('PASS: exec/PID-1 control stranded its semaphore and killed the cleanup child.', flush=True)

            head, head_record, head_facts = start('head', with_key=True)
            worker, worker_record, worker_facts = start('worker')
            for facts in (head_facts, worker_facts):
                assert facts['pid']!=1 and facts['parent']==1
            head_time = stop(head)
            assert (head_record/'child-finished').exists()
            assert head_facts['name'] not in names()
            assert worker_facts['name'] in names(), 'A still-serving sibling is untouched'
            worker_time = stop(worker)
            assert (worker_record/'child-finished').exists()
            assert names()==sorted(['unrelated-owner-file', old_facts['name']]), 'Only owned trackers remove their names'
            for name in (head, worker):
                log = docker('logs', name)
                assert key not in log and 'child cleanup did not finish' not in log
            forced, forced_record, forced_facts = start('forced')
            docker('kill', '--signal', 'KILL', forced)
            assert docker('inspect', '--format', '{{.State.ExitCode}}', forced)=='137'
            assert not (forced_record/'child-finished').exists()
            assert names()==sorted(['unrelated-owner-file', old_facts['name'], forced_facts['name']])
            print(json.dumps({'ok':True, 'headStopSeconds':head_time, 'workerStopSeconds':worker_time,
                              'bothTrackersCleaned':True, 'unrelatedAndOldLeftoverPreserved':True,
                              'forcedKillStillLeavesEvidence':True,
                              'keyAbsentFromDockerAndLogs':True, 'network':'none'}), flush=True)
    finally:
        for name in reversed(containers):
            docker('rm', '-f', name)
        if made_volume:
            docker('volume', 'rm', volume)
        print('Fixture containers and private shared-memory volume removed.', flush=True)


if __name__ == '__main__':
    main()
