import { Component, type ErrorInfo, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Copy, RotateCcw, TriangleAlert } from "lucide-react";
import { HelpTip } from "./ui";
import "./shell.css";

interface Props {
  id: string;
  title: string;
  children: ReactNode;
}

interface State {
  error: Error | null;
  componentStack: string;
  /** Bumped by "Reload window" to remount the body from scratch. */
  generation: number;
  copied: boolean;
}

/** Keeps a crash inside one window: the rest of the app keeps working, and the window can reload itself. */
export class WindowErrorBoundary extends Component<Props, State> {
  state: State = { error: null, componentStack: "", generation: 0, copied: false };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    this.setState({ componentStack: info.componentStack ?? "" });
    // The core writes it to the app log, redacted; a failure to log must not throw here.
    try {
      invoke("log_ui_error", {
        window: this.props.id,
        message: String(error?.message ?? error),
        stack: `${error?.stack ?? ""}\n${info.componentStack ?? ""}`.slice(0, 8000),
      }).catch(() => {});
    } catch {
      // Not in the app.
    }
  }

  details(): string {
    const e = this.state.error;
    return [`Window: ${this.props.title} (${this.props.id})`, `Error: ${e?.message ?? e}`, e?.stack ?? "", this.state.componentStack].join("\n");
  }

  copy = async () => {
    try {
      await navigator.clipboard.writeText(this.details());
      this.setState({ copied: true });
      window.setTimeout(() => this.setState({ copied: false }), 1500);
    } catch {
      // Clipboard refused; the details are still on screen.
    }
  };

  reload = () => this.setState((s) => ({ error: null, componentStack: "", generation: s.generation + 1 }));

  render() {
    const { error, generation, copied } = this.state;
    if (!error) return <ErrorKey key={generation}>{this.props.children}</ErrorKey>;
    return (
      <div className="win-error" role="alert">
        <TriangleAlert size={28} className="win-error-icon" aria-hidden />
        <h3>This window hit an error <HelpTip topic="troubleshooting" /></h3>
        <p className="win-error-msg mono">{String(error.message || error)}</p>
        <p className="muted">The rest of SparkBox is fine. Reloading starts this window over; nothing on your nodes changed.</p>
        <div className="win-error-actions">
          <button className="btn primary" onClick={this.reload} data-autofocus><RotateCcw size={13} aria-hidden /> Reload window</button>
          <button className="btn" onClick={this.copy}><Copy size={13} aria-hidden /> {copied ? "Copied" : "Copy details"}</button>
        </div>
      </div>
    );
  }
}

/** A keyed fragment, so a reload remounts the body. */
function ErrorKey({ children }: { children: ReactNode }) {
  return <>{children}</>;
}
