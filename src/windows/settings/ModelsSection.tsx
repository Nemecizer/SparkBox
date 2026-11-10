// Settings › Models: where models live on the nodes, the vLLM and SGLang images,
// the Hugging Face token, and which model the AI feasibility check uses. The vLLM images the
// cluster's containers already run are offered as suggestions; picking one only fills the field.
// The load recipes are listed read-only, with what each has been verified on.
import { useEffect, useState } from "react";
import { FormGroup, FormRow, HelpTip, InfoTip, Notice } from "../../components/ui";
import { ADMIN_REFUSED, withAdmin } from "../../lib/admin";
import { api, HF_ACCOUNT } from "../../lib/api";
import { modelsApi, verifiedWords, type ProviderView, type RecipeView } from "../../lib/modelsApi";
import { useCluster } from "../../lib/store";
import { toast } from "../../lib/toasts";
import type { ClusterSnapshot, ModelSettings } from "../../lib/types";
import { useWM } from "../../wm/WindowManager";
import { KeychainControl, useCredentialStatus } from "./LoginSection";
import "../ai.css";

/** vLLM and SGLang images the cluster's containers run, with the nodes that have them, most nodes first. */
export function imagesSeen(snap: ClusterSnapshot | null | undefined, engine: "vllm" | "sglang"): { image: string; nodes: string[] }[] {
  const by = new Map<string, string[]>();
  for (const n of snap?.nodes ?? []) {
    for (const c of n.containers) {
      if (!c.image.toLowerCase().includes(engine)) continue;
      const nodes = by.get(c.image) ?? [];
      if (!nodes.includes(n.name)) nodes.push(n.name);
      by.set(c.image, nodes);
    }
  }
  return [...by.entries()].map(([image, nodes]) => ({ image, nodes })).sort((a, b) => b.nodes.length - a.nodes.length || a.image.localeCompare(b.image));
}

function Seen({ list, current, onPick }: { list: { image: string; nodes: string[] }[]; current: string; onPick: (image: string) => void }) {
  const others = list.filter((x) => x.image !== current);
  if (!others.length) return null;
  return (
    <div className="ai-seen-images">
      <span className="muted">On the cluster:</span>
      {others.map((x) => (
        <button key={x.image} type="button" className="link mono" onClick={() => onPick(x.image)} title={`Use ${x.image} (fills the field; Save keeps it)`}>
          {x.image} <span className="muted">· {x.nodes.length === 1 ? x.nodes[0] : `${x.nodes.length} nodes`}</span>
        </button>
      ))}
    </div>
  );
}

/** The built-in load recipes. Nothing here changes one: a load picks its own on the Load page. */
function Recipes() {
  const [recipes, setRecipes] = useState<RecipeView[] | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    modelsApi.recipesList().then(setRecipes, () => setFailed(true));
  }, []);
  return (
    <FormGroup
      title={
        <>
          Recipes{" "}
          <HelpTip topic="use-a-recipe-for-a-load" label="About recipes" />
        </>
      }
    >
      {failed && <div className="muted">Couldn't read the recipes.</div>}
      {recipes?.map((r) => (
        <FormRow key={r.id} label={r.title} hint={r.note ?? undefined}>
          <span className="muted">{verifiedWords(r)}</span>
        </FormRow>
      ))}
    </FormGroup>
  );
}

export function ModelsSection() {
  const wm = useWM();
  const snap = useCluster();
  const [settings, setSettings] = useState<ModelSettings | null>(null);
  const [saved, setSaved] = useState<ModelSettings | null>(null);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const hf = useCredentialStatus([HF_ACCOUNT]);

  useEffect(() => {
    api.prefsGet().then((p) => {
      setSettings(p.models);
      setSaved(p.models);
    }, (e) => setError(String(e)));
    modelsApi.providersList().then((ps) => setProviders(ps.filter((p) => p.kind === "anthropic" || p.kind === "openai")), () => {});
  }, []);

  if (!settings) return <div className="pad muted">{error ?? "Loading…"}</div>;
  const dirty = JSON.stringify(settings) !== JSON.stringify(saved);
  const edit = (patch: Partial<ModelSettings>) => setSettings({ ...settings, ...patch });
  const provider = providers.find((p) => p.id === settings.advisorProvider);

  const save = async () => {
    try {
      // Model settings are Mac-side state: saving asks for Touch ID or the password.
      await withAdmin(() => api.prefsPatch({ models: settings }), { scope: "mac", action: "prefs_set" });
      setSaved(settings);
      toast({ kind: "success", title: "Model settings saved" });
    } catch (e) {
      if (e === ADMIN_REFUSED) return;
      toast({ kind: "error", title: "Couldn't save the model settings", body: String(e) });
    }
  };

  return (
    <div className="col gap ai-settings">
      <FormGroup
        title={
          <>
            Where models live{" "}
            <InfoTip label="About the model folder" topic="models.settings.folder">
              The same folder on every node; <span className="mono">~</span> is the cluster login's home. Downloads go to <span className="mono">{settings.modelRoot.replace(/\/$/, "")}/&lt;org&gt;/&lt;name&gt;</span>. Find models also looks in the Hugging Face cache (<span className="mono">~/.cache/huggingface/hub</span>).
            </InfoTip>
          </>
        }
      >
        <FormRow label="Model folder" htmlFor="models-root">
          <input id="models-root" className="wide mono" value={settings.modelRoot} onChange={(e) => edit({ modelRoot: e.target.value })} />
        </FormRow>
      </FormGroup>
      <FormGroup
        title={
          <>
            Engines{" "}
            <HelpTip topic="models.settings.images" label="About the engine images" />
          </>
        }
      >
        <FormRow label="vLLM image" htmlFor="models-image">
          <input id="models-image" className="wide mono" value={settings.image} onChange={(e) => edit({ image: e.target.value })} />
          <Seen list={imagesSeen(snap, "vllm")} current={settings.image} onPick={(image) => edit({ image })} />
        </FormRow>
        <FormRow label="SGLang image" htmlFor="models-sglang-image">
          <input id="models-sglang-image" className="wide mono" value={settings.sglangImage ?? "lmsysorg/sglang:spark"} onChange={(e) => edit({ sglangImage: e.target.value })} />
          <Seen list={imagesSeen(snap, "sglang")} current={settings.sglangImage ?? "lmsysorg/sglang:spark"} onPick={(sglangImage) => edit({ sglangImage })} />
        </FormRow>
      </FormGroup>
      <Recipes />
      <FormGroup
        title={
          <>
            Hugging Face{" "}
            <HelpTip topic="get-a-hugging-face-token" label="About the Hugging Face token" />
          </>
        }
      >
        <FormRow label="Access token" htmlFor="models-hf-token">
          <KeychainControl account={HF_ACCOUNT} id="models-hf-token" label="Hugging Face access token" saved={hf.status[HF_ACCOUNT]} onChange={hf.refresh} />
        </FormRow>
      </FormGroup>
      <FormGroup
        title={
          <>
            AI feasibility check{" "}
            <HelpTip topic="models.settings.advisor" label="About the AI feasibility check" />
          </>
        }
      >
        <FormRow label="Provider" htmlFor="models-advisor">
          <select id="models-advisor" value={settings.advisorProvider} onChange={(e) => {
            const p = providers.find((x) => x.id === e.target.value);
            edit({ advisorProvider: e.target.value, advisorModel: p?.effectiveDefault ?? p?.visible[0] ?? settings.advisorModel });
          }}>
            {!provider && <option value={settings.advisorProvider}>{settings.advisorProvider} (not available: pick Anthropic or OpenAI)</option>}
            {providers.map((p) => <option key={p.id} value={p.id}>{p.name}{p.hasKey ? "" : " (no key)"}</option>)}
          </select>
        </FormRow>
        <FormRow label="Model" htmlFor="models-advisor-model">
          <select id="models-advisor-model" value={settings.advisorModel} onChange={(e) => edit({ advisorModel: e.target.value })}>
            {!provider?.visible.includes(settings.advisorModel) && <option value={settings.advisorModel}>{settings.advisorModel}</option>}
            {provider?.visible.map((m) => <option key={m} value={m}>{m}</option>)}
          </select>
        </FormRow>
        {provider && !provider.hasKey && (
          <Notice tone="warn" action={{ label: "Open Providers", onClick: () => wm.open("providers") }}>Add your {provider.name} API key first.</Notice>
        )}
      </FormGroup>
      <div className="ai-settings-save">
        <button className="btn primary" disabled={!dirty} onClick={save}>Save</button>
        {dirty && <button className="btn" onClick={() => setSettings(saved)}>Revert</button>}
      </div>
    </div>
  );
}
