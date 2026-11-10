"""Offline tool consent, identity, secret handling and staged installation fixtures."""
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import sparkjob_test as fixture
sj = fixture.sj

HF_SCRIPT = '''#!/usr/bin/env python3
import json, os, pathlib, sys
a = sys.argv[1:]
if a == ["version"]:
    print("hf 2.4.0")
    raise SystemExit()
p = pathlib.Path(a[a.index("--local-dir")+1])
with open(os.environ["SPARKBOX_FIXTURE_LOG"], "w") as f:
    json.dump({"argv":a, "env":dict(os.environ), "tokenExists":pathlib.Path(os.environ["HF_TOKEN_PATH"]).is_file()},f)
(p/"config.json").write_bytes(b"{}")
(p/"model.safetensors").write_bytes(b"model")
'''

class DownloadTools(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="sparkbox-tools-", dir=fixture.TMP_HOME)
        self.home = Path(self.temp.name).resolve(); self.base = self.home/".sparkbox"; self.bin = self.home/"bin"; self.bin.mkdir()
        self.patches = [mock.patch.object(sj,"HOME",str(self.home)),mock.patch.object(sj,"JOBS",str(self.base/"jobs")),mock.patch.object(sj,"SECRETS",str(self.base/"job-secrets")),
                        mock.patch.dict(os.environ,{"HOME":str(self.home),"PATH":str(self.bin)+os.pathsep+os.environ["PATH"],"SPARKBOX_FIXTURE_LOG":str(self.home/"hf-run.json")})]
        for p in self.patches: p.start()
        (self.bin/"hf").write_text(HF_SCRIPT); (self.bin/"hf").chmod(0o700)
        self.tool=sj.executable_tool("host",str(self.bin/"hf"),"Existing hf on this node")
        self.spec={"repo":"org/model","revision":"a"*40,"dest":str(self.home/"model"),"image":"unused:configured","instance":"dev",
                   "files":[{"path":"config.json","size":2},{"path":"model.safetensors","size":5,"sha256":hashlib.sha256(b"model").hexdigest()}],"tool":self.tool,"token":"fake-private-token"}
        fixture.stand_in(str(self.bin),"docker",'#!/bin/sh\nexit 1\n')
    def tearDown(self):
        fixture.end_stand_ins(str(self.home))
        for p in reversed(self.patches): p.stop()
        self.temp.cleanup()
    def start(self,name="dl-tool",spec=None):
        sj.start("download",name,spec or self.spec)
        return fixture.wait_done(name)
    def test_discovery_reads_without_executing_tools_or_containers(self):
        before=sorted(str(p) for p in self.home.rglob("*")); inventory=sj.download_tools()
        self.assertIn(self.tool,inventory["options"])
        self.assertEqual(sorted(str(p) for p in self.home.rglob("*")),before)
        self.assertFalse((self.home/"hf-run.json").exists())
        self.assertTrue(any("No image will be pulled" in n for n in inventory["notes"]))
    def test_missing_changed_and_fabricated_tools_refuse_before_a_job_or_folder(self):
        for tool in [None,dict(self.tool,path="/bin/sh"),dict(self.tool,fingerprint="0"*64),dict(self.tool,extra="command")]:
            with self.subTest(tool=tool),self.assertRaisesRegex(RuntimeError,"Review the tool choice"):
                sj.start("download","refused",dict(self.spec,tool=tool))
        (self.bin/"hf").write_text(HF_SCRIPT+"\n# changed\n")
        with self.assertRaises(RuntimeError): sj.start("download","refused",self.spec)
        self.assertFalse((self.home/"model").exists()); self.assertFalse(Path(sj.status_path("refused")).exists())
    def test_host_download_is_detached_verified_and_keeps_tokens_private(self):
        with mock.patch.dict(os.environ,{"HF_TOKEN":"fake-inherited","HUGGING_FACE_HUB_TOKEN":"fake-inherited2","PYTHONPATH":"/unreviewed"}): result=self.start()
        self.assertEqual(result["state"],"done",result.get("message"))
        run=json.loads((self.home/"hf-run.json").read_text()); self.assertTrue(run["tokenExists"])
        for secret in ["fake-private-token","fake-inherited","fake-inherited2"]:
            self.assertNotIn(secret,json.dumps(run)); self.assertNotIn(secret,json.dumps(result))
        self.assertNotIn("PYTHONPATH",run["env"])
        self.assertEqual(run["argv"][run["argv"].index("--revision")+1],"a"*40)
        self.assertEqual(result["toolIdentity"],self.tool); self.assertEqual(result["toolVersion"],"hf 2.4.0")
        self.assertEqual(sj.read_manifest(str(self.home/"model"))["revision"],"a"*40)
        self.assertFalse((self.home/"model"/sj.PARTIAL).exists()); self.assertFalse((self.base/"job-secrets"/"dl-tool").exists())
        self.assertFalse(list(self.base.glob("hf-probe-*")))
    def test_install_consent_is_per_attempt_and_refusal_never_fetches_or_writes(self):
        choice=sj.install_tool_choice()
        with mock.patch.object(sj,"download_tools",return_value={"options":[choice]}),mock.patch("urllib.request.urlopen") as network:
            for consent in [False,None,"yes",1]:
                with self.assertRaisesRegex(RuntimeError,"explicit consent"):
                    sj.start("download","no-consent",dict(self.spec,tool=choice,installConsent=consent))
            network.assert_not_called()
        self.assertFalse(self.base.exists())
    def slow_host(self):
        script=HF_SCRIPT.replace('p = pathlib.Path', 'import time; time.sleep(60)\np = pathlib.Path')
        (self.bin/"hf").write_text(script)
        self.tool=sj.executable_tool("host",str(self.bin/"hf"),"Existing hf on this node")
        self.spec["tool"]=self.tool
        sj.start("download","slow-host",self.spec)
        def child_started():
            st=sj.read_status("slow-host")
            facts=sj.process_facts(st.get("childPid"))
            return st if facts and sj.hf_to(facts["argv"],str(self.home/"model")) else None
        return fixture.wait_for("the host download",child_started)
    def test_cancel_host_stops_its_process_and_secrets_even_when_docker_is_down(self):
        self.slow_host()
        st=sj.read_status("slow-host");pid=st["childPid"]
        result=sj.cancel("slow-host")
        fixture.wait_for("host child to stop",lambda:not sj.process_facts(pid))
        self.assertEqual((result["state"],result["message"]),("cancelled","Canceled"))
        self.assertFalse((self.base/"job-secrets"/"slow-host").exists())
        self.assertTrue((self.home/"model"/sj.PARTIAL).exists())
    def test_killed_host_is_owned_before_cleanup_and_does_not_require_docker(self):
        import signal
        self.slow_host();st=sj.read_status("slow-host")
        os.kill(st["pid"],signal.SIGKILL)
        os.waitpid(st["pid"], 0)
        self.assertFalse(sj.alive(st["pid"]))
        self.assertIsNotNone(sj.orphan(st))
        with self.assertRaises(RuntimeError):sj.stop_leftover(str(self.home/"model"),"another-instance")
        self.assertIsNotNone(sj.process_facts(st["childPid"]),"foreign cleanup must not signal it")
        sj.stop_leftover(str(self.home/"model"),"dev")
        fixture.wait_for("owned child cleanup",lambda:not sj.process_facts(st["childPid"]))
    def test_a_bad_host_version_stops_without_download_or_fallback(self):
        (self.bin/"hf").write_text('#!/bin/sh\nexit 1\n')
        self.spec["tool"]=sj.executable_tool("host",str(self.bin/"hf"),"Existing hf on this node")
        result=self.start("bad-version")
        self.assertEqual(result["state"],"failed")
        self.assertIn("no fallback",result["message"])
        self.assertFalse((self.home/"hf-run.json").exists())
        self.assertFalse((self.base/"venv").exists())

    def test_empty_version_cannot_reuse_old_installer_output(self):
        sj.write_status("empty-version",kind="download",state="running")
        log=Path(sj.JOBS)/"empty-version.log";log.write_text("Installed huggingface_hub 2.4.0\n")
        with mock.patch.object(sj,"check_download_tool",return_value=self.tool),mock.patch.object(sj,"run_watched",return_value=(0,"")):
            with self.assertRaisesRegex(RuntimeError,"did not report an hf version"):
                sj.prepare_download_tool("empty-version",self.spec)
        self.assertEqual(log.read_text(),"Installed huggingface_hub 2.4.0\n")
        self.assertFalse(list(self.base.glob("hf-probe-*")))

    def archive(self):
        script=('#!/usr/bin/env python3\nimport os,pathlib,sys,json\na=sys.argv[1:]\n'
                'with open(os.environ["SPARKBOX_FIXTURE_LOG"]+".uv","a") as f: f.write(json.dumps({"argv":a,"env":dict(os.environ)})+"\\n")\n'
                'if a[0]=="venv":\n p=pathlib.Path(a[-1]); (p/"bin").mkdir(parents=True); (p/"bin"/"python").symlink_to(sys.executable)\n'
                'else:\n p=pathlib.Path(a[a.index("--python")+1]).parent; (p/"hf").write_text('+repr(HF_SCRIPT)+'); (p/"hf").chmod(0o700)\n').encode()
        output=io.BytesIO()
        with tarfile.open(fileobj=output,mode="w:gz") as tar:
            item=tarfile.TarInfo("uv-aarch64-unknown-linux-gnu/uv");item.size=len(script);item.mode=0o700;tar.addfile(item,io.BytesIO(script))
        return output.getvalue()
    def install(self):
        payload=self.archive(); sj.write_status("install",kind="download",state="running",instance="dev")
        with mock.patch("urllib.request.urlopen",side_effect=lambda *a,**k:io.BytesIO(payload)) as network,mock.patch.object(sj,"UV_SHA256",hashlib.sha256(payload).hexdigest()):
            tool=sj.install_download_tool("install","dev");network.assert_called_once_with(sj.UV_URL,timeout=30);return tool
    def test_managed_install_is_pinned_staged_verified_and_runs_after_publication(self):
        tool=self.install();self.assertEqual(tool,sj.managed_tool());self.assertEqual(tool["kind"],"managed")
        self.assertTrue((self.base/"bin"/('uv-'+sj.UV_VERSION)).is_file());self.assertFalse(list(self.base.glob("tool-install-*")))
        calls=[json.loads(line) for line in (self.home/"hf-run.json.uv").read_text().splitlines()];self.assertEqual(len(calls),2)
        self.assertIn("--relocatable",calls[0]["argv"]);self.assertIn("--no-python-downloads",calls[0]["argv"])
        self.assertIn("huggingface_hub[hf_xet]=="+sj.HF_VERSION,calls[1]["argv"]);self.assertIn("hf-xet=="+sj.XET_VERSION,calls[1]["argv"])
        self.assertIn("--only-binary",calls[1]["argv"]);self.assertEqual(calls[1]["env"]["UV_NO_CONFIG"],"1")
        self.assertNotIn("sudo",json.dumps(calls));self.assertNotIn("fake-private-token",json.dumps(calls))
        result=self.start("managed-download",dict(self.spec,tool=tool));self.assertEqual(result["state"],"done",result.get("message"))
    def test_bad_checksum_never_executes_and_cleans_staging(self):
        self.base.mkdir()
        with mock.patch("urllib.request.urlopen",return_value=io.BytesIO(b"wrong")),mock.patch.object(sj,"run_watched") as run:
            with self.assertRaisesRegex(RuntimeError,"checksum"):sj.install_download_tool("bad-checksum","dev")
            run.assert_not_called()
        self.assertFalse((self.base/"venv").exists());self.assertFalse(list(self.base.glob("tool-bad-checksum-*")))
    def test_existing_or_symlinked_environment_is_never_replaced(self):
        (self.base/"venv").mkdir(parents=True);keep=self.base/"venv"/"keep";keep.write_text("owner data")
        with mock.patch("urllib.request.urlopen") as network:
            with self.assertRaisesRegex(RuntimeError,"will not be replaced"):sj.install_download_tool("existing","dev")
            network.assert_not_called()
        self.assertEqual(keep.read_text(),"owner data")
        shutil.rmtree(self.base/"venv");(self.base/"venv").symlink_to(self.bin)
        with self.assertRaises(RuntimeError):sj.install_download_tool("linked","dev")
        self.assertTrue((self.bin/"hf").is_file())
    def test_cancel_and_installer_failure_clean_only_new_staging(self):
        self.base.mkdir();keep=self.base/"keep";keep.write_text("owner data");payload=self.archive()
        for failure in [sj.Stopped(),RuntimeError("fixture failure")]:
            with mock.patch("urllib.request.urlopen",side_effect=lambda *a,**k:io.BytesIO(payload)),mock.patch.object(sj,"UV_SHA256",hashlib.sha256(payload).hexdigest()),mock.patch.object(sj,"run_watched",side_effect=failure):
                with self.assertRaises(type(failure)):sj.install_download_tool("interrupted","dev")
            self.assertFalse((self.base/"venv").exists());self.assertFalse(list(self.base.glob("tool-interrupted-*")));self.assertEqual(keep.read_text(),"owner data")
    def test_install_lock_refuses_a_second_writer(self):
        import fcntl
        self.base.mkdir()
        with (self.base/".download-tool.lock").open("w") as lock:
            fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
            with mock.patch("urllib.request.urlopen") as network:
                with self.assertRaisesRegex(RuntimeError,"still running"):sj.install_download_tool("other","dev")
                network.assert_not_called()
    def test_only_the_matching_host_writer_or_owned_installer_is_recognized(self):
        argv=[str(self.bin/"hf"),"download","org/model","--local-dir",str(self.home/"model")]
        self.assertTrue(sj.hf_to(argv,str(self.home/"model")));self.assertFalse(sj.hf_to(argv,str(self.home/"other")))
        with mock.patch.object(sj,"process_ids",return_value=[4242]),mock.patch.object(sj,"process_facts",return_value={"argv":argv,"uid":os.getuid(),"start":7}):
            self.assertEqual(sj.writers(str(self.home/"model"),None),[4242])
            with self.assertRaisesRegex(RuntimeError,"still be writing"):
                sj.stop_leftover(str(self.home/"model"),"dev")
        st={"toolStage":str(self.base/"tool-old")}
        self.assertTrue(sj.tool_install_child([str(self.base/"tool-old"/"uv"),"pip","install"],st))
        self.assertFalse(sj.tool_install_child(["/some/other/uv","pip","install"],st))

if __name__ == "__main__":unittest.main()
