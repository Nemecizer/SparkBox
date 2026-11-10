"""Offline chains: real detached workers/loopback rsync, fixture hf, temporary HOME only."""
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import tempfile
import time
import unittest
from unittest import mock
import sparkjob_test as fixture
from download_tools_test import HF_SCRIPT
sj = fixture.sj

class DownloadChain(unittest.TestCase):
    def setUp(self):
        self.base = Path(tempfile.mkdtemp(prefix="chain-", dir=fixture.TMP_HOME)).resolve()
        self.source, self.target = self.base/"source", self.base/"target"
        self.bin = self.base/"bin"; self.bin.mkdir()
        (self.bin/"hf").write_text(HF_SCRIPT); (self.bin/"hf").chmod(0o700)
        self.env = mock.patch.dict(os.environ, {"PATH":str(self.bin)+os.pathsep+os.environ["PATH"], "SPARKBOX_FIXTURE_LOG":str(self.base/"hf-log")})
        self.env.start()
        self.id = "dc-" + os.urandom(4).hex()
        self.chain = {"id":self.id,"source":"cell1","repo":"fixture/model","revision":"a"*40,"expiresAt":sj.now()+3600,
            "members":[{"node":"cell1","id":self.id+"-0","dest":str(self.source)},{"node":"cell2","id":self.id+"-1","dest":str(self.target)}]}
        self.spec = {"chain":self.chain,"node":"cell1","repo":"fixture/model","revision":"a"*40,"dest":str(self.source),"instance":"dev","image":"unused:fixture",
            "files":[{"path":"config.json","size":2},{"path":"model.safetensors","size":5,"sha256":hashlib.sha256(b"model").hexdigest()}],
            "tool":sj.executable_tool("host",str(self.bin/"hf"),"Existing hf on this node"),"password":"f"*32,"token":"fixture-private-token",
            "address":"192.0.2.1","allow":"192.0.2.2","port":8730,"protocol":2}
    def tearDown(self):
        for m in self.chain["members"]:
            if sj.read_status(m["id"]).get("state") == "running": sj.cancel(m["id"])
        fixture.end_stand_ins(str(self.base))
        self.env.stop(); shutil.rmtree(self.base,ignore_errors=True)
    def test_preflight_is_read_only_and_rejects_existing_and_symlink_paths(self):
        with mock.patch.object(sj,"fabric",return_value=[]):
            self.assertEqual(sj.chain_preflight(dict(self.spec,bytes=7))["dest"],str(self.source))
        self.assertFalse(self.source.exists())
        self.source.mkdir(); (self.source/"keep").write_text("saved")
        with self.assertRaisesRegex(RuntimeError,"empty"): sj.chain_empty(str(self.source))
        link=self.base/"link"; link.symlink_to(self.source)
        with self.assertRaisesRegex(RuntimeError,"symlinks"): sj.chain_empty(str(link/"new"))
        self.assertEqual((self.source/"keep").read_text(),"saved")
    def test_plan_roles_deadlines_and_loopback_refuse_before_writes(self):
        for change in [dict(expiresAt=sj.now()-1),dict(source="cell2"),dict(members=self.chain["members"]*2)]:
            with self.assertRaises(RuntimeError): sj.checked_chain(self.id+"-0",dict(self.spec,chain=dict(self.chain,**change)),"download")
        with self.assertRaises(RuntimeError): sj.checked_chain(self.id+"-0",self.spec,"pull")
        with self.assertRaisesRegex(RuntimeError,"fabric"): sj.checked_chain(self.id+"-0",dict(self.spec,address="127.0.0.1"),"download")
        self.assertFalse(self.source.exists()); self.assertFalse(Path(sj.status_path(self.id+"-0")).exists())
    def test_manifest_requires_exact_chain_revision_files_sizes_and_checksums(self):
        manifest={"chainId":self.id,"repo":self.chain["repo"],"revision":self.chain["revision"],"files":[dict(f,sha256=f.get("sha256",hashlib.sha256(b"{}").hexdigest())) for f in self.spec["files"]]}
        sj.chain_manifest(self.spec,manifest)
        for field in ["chainId","repo","revision"]:
            with self.assertRaises(RuntimeError): sj.chain_manifest(self.spec,dict(manifest,**{field:"changed"}))
        for change in [dict(path="other"),dict(size=9),dict(sha256="0"*64)]:
            changed=copy.deepcopy(manifest); changed["files"][1].update(change)
            with self.assertRaises(RuntimeError): sj.chain_manifest(self.spec,changed)
        with self.assertRaises(RuntimeError): sj.chain_manifest(self.spec,dict(manifest,files=manifest["files"][:1]))
    def test_source_verification_failure_never_opens_listener(self):
        with mock.patch.object(sj,"verify",return_value=["checksum mismatch"]), mock.patch.object(sj,"job_serve_v2") as serve:
            with self.assertRaisesRegex(RuntimeError,"incomplete"): sj.job_download(self.id+"-0",copy.deepcopy(self.spec))
        serve.assert_not_called(); self.assertFalse((self.source/sj.MANIFEST).exists())
    def test_download_removes_hub_token_before_sharing(self):
        def served(job,spec):
            self.assertNotIn("token",spec)
            self.assertFalse(Path(sj.SECRETS,job).exists())
            self.assertEqual(spec["expected"],[self.id+"-1"])
            self.assertEqual(sj.read_manifest(str(self.source))["revision"],self.chain["revision"])
            self.assertFalse((self.source/sj.PARTIAL).exists())
            return {"copiesFinished":1}
        with mock.patch.object(sj,"job_serve_v2",side_effect=served) as serve:
            sj.job_download(self.id+"-0",copy.deepcopy(self.spec))
        serve.assert_called_once()
    def test_admission_recheck_under_writer_lock_preserves_new_files(self):
        self.target.mkdir(); (self.target/"keep").write_text("saved")
        os.makedirs(sj.JOBS,exist_ok=True)
        with mock.patch.object(sj,"stop_leftover") as stop, self.assertRaisesRegex(RuntimeError,"empty"):
            sj.claim_folder(str(self.target),self.id+"-1","pull","dev",disk_check=lambda:sj.chain_empty(str(self.target)))
        stop.assert_not_called(); self.assertEqual((self.target/"keep").read_text(),"saved")

    def run_chain(self, fault=None):
        # Only listener address is translated for this isolated fixture; production admission
        # still rejects loopback above. All detached download/manifest/copy/ack code is real.
        port=None
        for p in range(8730,8930):
            with socket.socket() as sock:
                try: sock.bind(("127.0.0.1",p)); port=p; break
                except OSError: pass
        self.assertIsNotNone(port)
        source=copy.deepcopy(self.spec); source.update(address="127.0.0.1",allow="127.0.0.1",port=port,idleSeconds=5)
        pull=dict(source,node="cell2",dest=str(self.target),src="rsync://sparkbox@127.0.0.1:%d/model"%port)
        checked=sj.checked_chain
        def local_checked(job,spec,kind):
            return checked(job,dict(spec,address="192.0.2.1",allow="192.0.2.2") if kind=="download" else spec,kind)
        if fault == "expiry":
            self.chain["expiresAt"]=sj.now()+3
            source["chain"]=pull["chain"]=self.chain
            (self.bin/"hf").write_text(HF_SCRIPT.replace('p = pathlib.Path','import time; time.sleep(60)\np = pathlib.Path'))
            source["tool"]=sj.executable_tool("host",str(self.bin/"hf"),"Existing hf on this node")
        with mock.patch.object(sj,"checked_chain",side_effect=local_checked):
            if fault in (None,"expiry"):
                sj.start("pull",self.id+"-1",pull)
                self.assertEqual(sj.read_status(self.id+"-1")["chain"],self.chain)
            sj.start("download",self.id+"-0",source)
            if fault != "expiry":
                fixture.wait_for("chain source listener",lambda:sj.read_status(self.id+"-0").get("step")=="serving",timeout=15)
                self.assertEqual(sj.read_status(self.id+"-0")["rateBps"],0)
                if fault == "corrupt": (self.source/"model.safetensors").write_bytes(b"other")
                if fault == "changed-destination": self.target.mkdir(); (self.target/"keep").write_text("saved")
                if fault == "corrupt": sj.start("pull",self.id+"-1",pull)
                if fault == "changed-destination":
                    # It was empty when setup was reviewed. Rechecking before dispatch refuses.
                    with self.assertRaisesRegex(RuntimeError,"empty"): sj.start("pull",self.id+"-1",pull)
            # No application polling/dispatch: detached workers finish their own phases.
            time.sleep(4)
            source_result=fixture.wait_done(self.id+"-0",timeout=20)
            if fault != "changed-destination":
                result=fixture.wait_done(self.id+"-1",timeout=20)
                self.assertEqual(result["state"],"failed" if fault else "done",result)
                self.assertNotIn(self.spec["password"],json.dumps(result)); self.assertNotIn(self.spec["token"],json.dumps(result))
            self.assertEqual(source_result["state"],"failed" if fault else "done",source_result)
            if not fault:
                self.assertEqual(sj.read_manifest(str(self.target))["checked"],"source-sha256")
                self.assertEqual((self.target/"model.safetensors").read_bytes(),b"model")
            else: self.assertFalse((self.target/sj.MANIFEST).exists())
            if fault=="changed-destination": self.assertEqual((self.target/"keep").read_text(),"saved")
            for m in self.chain["members"]:
                self.assertFalse(Path(sj.SECRETS,m["id"]).exists()); self.assertFalse(Path(sj.RSYNCD,m["id"]).exists())
    def test_detached_download_verify_copy_and_secret_cleanup(self): self.run_chain()
    def test_copy_corruption_fails_source_chain_and_preserves_unverified_target(self): self.run_chain("corrupt")
    def test_changed_destination_refuses_and_source_expires_without_false_success(self): self.run_chain("changed-destination")
    def test_deadline_stops_slow_download_and_waiting_target(self): self.run_chain("expiry")

if __name__ == "__main__": unittest.main()
