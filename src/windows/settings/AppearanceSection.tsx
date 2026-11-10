import { Eclipse, Monitor, Moon, Sun } from "lucide-react";
import { FormGroup, FormRow, HelpTip, Meter, Segmented } from "../../components/ui";
import { setAppearance, TEXT_SCALES, useAppearance, useTheme, type ThemePref } from "../../lib/theme";

const THEMES: { value: ThemePref; label: string; icon: typeof Sun }[] = [
  { value: "system", label: "System", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
  { value: "black", label: "Black", icon: Eclipse },
];

/** The footnote under the theme control: what the chosen theme does beyond the window. */
function themeNote(theme: ThemePref, effective: string): string {
  if (theme === "system") return `Following macOS, currently ${effective}.`;
  if (theme === "black") return "Black is for an OLED screen or a dark room: true black, with brighter colors for the numbers. The title bar and menus use the dark look.";
  return "The window's title bar and menus switch too.";
}

/**
 * A framed sample of the UI that follows every change at once, before closing Settings. It's a
 * picture, not controls: nothing in it can be clicked or focused.
 */
function Preview() {
  return (
    <figure className="appearance-preview" aria-label="Preview of the theme and text size">
      <figcaption className="appearance-preview-caption">Preview</figcaption>
      <div className="appearance-preview-frame" aria-hidden inert>
        <div className="appearance-preview-bar">
          <span className="strong">cell1</span>
          <span className="appearance-preview-ok"><span className="ok-dot" />Healthy</span>
          <span className="spacer" />
          <span className="btn">Button</span>
          <span className="btn primary">Primary</span>
        </div>
        <div className="appearance-preview-body">
          <div className="appearance-preview-row">
            <span className="text-2">GPU</span>
            <Meter pct={73} label="73%" color="var(--m-gpu)" ariaLabel="Example GPU meter" />
          </div>
          <div className="appearance-preview-row">
            <span className="text-2">Memory</span>
            <Meter pct={88} label="105 / 119 GiB" color="var(--m-mem)" warn={85} crit={95} ariaLabel="Example memory meter" />
          </div>
          <p className="muted">Secondary text reads like this; links like <span className="link-text">this</span>.</p>
        </div>
      </div>
    </figure>
  );
}

export function AppearanceSection() {
  const a = useAppearance();
  const theme = useTheme();
  return (
    <>
      <Preview />
      <FormGroup
        title="Theme and text"
        footnote={themeNote(a.theme, theme.effective)}
      >
        <FormRow label={<>Theme <HelpTip topic="desk.appearance.theme" /></>}>
          <Segmented
            label="Theme"
            value={a.theme}
            onChange={(v) => setAppearance({ theme: v })}
            options={THEMES.map((t) => ({ value: t.value, label: t.label, icon: t.icon }))}
          />
        </FormRow>
        <FormRow label="Text size" hint="Every window, tables included">
          <Segmented
            label="Text size"
            value={String(a.textScale)}
            onChange={(v) => setAppearance({ textScale: Number(v) })}
            options={TEXT_SCALES.map((s) => ({ value: String(s.value), label: s.label }))}
          />
        </FormRow>
      </FormGroup>
      <FormGroup
        title="Accessibility"
        footnote="System follows macOS System Settings › Accessibility › Display."
      >
        <FormRow label={<>Reduce motion <HelpTip topic="desk.appearance.motion" /></>} hint="No pulsing, spinning or sliding">
          <Segmented
            label="Reduce motion"
            value={a.motion}
            onChange={(v) => setAppearance({ motion: v })}
            options={[{ value: "system", label: "System" }, { value: "reduce", label: "On" }]}
          />
        </FormRow>
        <FormRow label={<>Increase contrast <HelpTip topic="desk.appearance.contrast" /></>} hint="Darker lines, solid selection, thicker focus ring">
          <Segmented
            label="Increase contrast"
            value={a.contrast}
            onChange={(v) => setAppearance({ contrast: v })}
            options={[{ value: "system", label: "System" }, { value: "more", label: "On" }]}
          />
        </FormRow>
      </FormGroup>
    </>
  );
}
