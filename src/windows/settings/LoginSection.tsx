import { useCallback, useEffect, useId, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { KeyRound, Lock, TriangleAlert } from "lucide-react";
import { FormGroup, FormRow, HelpTip, Notice, Segmented } from "../../components/ui";
import { ADMIN_REFUSED, withAdmin } from "../../lib/admin";
import { api, inTauri, SSH_ACCOUNT } from "../../lib/api";
import { confirmAction } from "../../lib/session";
import { toast } from "../../lib/toasts";
import { InventorySaveBar, useInventoryDraft } from "./NodesSection";
import "./login.css";

/** Settings › Cluster login's login options (src-tauri/src/live/sshconfig.rs). */
export type AgentChoice = "system" | "onePassword" | "secretive" | "off";

export interface LoginSettings {
  agent: AgentChoice;
  identityFile: string | null;
  useSshConfig: boolean;
  passwordFallback: boolean;
}

export interface NodeLogin {
  node: string;
  name: string;
  method: "agent" | "key" | "password" | null;
  detail: string | null;
  /** "cell1 · key ~/.ssh/id_ed25519 via agent" */
  summary: string;
  agent: string | null;
  agentRunning: boolean;
  keys: string[];
  password: boolean;
  jump: string | null;
  notes: string[];
}

const DEFAULTS: LoginSettings = { agent: "system", identityFile: null, useSshConfig: true, passwordFallback: true };

export const loginApi = {
  settings: () => (inTauri ? invoke<LoginSettings>("login_settings_get") : Promise.resolve(DEFAULTS)),
  save: (settings: LoginSettings) => invoke<LoginSettings>("login_settings_set", { settings }),
  methods: () => invoke<NodeLogin[]>("login_methods"),
  setPassphrase: (path: string, passphrase: string | null) => invoke<void>("login_passphrase_set", { path, passphrase }),
  setupKey: (nodes: string[], keyPath: string, generate: boolean, disablePassword: boolean) =>
    invoke<KeySetup[]>("login_setup_key", { nodes, keyPath, generate, disablePassword }),
};

export interface KeySetup {
  node: string;
  verified: boolean;
  passwordOff: boolean;
  message: string;
}

/** Adds this Mac's key to the chosen nodes, proves it works, then optionally turns password login off. */
function KeySetupPanel({ nodes, onDone, onClose }: { nodes: NodeLogin[]; onDone: () => void; onClose: () => void }) {
  const [key, setKey] = useState("~/.ssh/id_ed25519");
  const [generate, setGenerate] = useState(false);
  const [chosen, setChosen] = useState<Set<string>>(new Set(nodes.filter((n) => n.method === "password").map((n) => n.node)));
  const [off, setOff] = useState(false);
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<KeySetup[] | null>(null);
  const run = async () => {
    // Every node losing password login at once: say so, and have the count typed.
    if (off && chosen.size === nodes.length && nodes.length > 1) {
      const ok = await confirmAction({
        title: `Turn password login off on all ${nodes.length} nodes?`,
        body: "From then on only keys get in over SSH, on every node. SparkBox checks each key works first, and turns password login off only where it did and sshd confirms it.",
        confirmLabel: "Turn it off",
        danger: true,
        requireTyping: `${nodes.length} nodes`,
      });
      if (!ok) return;
    }
    setBusy(true);
    try {
      const r = await withAdmin(() => loginApi.setupKey([...chosen], key, generate, off), { action: "login_setup_key" });
      setResults(r);
      onDone();
    } catch (e) {
      setResults([{ node: "", verified: false, passwordOff: false, message: String((e as { message?: string })?.message ?? e) }]);
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormGroup title={<span className="login-title">Set up key login <HelpTip topic="set-up-key-login" /></span>}>
      <FormRow label="Key" htmlFor="setup-key" hint="Its .pub goes into each node's ~/.ssh/authorized_keys">
        <input id="setup-key" className="mono" value={key} spellCheck={false} onChange={(e) => setKey(e.target.value)} />
      </FormRow>
      <FormRow label="New key">
        <label className="check">
          <input type="checkbox" checked={generate} onChange={(e) => setGenerate(e.target.checked)} />
          Create it (ed25519) if it doesn't exist
        </label>
      </FormRow>
      <FormRow label="Nodes">
        <span className="login-nodes">
          {nodes.map((n) => (
            <label key={n.node} className="check">
              <input type="checkbox" checked={chosen.has(n.node)} onChange={(e) => {
                const next = new Set(chosen);
                if (e.target.checked) next.add(n.node);
                else next.delete(n.node);
                setChosen(next);
              }} />
              {n.name}
            </label>
          ))}
        </span>
      </FormRow>
      <FormRow label={<>Password login <HelpTip topic="start.login.password-off" /></>}>
        <label className="check">
          <input type="checkbox" checked={off} onChange={(e) => setOff(e.target.checked)} />
          Then turn it off where the key works
        </label>
      </FormRow>
      {off && (
        <div className="fg-row">
          <span className="login-caution"><TriangleAlert size={13} aria-hidden /> With password login off, only keys get in over SSH: keep this key (and a backup) safe. sudo still uses the password.</span>
        </div>
      )}
      {results && (
        <ul className="login-results">
          {results.map((r) => (
            <li key={r.node || "error"} className={r.verified ? undefined : "crit-text"}>
              {r.node ? `${nodes.find((n) => n.node === r.node)?.name ?? r.node}: ` : ""}{r.message}
            </li>
          ))}
        </ul>
      )}
      <div className="fg-row login-save">
        <button type="button" className="btn" onClick={onClose}>Close</button>
        <button type="button" className="btn primary" disabled={busy || !chosen.size || !key.trim()} onClick={() => void run()}>
          {busy ? "Setting up…" : `Set up on ${chosen.size} node${chosen.size === 1 ? "" : "s"}`}
        </button>
      </div>
    </FormGroup>
  );
}

const METHOD_WORD: Record<NonNullable<NodeLogin["method"]>, string> = { agent: "SSH key (agent)", key: "SSH key", password: "Password" };

/**
 * One neutral line under the nodes: the shared password is how the owner chose to log in, and
 * key login stays one click away. Never a warning.
 */
export function loginFootnote(nodes: Pick<NodeLogin, "method">[], openSetup: () => void, setupOpen = false) {
  if (!nodes.length) return undefined;
  const all = nodes.every((n) => n.method === "password");
  const text = all
    ? "SparkBox logs in to every node with your shared cluster password. Key login is optional."
    : nodes.some((n) => n.method === "password")
      ? "Nodes without a key log in with your shared cluster password. Key login is optional."
      : "Every node logs in with an SSH key; sudo still uses the cluster password.";
  return (
    <span className="login-foot">
      {text}
      {!setupOpen && <> <button type="button" className="link" onClick={openSetup}>Set up key login…</button></>}
    </span>
  );
}

type Msg = { tone?: "error"; text: string } | null;

/** Which Keychain accounts hold a secret. `refresh` re-reads after a change. */
export function useCredentialStatus(accounts: string[]): { status: Record<string, boolean>; error: string | null; refresh: () => void } {
  const key = accounts.join("\n");
  const [status, setStatus] = useState<Record<string, boolean>>({});
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(() => {
    if (!key) return;
    api.credentialStatus(key.split("\n")).then(
      (s) => {
        setStatus(s);
        setError(null);
      },
      (e) => setError(`Couldn't read the Keychain: ${e}`),
    );
  }, [key]);
  useEffect(refresh, [refresh]);
  return { status, error, refresh };
}

/**
 * A password box that writes straight to the macOS Keychain. The secret never
 * comes back to the UI: the field only shows whether one is saved.
 */
export function KeychainControl({ account, id, label, saved, onChange }: {
  account: string;
  id?: string;
  label: string;
  saved: boolean | undefined;
  onChange: () => void;
}) {
  const [value, setValue] = useState("");
  const [msg, setMsg] = useState<Msg>(null);

  const save = async () => {
    try {
      await withAdmin(() => api.setCredential(account, value), { scope: "mac", action: "set_credential" });
      setValue("");
      setMsg({ text: "Saved to Keychain." });
      onChange();
    } catch (e) {
      if (e !== ADMIN_REFUSED) setMsg({ tone: "error", text: String((e as { message?: string })?.message ?? e) });
    }
  };
  const remove = async () => {
    try {
      await withAdmin(() => api.clearCredential(account), { scope: "mac", action: "clear_credential" });
      setMsg({ text: "Removed from Keychain." });
      onChange();
    } catch (e) {
      if (e !== ADMIN_REFUSED) setMsg({ tone: "error", text: String((e as { message?: string })?.message ?? e) });
    }
  };

  return (
    <span className="keychain">
      <input
        id={id}
        type="password"
        value={value}
        aria-label={id ? undefined : label}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && value && save()}
        placeholder={saved ? "Saved in Keychain" : "Not saved"}
        autoComplete="off"
      />
      <button type="button" className="btn primary" disabled={!value} onClick={save}>Save</button>
      <button type="button" className="btn" disabled={!saved} onClick={remove}>Remove</button>
      {msg && <span className={msg.tone === "error" ? "error-text" : "muted"} role="status">{msg.text}</span>}
    </span>
  );
}

/** The older one-line layout (label, field, buttons), kept for windows that embed it. */
export function KeychainField({ account, label, saved, onChange }: {
  account: string;
  label: string;
  saved: boolean | undefined;
  onChange: () => void;
}) {
  const id = useId();
  return (
    <div className="form-row">
      <label htmlFor={id}>{label}</label>
      <KeychainControl account={account} id={id} label={label} saved={saved} onChange={onChange} />
    </div>
  );
}

/** How each node logs in, and the options for it: agent, key file, ~/.ssh/config, password last. */
function LoginMethods() {
  const [settings, setSettings] = useState<LoginSettings | null>(null);
  const [draft, setDraft] = useState<LoginSettings | null>(null);
  const [nodes, setNodes] = useState<NodeLogin[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [passphrase, setPassphrase] = useState("");
  const [setup, setSetup] = useState(false);
  const load = useCallback(() => {
    loginApi.settings().then((s) => {
      setSettings(s);
      setDraft(s);
    }, (e) => setError(String(e)));
    loginApi.methods().then(setNodes, (e) => setError(String(e)));
  }, []);
  useEffect(load, [load]);
  // The methods change as nodes connect.
  useEffect(() => {
    const t = window.setInterval(() => loginApi.methods().then(setNodes, () => {}), 5000);
    return () => window.clearInterval(t);
  }, []);
  if (!draft || !settings) return error ? <Notice tone="error" onRetry={load}>{error}</Notice> : null;
  const changed = JSON.stringify(draft) !== JSON.stringify(settings);
  const save = async () => {
    try {
      const saved = await withAdmin(() => loginApi.save(draft), { scope: "mac", action: "login_settings_set" });
      setSettings(saved);
      setDraft(saved);
      toast({ kind: "success", title: "Cluster login saved", body: "Nodes reconnect with it now." });
      load();
    } catch (e) {
      setError(String((e as { message?: string })?.message ?? e));
    }
  };
  const savePassphrase = async () => {
    if (!draft.identityFile) return;
    try {
      await withAdmin(() => loginApi.setPassphrase(draft.identityFile!, passphrase), { scope: "mac", action: "login_passphrase_set" });
      setPassphrase("");
      toast({ kind: "success", title: "Passphrase saved in the Keychain" });
    } catch (e) {
      setError(String((e as { message?: string })?.message ?? e));
    }
  };
  return (
    <>
      {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}
      <FormGroup
        title={
          <span className="login-title">
            How SparkBox logs in
            <HelpTip topic="start.login.order" label="About the login order" />
          </span>
        }
      >
        <FormRow label={<>SSH agent <HelpTip topic="start.login.ssh-agent" /></>}>
          <Segmented<AgentChoice>
            label="SSH agent"
            value={draft.agent}
            onChange={(agent) => setDraft({ ...draft, agent })}
            options={[
              { value: "system", label: "System", title: "The agent Terminal's ssh uses (SSH_AUTH_SOCK)" },
              { value: "onePassword", label: "1Password" },
              { value: "secretive", label: "Secretive" },
              { value: "off", label: "Off" },
            ]}
          />
        </FormRow>
        <FormRow label={<>Key file <HelpTip topic="start.login.key-file" /></>} htmlFor="login-key" hint="Tried after the agent's keys">
          <input id="login-key" className="mono" value={draft.identityFile ?? ""} placeholder="~/.ssh/id_ed25519" spellCheck={false} autoComplete="off"
            onChange={(e) => setDraft({ ...draft, identityFile: e.target.value || null })} />
        </FormRow>
        <FormRow label={<>Key passphrase <HelpTip topic="start.login.passphrase" /></>} htmlFor="login-passphrase" hint="Only for a key that has one; kept in the Keychain">
          <span className="keychain">
            <input id="login-passphrase" type="password" value={passphrase} disabled={!settings.identityFile} autoComplete="off"
              placeholder={settings.identityFile ? "Not needed for most keys" : "Save a key file first"}
              onChange={(e) => setPassphrase(e.target.value)} onKeyDown={(e) => e.key === "Enter" && passphrase && void savePassphrase()} />
            <button type="button" className="btn" disabled={!passphrase || !settings.identityFile} onClick={() => void savePassphrase()}>Save</button>
          </span>
        </FormRow>
        <FormRow label={<>~/.ssh/config <HelpTip topic="start.login.ssh-config" /></>}>
          <label className="check">
            <input type="checkbox" checked={draft.useSshConfig} onChange={(e) => setDraft({ ...draft, useSshConfig: e.target.checked })} />
            Use it for host names, ports, users, keys and jump hosts
          </label>
        </FormRow>
        <FormRow label={<>Password <HelpTip topic="start.login.password-fallback" /></>}>
          <label className="check">
            <input type="checkbox" checked={draft.passwordFallback} onChange={(e) => setDraft({ ...draft, passwordFallback: e.target.checked })} />
            Fall back to the cluster password when no key is accepted
          </label>
        </FormRow>
        {changed && (
          <div className="fg-row login-save">
            <span className="muted">Nodes reconnect when you save.</span>
            <button type="button" className="btn" onClick={() => setDraft(settings)}>Revert</button>
            <button type="button" className="btn primary" onClick={() => void save()}>
              <Lock size={12} aria-hidden /> Save
            </button>
          </div>
        )}
      </FormGroup>
      <FormGroup title={<span className="login-title">Each node <HelpTip topic="start.login.each-node" /></span>} footnote={loginFootnote(nodes ?? [], () => setSetup(true), setup)}>
        {(nodes ?? []).map((n) => (
          <div key={n.node} className="fg-row login-node">
            <div className="fg-label">
              <span className="strong">{n.name}</span>
              <span className="fg-hint">
                {n.method === null ? `Will try ${[n.agentRunning ? "the agent" : null, ...n.keys, n.password ? "the password" : null].filter(Boolean).join(", ") || "nothing: add a key or the password"}` : METHOD_WORD[n.method]}
                {n.jump ? ` · through ${n.jump}` : ""}
              </span>
            </div>
            <div className="fg-control login-method">
              {/* The shared password is the owner's choice: a plain lock, no warning. */}
              {n.method === "password" ? <Lock size={13} className="muted" aria-hidden /> : <KeyRound size={13} className="muted" aria-hidden />}
              {/* The name is already on the left: "shared cluster password", "key ~/.ssh/id_ed25519". */}
              <span className={n.method === "password" ? "muted" : "mono"}>{n.summary.replace(`${n.name} · `, "")}</span>
            </div>
          </div>
        ))}
        {nodes?.length === 0 && <div className="fg-row"><span className="muted">No nodes yet.</span></div>}
      </FormGroup>
      {setup && nodes && <KeySetupPanel nodes={nodes} onDone={() => loginApi.methods().then(setNodes, () => {})} onClose={() => setSetup(false)} />}
    </>
  );
}

export function LoginSection() {
  const inv = useInventoryDraft();
  const creds = useCredentialStatus([SSH_ACCOUNT]);
  const user = inv.draft?.nodes[0]?.user ?? "";
  const mixed = !!inv.draft && new Set(inv.draft.nodes.map((n) => n.user)).size > 1;

  return (
    <>
      {creds.error && <Notice tone="error" onRetry={creds.refresh}>{creds.error}</Notice>}
      {inv.error && <Notice tone="error" onRetry={inv.reload}>{inv.error}</Notice>}
      <FormGroup
        title={
          <span className="login-title">
            Account on every node
            <HelpTip topic="start.login.password" label="About the cluster password" />
          </span>
        }
      >
        <FormRow label="User" htmlFor="login-user" hint={mixed ? "Nodes currently use different users; saving sets this one on all" : undefined}>
          <input
            id="login-user"
            value={user}
            disabled={!inv.draft}
            autoComplete="off"
            spellCheck={false}
            aria-invalid={inv.draft && !user.trim() ? true : undefined}
            onChange={(e) => inv.edit((d) => ({ ...d, nodes: d.nodes.map((n) => ({ ...n, user: e.target.value })) }))}
          />
        </FormRow>
        <FormRow label="Password" htmlFor="login-password">
          <KeychainControl account={SSH_ACCOUNT} id="login-password" label="Cluster password" saved={creds.status[SSH_ACCOUNT]} onChange={creds.refresh} />
        </FormRow>
      </FormGroup>
      <InventorySaveBar />
      <LoginMethods />
    </>
  );
}
