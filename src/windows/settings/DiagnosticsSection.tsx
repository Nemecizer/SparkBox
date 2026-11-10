import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FormGroup, FormRow, HelpTip, Notice } from "../../components/ui";
import { ADMIN_REFUSED, isLookOnly, LOOK_ONLY_TIP, useAdminState, withAdmin } from "../../lib/admin";
import { api, inTauri } from "../../lib/api";
import { useDataSource } from "../../lib/dataSource";
import { captureEvidence, evidenceSummary, listEvidence, type EvidenceFolder } from "../../lib/evidence";
import { fmtDateTime } from "../../lib/format";
import { confirmAction } from "../../lib/session";

const errorText = (e: unknown) => String((e as { message?: string })?.message ?? e);

/**
 * Settings › Diagnostics › Evidence: captures every connected node (reads only, so look-only
 * mode allows it) into ~/Library/Logs/SparkBox/evidence, and lists the last five captures.
 */
function EvidenceGroup() {
  const live = useDataSource() === "live";
  const [phase, setPhase] = useState("general");
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ made?: EvidenceFolder; error?: string } | null>(null);
  const [recent, setRecent] = useState<EvidenceFolder[]>([]);

  useEffect(() => {
    if (!inTauri || !live) return;
    listEvidence().then(setRecent, () => setRecent([]));
  }, [live, result]);

  const capture = async () => {
    setBusy(true);
    try {
      setResult({ made: await captureEvidence(phase.trim() || "general", label) });
    } catch (e) {
      setResult({ error: errorText(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <FormGroup
      title={<>Evidence <HelpTip topic="capture-evidence" /></>}
      footnote="Reads each connected node (containers, images, failed services, disk, models, apt, packages, GPU, addresses, listening ports, memory, /dev/shm) and saves it with a diff against the capture before it, the activity log and the SSH log, in ~/Library/Logs/SparkBox/evidence. Nothing on the nodes changes. Secrets are masked."
    >
      <FormRow label={<>Group <HelpTip topic="ops.evidence.phase" /></>} htmlFor="evidence-phase" hint="Captures in the same group are compared with each other">
        <input id="evidence-phase" className="mono" value={phase} onChange={(e) => setPhase(e.target.value)} disabled={!live} />
      </FormRow>
      <FormRow label={<>Label <HelpTip topic="ops.evidence.label" /></>} htmlFor="evidence-label" hint="For example before-update or after-reboot">
        <input id="evidence-label" value={label} onChange={(e) => setLabel(e.target.value)} disabled={!live} />
      </FormRow>
      <FormRow label="Capture" hint={live ? (busy ? "Reading every node; this can take a few minutes" : undefined) : "Available with Live data"}>
        <button type="button" className="btn" onClick={capture} disabled={busy || !live}>{busy ? "Capturing…" : "Capture evidence"}</button>
      </FormRow>
      {result?.made && (
        <FormRow label="Saved" hint={evidenceSummary(result.made)}>
          <span className="fg-inline">
            <span className="mono fg-path" title={result.made.path}>{result.made.path}</span>
            <button type="button" className="btn" onClick={() => void revealItemInDir(result.made!.path)}>Show in Finder</button>
          </span>
        </FormRow>
      )}
      {result?.made?.problems?.length ? (
        <FormRow label="Problems">
          <span className="error-text">{result.made.problems.join(" · ")}</span>
        </FormRow>
      ) : null}
      {result?.error && <FormRow label="Failed"><span className="error-text">{result.error}</span></FormRow>}
      {live && recent.slice(0, 5).map((f) => (
        <FormRow key={f.path} label={`${f.phase} · ${f.label}`} hint={`${fmtDateTime(f.startedMs)} · ${evidenceSummary(f)}`}>
          <button type="button" className="btn" onClick={() => void revealItemInDir(f.path)}>Show in Finder</button>
        </FormRow>
      ))}
    </FormGroup>
  );
}

export function DiagnosticsSection() {
  // Look-only mode holds the doctor (it runs checks on every node and writes a file) and
  // forgetting the host keys (a change on the Mac).
  const lookOnly = isLookOnly(useAdminState());
  // In Demo the doctor would log in to the real nodes: it runs with Live data only.
  const live = useDataSource() === "live";
  const [doctor, setDoctor] = useState<{ busy: boolean; path?: string; error?: string }>({ busy: false });
  const [msg, setMsg] = useState<{ tone?: "error" | "success"; text: string } | null>(null);

  const runDoctor = async () => {
    setDoctor({ busy: true });
    try {
      setDoctor({ busy: false, path: await api.doctorRun() });
    } catch (e) {
      setDoctor({ busy: false, error: String((e as { message?: string })?.message ?? e) });
    }
  };

  const forgetKeys = async () => {
    const ok = await confirmAction({
      title: "Forget the pinned SSH host keys?",
      body: "Only do this after reinstalling a node. Each node then waits for you to trust it again, unless ~/.ssh/known_hosts already holds that exact key.",
      confirmLabel: "Forget keys",
      danger: true,
    });
    if (!ok) return;
    withAdmin(() => api.knownHostsForget(), { scope: "mac", action: "known_hosts_forget" }).then(
      () => setMsg({ tone: "success", text: "Host keys forgotten. Each node waits for you to trust it again, unless ~/.ssh/known_hosts already holds that exact key." }),
      (e) => e !== ADMIN_REFUSED && setMsg({ tone: "error", text: String((e as { message?: string })?.message ?? e) }),
    );
  };

  return (
    <>
      {msg && <Notice tone={msg.tone} onDismiss={() => setMsg(null)}>{msg.text}</Notice>}
      <FormGroup
        title={<>Doctor report <HelpTip topic="collect-diagnostics" /></>}
        footnote="Collects what each node reports (OS, nvidia-smi, memory, disks, network, Docker, sshd's MaxSessions, machine-id, Docker's image store, GRUB_DEFAULT, ufw, apt and dgx-release, and the last reading SparkBox received) into a text file in Downloads, so problems can be fixed from real output. With Admin unlocked it also reads sshd's own settings with sudo (sshd -T). It contains no passwords; skim it before sharing."
      >
        <FormRow label="Report" hint={lookOnly ? "Not in look-only mode: it runs checks on every node" : !live ? "Available with Live data: it reads the real nodes" : doctor.path ? undefined : "Takes about ten seconds"}>
          <span className="fg-inline">
            <button type="button" className="btn" onClick={runDoctor} disabled={doctor.busy || lookOnly || !live}>{doctor.busy ? "Collecting…" : "Run doctor"}</button>
            {doctor.path && (
              <button type="button" className="btn" onClick={() => revealItemInDir(doctor.path!)}>Show in Finder</button>
            )}
          </span>
        </FormRow>
        {doctor.path && <FormRow label="Saved to"><span className="mono fg-path" title={doctor.path}>{doctor.path}</span></FormRow>}
        {doctor.error && <FormRow label="Failed"><span className="error-text">{doctor.error}</span></FormRow>}
      </FormGroup>
      <EvidenceGroup />
      <FormGroup
        title={<>SSH host keys <HelpTip topic="trust-a-host-key" /></>}
        footnote="SparkBox connects to a node once you trust its host key, or when ~/.ssh/known_hosts already holds that exact key, and it refuses a node whose key changes. Forget the keys only after reinstalling a node."
      >
        <FormRow label="Pinned keys">
          <button type="button" className="btn" onClick={forgetKeys} disabled={lookOnly} title={lookOnly ? LOOK_ONLY_TIP : undefined}>Forget host keys…</button>
        </FormRow>
      </FormGroup>
    </>
  );
}
