import { useEffect, useState, type KeyboardEvent } from "react";
import { RotateCcw } from "lucide-react";
import { Badge, FormGroup, HelpTip, Notice } from "../../components/ui";
import { registerCommands } from "../../lib/commands";
import { netApi, type ScenarioInfo } from "../../lib/netApi";
import { getCluster, useClusterSelector } from "../../lib/store";
import { toast } from "../../lib/toasts";
import "../net.css";

/** One palette command per scenario ("Demo scenario: Fabric link flapping"), registered once the list arrives. */
let registered = false;
function registerScenarioCommands() {
  if (registered || typeof window === "undefined") return;
  registered = true;
  netApi.demoScenariosList().then(
    (list) => {
      const onlyDemo = () => (getCluster()?.demo === false ? "Scenarios play only with Demo data" : true);
      registerCommands(
        list.map((s) => ({
          id: `demo:scenario:${s.id}`,
          title: s.id === "default" ? "Demo scenario: back to everyday serving" : `Demo scenario: ${s.title}`,
          group: "Demo",
          subtitle: s.description,
          keywords: ["scenario", "demo", "simulate", ...s.affects],
          when: onlyDemo,
          run: async () => {
            await netApi.demoScenarioSet(s.id === "default" ? null : s.id);
            toast({ kind: "info", title: s.id === "default" ? "Back to everyday serving" : `Playing “${s.title}”`, body: s.id === "default" ? undefined : s.description });
          },
        })),
      );
    },
    () => {
      registered = false;
    },
  );
}
registerScenarioCommands();

/** "0:42" until the next phase. */
function clock(s: number): string {
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

function every(periodS: number): string | null {
  if (!periodS) return null;
  return periodS < 120 ? `Every ${periodS} s` : `Every ${Math.round(periodS / 60)} min`;
}

const phaseWord = (p: string) => (p === "fault" ? "Fault" : p === "recovering" ? "Recovering" : "Healthy");

/** Settings › Demo scenarios: scripted faults for the simulator, one at a time. */
export function ScenariosSection() {
  const demo = useClusterSelector((s) => s?.demo ?? true);
  const active = useClusterSelector((s) => s?.scenario ?? null) ?? "default";
  const phase = useClusterSelector((s) => s?.scenarioPhase ?? null, (a, b) => a?.label === b?.label && a?.nextChangeS === b?.nextChangeS);
  const [list, setList] = useState<ScenarioInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    netApi.demoScenariosList().then(setList, (e) => setError(String(e)));
  }, []);

  const choose = async (id: string) => {
    if (id === active || busy) return;
    setBusy(true);
    try {
      await netApi.demoScenarioSet(id === "default" ? null : id);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!list || !["ArrowUp", "ArrowDown"].includes(e.key)) return;
    e.preventDefault();
    const i = list.findIndex((s) => s.id === active);
    const next = list[(i + (e.key === "ArrowDown" ? 1 : list.length - 1)) % list.length];
    choose(next.id);
    (e.currentTarget.querySelector(`[data-id="${next.id}"]`) as HTMLElement | null)?.focus();
  };

  if (!demo) {
    return (
      <Notice tone="info">
        Scenarios play only in the simulator. Switch to Demo in Data source to try them; your real cluster is never touched.
      </Notice>
    );
  }

  return (
    <>
      {error && <Notice tone="error">{error}</Notice>}
      <FormGroup
        title={
          <span className="scenarios-title">
            Scenario
            <HelpTip topic="desk.scenarios.about" label="About scenarios" />
          </span>
        }
      >
        <div className="scenarios" role="radiogroup" aria-label="Demo scenario" onKeyDown={onKey}>
          {!list && <p className="muted scenarios-loading">Loading scenarios…</p>}
          {list?.map((s) => {
            const on = s.id === active;
            const repeat = every(s.periodS);
            return (
              <button
                key={s.id}
                type="button"
                role="radio"
                data-id={s.id}
                aria-checked={on}
                tabIndex={on ? 0 : -1}
                disabled={busy}
                className={`scenario${on ? " on" : ""}`}
                onClick={() => choose(s.id)}
              >
                <span className="scenario-radio" aria-hidden />
                <span className="scenario-main">
                  <span className="scenario-head">
                    <span className="scenario-title">{s.title}</span>
                    {on && <Badge tone={phase?.phase === "fault" ? "warn" : "neutral"}>{phase && s.id !== "as-found" ? phaseWord(phase.phase) : "Active"}</Badge>}
                    {on && phase?.nextChangeS != null && <span className="scenario-meta num">next in {clock(phase.nextChangeS)}</span>}
                    {repeat && <span className="scenario-meta">{repeat}</span>}
                  </span>
                  <span className="scenario-body">{s.description}</span>
                  {s.affects.length > 0 && (
                    <span className="scenario-affects">
                      {s.affects.map((a) => <span key={a} className="chip">{a}</span>)}
                    </span>
                  )}
                </span>
              </button>
            );
          })}
        </div>
      </FormGroup>
      <div className="scenarios-footer">
        <button className="btn" disabled={busy || active === "default"} onClick={() => choose("default")}>
          <RotateCcw size={12} /> Back to everyday serving
        </button>
      </div>
    </>
  );
}
