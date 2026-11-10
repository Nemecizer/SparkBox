// The switch's own state at the head of its window, in the one severity mapping (§2.3):
// a quiet dot when it answers and is healthy, the loud offline glyph when it doesn't.
import { glyphOfStatus, glyphWord, StateGlyph } from "../../components/ui";
import { switchDisplayStatus, useHealth, DISPLAY_LABEL } from "../../lib/health";
import type { SwitchSnapshot } from "../../lib/types";

export function SwitchState({ sw }: { sw: SwitchSnapshot }) {
  const health = useHealth();
  const st = switchDisplayStatus(sw, health);
  const g = glyphOfStatus(st);
  return (
    <span className={`status switch-state tone-${g}`}>
      <StateGlyph state={g} label={DISPLAY_LABEL[st]} />
      <span aria-hidden>{g === "ok" ? "Online" : g === "offline" ? glyphWord("offline") : DISPLAY_LABEL[st]}</span>
    </span>
  );
}
