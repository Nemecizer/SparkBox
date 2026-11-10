// What listens on a Spark's ports, and who most likely owns one. `ss` without root shows no
// owner for another user's socket (every port but the cluster user's own), so the owner is
// worked out from what's known: a pid SparkBox could see (confirmed), else the process the
// port belongs to running on that node ("nginx (likely)"), else its systemd unit
// ("postfix.service (likely)"). A guess always says "likely" and shows its evidence.
import type { Listener, ListenerScope, NodeSnapshot, Process } from "./types";

export interface KnownPort {
  /** "SMTP (Postfix)" */
  label: string;
  /** Process names that serve it (as the collector names them, full names). */
  processes: string[];
  /** Command lines that serve it, for servers that run as python3. */
  cmd?: RegExp;
  units: string[];
}

/** The ports a DGX Spark listens on, and what usually holds them. */
export const KNOWN_PORTS: Record<number, KnownPort> = {
  22: { label: "SSH", processes: ["sshd"], units: ["ssh.service"] },
  25: { label: "SMTP (Postfix)", processes: ["master"], units: ["postfix@-.service", "postfix.service"] },
  53: { label: "DNS (systemd-resolved)", processes: ["systemd-resolved"], units: ["systemd-resolved.service"] },
  631: { label: "CUPS (printing)", processes: ["cupsd"], units: ["cups.service"] },
  3350: { label: "xrdp session manager", processes: ["xrdp-sesman"], units: ["xrdp-sesman.service"] },
  3389: { label: "Remote desktop (xrdp)", processes: ["xrdp"], units: ["xrdp.service"] },
  6379: { label: "Ray GCS", processes: ["gcs_server", "redis-server"], cmd: /gcs_server|ray start/, units: [] },
  8000: { label: "vLLM (OpenAI-style API)", processes: ["vllm", "trtllm-serve"], cmd: /\bvllm\b.*\bserve\b|-m vllm|trtllm-serve/, units: [] },
  8080: { label: "HTTP (nginx, llama-server…)", processes: ["nginx", "llama-server"], units: ["nginx.service"] },
  8265: { label: "Ray dashboard", processes: [], cmd: /ray\/dashboard|dashboard\.py/, units: [] },
  11000: { label: "DGX Dashboard", processes: ["dashboard-service"], units: ["dgx-dashboard.service"] },
  11434: { label: "Ollama", processes: ["ollama"], units: ["ollama.service"] },
  29500: { label: "torch distributed", processes: ["torchrun"], cmd: /torchrun|torch\.distributed/, units: [] },
  29501: { label: "vLLM multi-node rendezvous", processes: ["vllm", "torchrun"], cmd: /\bvllm\b.*\bserve\b|-m vllm|torchrun|torch\.distributed|MASTER_PORT/, units: [] },
  30000: { label: "SGLang", processes: ["sglang"], cmd: /sglang/, units: [] },
};

/** Any other port bound to loopback. */
export const LOCAL_ONLY = "Local only";

/** Who can reach a listener: from the collector, or worked out from its address (older collectors, Demo). */
export function scopeOf(l: Pick<Listener, "addr" | "scope">): ListenerScope {
  if (l.scope) return l.scope;
  const a = l.addr;
  if (a === "0.0.0.0" || a === "::" || a === "*") return "all";
  if (a.startsWith("127.") || a === "::1") return "local";
  if (a.startsWith("192.168.100.") || a.startsWith("192.168.101.")) return "fabric";
  return "lan";
}

export const REACH_WORD: Record<ListenerScope, string> = { local: "This node only", all: "All interfaces", lan: "LAN", fabric: "Fabric" };

/** What usually listens there: "SMTP (Postfix)", "Local only" for an unknown loopback port, or null. */
export function usually(l: Pick<Listener, "port" | "addr" | "scope">): string | null {
  return KNOWN_PORTS[l.port]?.label ?? (scopeOf(l) === "local" ? LOCAL_ONLY : null);
}

export interface PortOwner {
  /** "nginx (2210)", "nginx (likely)", "postfix@-.service (likely)", "container vllm-head"; null when nothing says. */
  owner: string | null;
  confidence: "confirmed" | "likely" | null;
  via: "process" | "container" | "sudo" | "inferred" | null;
  /** The evidence, for the tooltip. */
  title: string;
}

const NEEDS_ROOT = "Seeing a port's owner needs root.";

const serves = (k: KnownPort, p: Process) => !p.kernel && (k.processes.includes(p.name) || (!!k.cmd && k.cmd.test(p.cmd)));

/**
 * Who owns a listening port. Confirmed: the pid the collector saw, a process listing the port,
 * a container's port mapping, or what sudo ss revealed. Otherwise likely: the process the port
 * belongs to, running on this node, else a process in its systemd unit.
 */
export function portOwner(node: NodeSnapshot, l: Listener, revealed?: Listener[]): PortOwner {
  const byPid = l.pid !== null ? node.processes.find((p) => p.pid === l.pid) : undefined;
  const proc = byPid ?? node.processes.find((p) => p.ports.includes(l.port));
  if (proc?.container) return { owner: `container ${proc.container}`, confidence: "confirmed", via: "container", title: `Container ${proc.container}` };
  if (l.process) return { owner: `${l.process} (${l.pid})`, confidence: "confirmed", via: "process", title: `${l.process}, pid ${l.pid}` };
  if (proc) return { owner: `${proc.name} (${proc.pid})`, confidence: "confirmed", via: "process", title: proc.cmd };
  const mapped = node.containers.find((c) => c.state === "running" && c.ports.some((m) => m.split("->")[0].split(":").pop() === String(l.port)));
  if (mapped) return { owner: `container ${mapped.name}`, confidence: "confirmed", via: "container", title: `Container ${mapped.name} publishes ${l.port}` };
  const seen = revealed?.find((r) => r.port === l.port && r.process);
  if (seen) return { owner: `${seen.process}${seen.pid ? ` (${seen.pid})` : ""}`, confidence: "confirmed", via: "sudo", title: "Read with sudo ss" };
  const known = KNOWN_PORTS[l.port];
  if (known) {
    const runner = node.processes.find((p) => serves(known, p));
    if (runner) {
      return {
        owner: `${runner.name} (likely)`,
        confidence: "likely",
        via: "inferred",
        title: `${NEEDS_ROOT} SparkBox matched it to the ${runner.name} process running on ${node.name}.`,
      };
    }
    const unit = known.units.find((u) => node.processes.some((p) => p.unit === u));
    if (unit) {
      return { owner: `${unit} (likely)`, confidence: "likely", via: "inferred", title: `${NEEDS_ROOT} SparkBox matched it to ${unit}, which is running on ${node.name}.` };
    }
  }
  return { owner: null, confidence: null, via: null, title: `${NEEDS_ROOT} Reveal with sudo shows it.` };
}

/** "c1", "c3", "gx10": a node's name short enough for a narrow Node column. */
export function shortNode(name: string): string {
  const m = name.match(/^([a-z])[a-z]*(\d+)$/i);
  if (m) return `${m[1]}${m[2]}`;
  return name.split(/[-_.]/)[0].slice(0, 6);
}
