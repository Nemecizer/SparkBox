import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { BookOpen, Compass, Keyboard, PlugZap } from "lucide-react";
import { openHelp } from "../help/nav";
import { useCluster } from "../lib/store";
import { closePanel, openPanel, usePanel } from "../lib/session";
import { useWMActions } from "../wm/WindowManager";
import { BrandMark } from "./BrandMark";
import "./shell.css";

const WELCOMED_KEY = "sparkbox.welcomed";

export function hasBeenWelcomed(): boolean {
  try {
    return !!localStorage.getItem(WELCOMED_KEY);
  } catch {
    return true;
  }
}

function markWelcomed() {
  try {
    localStorage.setItem(WELCOMED_KEY, new Date().toISOString());
  } catch {
    // Shows again next launch; harmless.
  }
}

/** The first-run sheet (Help › Show Welcome brings it back). */
export function Welcome({ isKnown }: { isKnown: (kind: string) => boolean }) {
  const panel = usePanel();
  useEffect(() => {
    if (!hasBeenWelcomed()) openPanel("welcome");
  }, []);
  if (panel !== "welcome") return null;
  return createPortal(<Sheet isKnown={isKnown} />, document.body);
}

function Sheet({ isKnown }: { isKnown: (kind: string) => boolean }) {
  const wm = useWMActions();
  const cluster = useCluster();
  const primary = useRef<HTMLButtonElement>(null);
  const done = () => {
    markWelcomed();
    closePanel();
  };

  useEffect(() => {
    primary.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        done();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const explore = () => {
    done();
    // Updates apply in order, so each snap lands on the window just opened.
    wm.open("dashboard");
    wm.open("topology");
    wm.snap("dashboard", "left");
    wm.snap("topology", "right");
  };
  const connect = () => {
    done();
    if (isKnown("connect")) wm.open("connect");
    else wm.open("settings", { section: "nodes" });
  };
  const live = cluster && !cluster.demo;

  return (
    <div className="sb-overlay welcome-scrim" data-shell-overlay>
      <div className="welcome" role="dialog" aria-modal="true" aria-labelledby="welcome-title" aria-describedby="welcome-body">
        <BrandMark size={40} className="welcome-mark" />
        <h2 id="welcome-title">Welcome to SparkBox</h2>
        <p id="welcome-body" className="welcome-lede">
          {live
            ? "You're connected to your Sparks. Everything you see is live."
            : "You're looking at a simulated cluster (Demo data): four Sparks, a 200G fabric switch and a gateway, all safe to click."}
        </p>
        <div className="welcome-actions">
          <button ref={primary} className="welcome-action primary" onClick={explore}>
            <Compass size={20} aria-hidden />
            <span><b>{live ? "Look around" : "Explore the demo"}</b><span>Opens the Dashboard and Topology side by side.</span></span>
          </button>
          <button className="welcome-action" onClick={connect}>
            <PlugZap size={20} aria-hidden />
            <span><b>Connect my Sparks</b><span>{live ? "Add or change nodes and their sign-in." : "Add your nodes and sign-in, then switch to Live."}</span></span>
          </button>
          <button className="welcome-action" onClick={() => {
            done();
            openHelp("getting-started");
          }}>
            <BookOpen size={20} aria-hidden />
            <span><b>Getting Started</b><span>The shortest path from opening SparkBox to running your first model.</span></span>
          </button>
          <button className="welcome-action" onClick={() => {
            markWelcomed();
            openPanel("shortcuts");
          }}>
            <Keyboard size={20} aria-hidden />
            <span><b>Keyboard shortcuts</b><span>⌘K finds anything; ⌘` moves between windows.</span></span>
          </button>
        </div>
        <button className="link welcome-skip" onClick={done}>Not now</button>
      </div>
    </div>
  );
}
