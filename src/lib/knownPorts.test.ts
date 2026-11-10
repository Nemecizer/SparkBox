import { describe, expect, it } from "vitest";
import { portOwner, REACH_WORD, scopeOf, shortNode, usually } from "./knownPorts";
import type { Listener, NodeSnapshot, Process } from "./types";

// The listeners the real cluster reported on Oct 29 (snapshot-tour-2.json), as `ss` shows them
// to the cluster user: no owners.
const L = (port: number, addr: string): Listener => ({ port, addr, pid: null, process: null });
const TOUR: Record<string, Listener[]> = {
  cell1: [L(22, "0.0.0.0"), L(25, "0.0.0.0"), L(53, "127.0.0.53"), L(631, "127.0.0.1"), L(3350, "::1"), L(3389, "::"), L(8080, "0.0.0.0"), L(11000, "127.0.0.1"), L(11434, "127.0.0.1")],
  cell2: [L(22, "0.0.0.0"), L(25, "0.0.0.0"), L(53, "127.0.0.54"), L(631, "127.0.0.1"), L(3350, "::1"), L(3389, "::"), L(11000, "127.0.0.1")],
  gx10: [L(22, "0.0.0.0"), L(25, "127.0.0.1"), L(53, "127.0.0.53"), L(631, "127.0.0.1"), L(3350, "::1"), L(3389, "::"), L(11000, "127.0.0.1"), L(43179, "127.0.0.1")],
};

// The processes each node runs, with the full names the collector now sends (live/fixtures/doctor).
const P = (name: string, unit: string | null, extra: Partial<Process> = {}): Process =>
  ({ pid: 1000 + name.length, name, user: "root", cmd: `/usr/sbin/${name}`, cpuPct: 0, rss: 0, nice: 0, container: null, unit, ports: [], gpuMem: 0, ...extra });
const DAEMONS = [P("sshd", "ssh.service"), P("master", "postfix@-.service"), P("qmgr", "postfix@-.service"), P("systemd-resolved", "systemd-resolved.service"),
  P("cupsd", "cups.service"), P("xrdp-sesman", "xrdp-sesman.service"), P("xrdp", "xrdp.service"),
  P("dashboard-service", "dgx-dashboard.service", { comm: "dashboard-servi", user: "dgx-dashboard-service-user" })];
const node = (id: string, listeners: Listener[], extra: Process[] = []): NodeSnapshot =>
  ({ id, name: id, host: id, status: "online", latencyMs: null, error: null, listeners, processes: [...DAEMONS, ...extra], containers: [] } as unknown as NodeSnapshot);

const cell1 = node("cell1", TOUR.cell1, [P("nginx", "nginx.service"), P("nginx", "nginx.service", { pid: 2211, user: "www-data" }), P("ollama", "ollama.service", { user: "ollama", cmd: "/usr/local/bin/ollama serve" })]);
const cell2 = node("cell2", TOUR.cell2);
const gx10 = node("gx10-0000", TOUR.gx10);

describe("listening ports on the real Sparks", () => {
  it("names cell1's owners from its processes, and says they're likely", () => {
    const owners = Object.fromEntries(cell1.listeners.map((l) => [l.port, portOwner(cell1, l).owner]));
    expect(owners).toEqual({
      22: "sshd (likely)", 25: "master (likely)", 53: "systemd-resolved (likely)", 631: "cupsd (likely)", 3350: "xrdp-sesman (likely)",
      3389: "xrdp (likely)", 8080: "nginx (likely)", 11000: "dashboard-service (likely)", 11434: "ollama (likely)",
    });
    const nginx = portOwner(cell1, cell1.listeners.find((l) => l.port === 8080)!);
    expect(nginx).toMatchObject({ confidence: "likely", via: "inferred" });
    expect(nginx.title).toBe("Seeing a port's owner needs root. SparkBox matched it to the nginx process running on cell1.");
  });

  it("never calls 8080 llama.cpp unless llama-server runs", () => {
    const l = cell1.listeners.find((x) => x.port === 8080)!;
    expect(usually(l)).toBe("HTTP (nginx, llama-server…)");
    const llama = node("cell2", [L(8080, "0.0.0.0")], [P("llama-server", null, { cmd: "/usr/bin/llama-server -m /m/q.gguf --port 8080" })]);
    expect(portOwner(llama, llama.listeners[0]).owner).toBe("llama-server (likely)");
    expect(portOwner(cell2, L(8080, "0.0.0.0")).owner).toBeNull();
  });

  it("falls back to the unit, then to nothing with what usually listens there", () => {
    const noMaster = node("cell2", TOUR.cell2);
    noMaster.processes = noMaster.processes.filter((p) => p.name !== "master");
    expect(portOwner(noMaster, L(25, "0.0.0.0")).owner).toBe("postfix@-.service (likely)");
    const bare = { ...cell2, processes: [] } as NodeSnapshot;
    expect(portOwner(bare, L(25, "0.0.0.0"))).toMatchObject({ owner: null, confidence: null });
    expect(usually(L(25, "0.0.0.0"))).toBe("SMTP (Postfix)");
    // gx10-0000's unknown loopback port.
    expect(portOwner(gx10, L(43179, "127.0.0.1")).owner).toBeNull();
    expect(usually(L(43179, "127.0.0.1"))).toBe("Local only");
    expect(usually(L(43179, "0.0.0.0"))).toBeNull();
  });

  it("a pid the collector could see is confirmed", () => {
    const own = node("cell2", [{ port: 8000, addr: "0.0.0.0", pid: 4242, process: "python3" }]);
    expect(portOwner(own, own.listeners[0])).toMatchObject({ owner: "python3 (4242)", confidence: "confirmed" });
  });

  it("says who can reach each port", () => {
    const reach = (l: Listener) => REACH_WORD[scopeOf(l)];
    expect(cell1.listeners.map((l) => `${l.port} ${reach(l)}`)).toEqual([
      "22 All interfaces", "25 All interfaces", "53 This node only", "631 This node only", "3350 This node only", "3389 All interfaces",
      "8080 All interfaces", "11000 This node only", "11434 This node only",
    ]);
    expect(reach(L(25, "127.0.0.1"))).toBe("This node only");
    expect(reach({ ...L(5000, "192.168.100.1") })).toBe("Fabric");
    expect(reach({ ...L(5000, "192.168.50.51") })).toBe("LAN");
    expect(reach({ ...L(5000, "0.0.0.0"), scope: "local" })).toBe("This node only");
  });

  it("shortens node names for a narrow column", () => {
    expect(["cell1", "cell3", "gx10-0000"].map(shortNode)).toEqual(["c1", "c3", "gx10"]);
  });
});
