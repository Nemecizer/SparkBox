import { useState, type KeyboardEvent } from "react";
import { FlaskConical, Server } from "lucide-react";
import { Icon } from "../../components/Icon";
import { FormGroup, FormRow, HelpTip, Notice } from "../../components/ui";
import { ADMIN_REFUSED, withAdmin } from "../../lib/admin";
import { api, SSH_ACCOUNT } from "../../lib/api";
import { useDataSource } from "../../lib/dataSource";
import type { DataSource } from "../../lib/types";
import { setPrefs, usePrefs } from "./GeneralSection";
import { KeychainControl, useCredentialStatus } from "./LoginSection";

const CHOICES: { value: DataSource; title: string; body: string; icon: typeof Server }[] = [
  {
    value: "demo",
    title: "Demo",
    body: "A simulated twin of your four Sparks: the same hostnames, hardware and software, serving a model by default. Nothing connects anywhere; actions change only the simulation.",
    icon: FlaskConical,
  },
  {
    value: "live",
    title: "Live",
    body: "Your Sparks over SSH (directly at home, through the VPN when away). Each runs a small read-only collector, ~/.sparkbox/collector.py.",
    icon: Server,
  },
];

export function DataSourceSection() {
  const { prefs, error } = usePrefs();
  const creds = useCredentialStatus([SSH_ACCOUNT]);
  const current = prefs?.dataSource ?? "demo";
  // What the app shows can differ from the saved choice for one run (SPARKBOX_DATA_SOURCE).
  const shown = useDataSource();
  const [failed, setFailed] = useState<string | null>(null);
  // Switching what SparkBox talks to is Mac-side state: it always asks for Touch ID or the password.
  const choose = async (v: DataSource) => {
    if (!prefs || v === current) return;
    try {
      setPrefs(await withAdmin(() => api.dataSourceSet(v), { scope: "mac", action: "data_source_set" }));
      setFailed(null);
    } catch (e) {
      if (e !== ADMIN_REFUSED) setFailed(`Couldn't switch to ${v === "live" ? "Live" : "Demo"}: ${(e as { message?: string })?.message ?? String(e)}`);
    }
  };
  const onKey = (e: KeyboardEvent) => {
    if (!["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(e.key)) return;
    e.preventDefault();
    const next = CHOICES[(CHOICES.findIndex((c) => c.value === current) + 1) % CHOICES.length].value;
    void choose(next);
    (e.currentTarget.querySelector(`[data-value="${next}"]`) as HTMLElement | null)?.focus();
  };

  return (
    <>
      {(error || failed) && <Notice tone="error" onDismiss={failed ? () => setFailed(null) : undefined}>{failed ?? error}</Notice>}
      <div className="choice-cards" role="radiogroup" aria-label="Data source" onKeyDown={onKey}>
        {CHOICES.map((c) => (
          <button
            key={c.value}
            type="button"
            role="radio"
            data-value={c.value}
            aria-checked={current === c.value}
            tabIndex={current === c.value ? 0 : -1}
            disabled={!prefs}
            className={`choice-card${current === c.value ? " on" : ""}`}
            onClick={() => void choose(c.value)}
          >
            <span className="choice-card-head">
              <Icon icon={c.icon} size={16} />
              <span className="choice-card-title">{c.title}</span>
              <span className="choice-card-radio" aria-hidden />
            </span>
            <span className="choice-card-body">{c.body}</span>
          </button>
        ))}
      </div>
      {prefs && shown !== current && (
        <Notice tone="info">
          This run shows {shown === "live" ? "Live" : "Demo"} data (chosen when SparkBox was started). The choice above is saved for the next launch.
        </Notice>
      )}
      <p className="settings-saved muted">Demo and Live keep their own tasks, chats and terminal sessions: switching shows the other side's.</p>
      {current === "live" && creds.status[SSH_ACCOUNT] === false && (
        <FormGroup title="Needed for Live" footnote="SparkBox can't log in to the nodes until the cluster password is in the Keychain.">
          <FormRow label={<>Cluster password <HelpTip topic="save-the-cluster-password" /></>} htmlFor="ds-password">
            <KeychainControl account={SSH_ACCOUNT} id="ds-password" label="Cluster password" saved={false} onChange={creds.refresh} />
          </FormRow>
        </FormGroup>
      )}
      {current === "live" && creds.status[SSH_ACCOUNT] && (
        <p className="settings-saved muted">The cluster password is saved. Connection details are in Nodes and Cluster login.</p>
      )}
    </>
  );
}
