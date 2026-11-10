import { useEffect } from "react";
import { Check, Sparkles } from "lucide-react";
import { Icon } from "../components/Icon";
import { HelpTip } from "../components/ui";
import { useWM, winId, type Params } from "../wm/WindowManager";
import { PLANNED } from "./plannedWindows";

export function Planned({ params }: { params: Params }) {
  const wm = useWM();
  const p = PLANNED[params.key];

  // Entries that a built window now covers hand over to it (e.g. vLLM → Models › Load).
  useEffect(() => {
    if (!p?.redirect) return;
    wm.open(p.redirect.kind, p.redirect.params);
    wm.close(winId("planned", params));
    // Once, when this entry opens.
  }, [p?.redirect]);

  if (!p) return <div className="pad muted">This window isn't available in this version of SparkBox.</div>;
  if (p.redirect) return <div className="pad muted">{p.summary}</div>;
  return (
    <div className="planned">
      <div className="planned-head">
        <span className="planned-icon"><Icon icon={Sparkles} size={20} /></span>
        <div>
          <div className="planned-title">{p.title}</div>
          <span className="badge info">Planned</span>
        </div>
      </div>
      <p className="planned-summary">{p.summary}</p>
      <h3 className="section-title">What it will do <HelpTip topic="whats-coming" /></h3>
      <ul className="planned-points">
        {p.points.map((pt) => (
          <li key={pt}><Icon icon={Check} size={12} />{pt}</li>
        ))}
      </ul>
    </div>
  );
}
