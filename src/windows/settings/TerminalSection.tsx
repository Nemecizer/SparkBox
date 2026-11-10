import { useEffect, useMemo, useState } from "react";
import { FormGroup, FormRow, HelpTip, InfoTip, Notice, Segmented, Switch } from "../../components/ui";
import { ADMIN_REFUSED, withAdmin } from "../../lib/admin";
import { useDataSource } from "../../lib/dataSource";
import { inTauri } from "../../lib/api";
import { opsApi, type SecuritySettings } from "../../lib/opsApi";
import { useTheme } from "../../lib/theme";
import {
  DEFAULT_PREFS, fontStack, installedMonoFonts, setTermPrefs, THEME_LABEL, useTermPrefs, type CursorStyle, type OptionKey, type TermThemeId,
} from "../terminal/prefs";
import { PREVIEW_ORDER, terminalTheme } from "../terminal/theme";
import "../terminal/terminal.css";

const SCROLLBACK = [1000, 5000, 10_000, 50_000, 100_000];

/** A few lines of a real-looking session in the chosen theme and font. */
function Preview() {
  const p = useTermPrefs();
  useTheme(); // follow app appearance changes
  const t = terminalTheme(p.theme);
  const c = (k: keyof typeof t) => String(t[k] ?? "");
  return (
    <div
      className="sb-term-preview"
      style={{ background: c("background"), color: c("foreground"), fontFamily: fontStack(p.font), fontSize: p.fontSize, lineHeight: p.lineHeight }}
      aria-label="Terminal preview"
    >
      <div><span style={{ color: c("green") }}>sparkuser@cell1</span>:<span style={{ color: c("blue") }}>~/models</span>$ ls -la</div>
      <div>drwxr-xr-x  4 sparkuser sparkuser 4096 Oct 28 09:14 <span style={{ color: c("blue") }}>meta-llama</span></div>
      <div>lrwxrwxrwx  1 sparkuser sparkuser   32 Oct 28 09:14 <span style={{ color: c("cyan") }}>latest</span> -&gt; meta-llama/Llama-3.3-70B-Instruct</div>
      <div><span style={{ color: c("yellow") }}>WARN</span> kv cache 91% · <span style={{ color: c("red") }}>ERROR</span> NCCL timeout · <span style={{ color: c("magenta") }}>rdma</span> 23.1 GB/s</div>
      <div>
        <span style={{ color: c("green") }}>sparkuser@cell1</span>:<span style={{ color: c("blue") }}>~/models</span>${" "}
        <span className={`sb-term-preview-cursor ${p.cursorStyle}${p.cursorBlink ? " blink" : ""}`} style={{ ["--cur" as string]: c("cursor") }} />
      </div>
      <div className="sb-term-preview-swatches" aria-hidden>
        {PREVIEW_ORDER.map((k) => <span key={k} style={{ background: c(k) }} title={k} />)}
      </div>
    </div>
  );
}

type ShellIdle = NonNullable<SecuritySettings["shellIdleMinutes"]>;

/**
 * Lock idle shells after: the one terminal setting the backend enforces (an open shell that
 * nobody types in stops taking keystrokes). Loosening it is a security change: Mac scope.
 */
function ShellLock() {
  const [settings, setSettings] = useState<SecuritySettings | null>(inTauri ? null : { idleMinutes: 15, retentionDays: 90, shellIdleMinutes: 60 });
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (inTauri) opsApi.securityGet().then(setSettings, (e) => setError(String((e as { message?: string })?.message ?? e)));
  }, []);
  const value = settings?.shellIdleMinutes ?? 60;
  const live = useDataSource() === "live";
  const save = async (minutes: ShellIdle) => {
    if (!settings) return;
    const before = settings;
    const next = { ...settings, shellIdleMinutes: minutes };
    setSettings(next);
    if (!inTauri) return;
    try {
      setSettings(await withAdmin(() => opsApi.securitySet(next), { scope: "mac", action: "security_set" }));
      setError(null);
    } catch (e) {
      setSettings(before);
      if (e !== ADMIN_REFUSED) setError(String((e as { message?: string })?.message ?? e));
    }
  };
  return (
    <FormGroup
      title={<span className="sb-term-group-title">Shells<HelpTip topic="desk.terminal.idle" label="About idle shells" /></span>}
    >
      {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}
      <FormRow label="Lock idle shells after" hint={value === 0 ? (live ? "Demo only; Live shells lock after 8 h" : "Shells lock only when you lock") : "With nothing typed in them"}>
        <Segmented<string>
          label="Lock idle shells after"
          value={String(value)}
          onChange={(v) => void save(Number(v) as ShellIdle)}
          options={[
            { value: "15", label: "15 min" },
            { value: "60", label: "60 min" },
            { value: "240", label: "4 h" },
            // "Never" applies to Demo shells only: one on the real cluster locks after 8 hours.
            { value: "0", label: live ? "8 h" : "Never" },
          ]}
        />
      </FormRow>
    </FormGroup>
  );
}

/** Settings › Terminal: idle shells, look, cursor, scrollback, copy on select and the Option key. */
export function TerminalSection() {
  const p = useTermPrefs();
  const fonts = useMemo(() => {
    const list = installedMonoFonts();
    return list.includes(p.font) ? list : [...list, p.font];
  }, [p.font]);
  return (
    <>
      <ShellLock />
      <Preview />
      <FormGroup title={<span className="sb-term-group-title">Look<HelpTip topic="desk.terminal.look" label="About these settings" /></span>}>
        <FormRow label="Theme" htmlFor="term-theme" hint="SparkBox Dark and Light use the app's colors">
          <select id="term-theme" value={p.theme} onChange={(e) => setTermPrefs({ theme: e.target.value as TermThemeId })}>
            {(Object.keys(THEME_LABEL) as TermThemeId[]).map((id) => <option key={id} value={id}>{THEME_LABEL[id]}</option>)}
          </select>
        </FormRow>
        <FormRow label="Font" htmlFor="term-font" hint="Monospace fonts installed on this Mac">
          <select id="term-font" value={p.font} onChange={(e) => setTermPrefs({ font: e.target.value })}>
            {fonts.map((f) => <option key={f} value={f}>{f}</option>)}
          </select>
        </FormRow>
        <FormRow label="Size" htmlFor="term-size">
          <span className="sb-term-stepper">
            <button type="button" className="btn small" aria-label="Smaller" onClick={() => setTermPrefs({ fontSize: p.fontSize - 1 })} disabled={p.fontSize <= 9}>−</button>
            <input id="term-size" className="narrow num" type="number" min={9} max={24} value={p.fontSize} onChange={(e) => setTermPrefs({ fontSize: Number(e.target.value) })} />
            <span className="muted">pt</span>
            <button type="button" className="btn small" aria-label="Larger" onClick={() => setTermPrefs({ fontSize: p.fontSize + 1 })} disabled={p.fontSize >= 24}>+</button>
          </span>
        </FormRow>
        <FormRow label="Line height" htmlFor="term-lh">
          <select id="term-lh" value={String(p.lineHeight)} onChange={(e) => setTermPrefs({ lineHeight: Number(e.target.value) })}>
            {[1, 1.1, 1.15, 1.2, 1.3, 1.4].map((v) => <option key={v} value={String(v)}>{v.toFixed(2)}</option>)}
          </select>
        </FormRow>
      </FormGroup>
      <FormGroup title="Cursor and scrollback">
        <FormRow label="Cursor">
          <Segmented<CursorStyle>
            label="Cursor style"
            value={p.cursorStyle}
            onChange={(v) => setTermPrefs({ cursorStyle: v })}
            options={[{ value: "block", label: "Block" }, { value: "bar", label: "Bar" }, { value: "underline", label: "Underline" }]}
          />
        </FormRow>
        <FormRow label="Blinking cursor">
          <Switch checked={p.cursorBlink} onChange={(v) => setTermPrefs({ cursorBlink: v })} label="Blinking cursor" />
        </FormRow>
        <FormRow label={<>Scrollback <HelpTip topic="desk.terminal.scrollback" /></>} htmlFor="term-sb" hint="Lines each tab keeps; ⌘F searches them">
          <select id="term-sb" value={String(p.scrollback)} onChange={(e) => setTermPrefs({ scrollback: Number(e.target.value) })}>
            {[...new Set([...SCROLLBACK, p.scrollback])].sort((a, b) => a - b).map((v) => <option key={v} value={String(v)}>{v.toLocaleString()} lines</option>)}
          </select>
        </FormRow>
        <FormRow label={<>Copy on select <HelpTip topic="desk.terminal.copy-on-select" /></>} hint="Selecting text copies it, as in X11 terminals">
          <Switch checked={p.copyOnSelect} onChange={(v) => setTermPrefs({ copyOnSelect: v })} label="Copy on select" />
        </FormRow>
      </FormGroup>
      <FormGroup
        title={
          <span className="sb-term-group-title">
            Keyboard
            <InfoTip label="About the Option key" topic="desk.terminal.option">
              Meta sends Esc plus the key, for shell shortcuts like ⌥B and ⌥F (word left and right) or ⌥. (last argument). Keep one Option key Normal if your keyboard layout types @, |, {"{ }"} or [ ] with Option.
            </InfoTip>
          </span>
        }
      >
        <FormRow label="Option key as Meta">
          <Segmented<OptionKey>
            label="Option key as Meta"
            value={p.optionKey}
            onChange={(v) => setTermPrefs({ optionKey: v })}
            options={[
              { value: "normal", label: "Normal" },
              { value: "left", label: "Left ⌥" },
              { value: "right", label: "Right ⌥" },
              { value: "both", label: "Both" },
            ]}
          />
        </FormRow>
      </FormGroup>
      <p className="form-footnote">
        <button type="button" className="btn small" onClick={() => setTermPrefs(DEFAULT_PREFS)}>Restore defaults</button>
      </p>
    </>
  );
}
