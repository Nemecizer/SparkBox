// r4-nodes' mock handlers: the nodes as the Live tour found them. Every node logs in with the
// shared cluster password; Wake-on-LAN can't be read without sudo; Services comes from one
// `systemctl show` with each unit's type and timer; the Test of a node whose host key isn't
// trusted yet stops there and says so.
import type { MockHandlers } from "../../test/tauri";
import { handlers as ops } from "./ops";

const NODES = ["cell1", "cell2", "cell3", "gx10-0000"];
/** The key cell1, cell2 and cell3 share (a cloned image), and gx10-0000's. */
const CLONED = "SHA256:BZ5Ax5nfR3u/LCFrjzg3eroa9t08yevI46BSyXedAbg";
const GX10 = "SHA256:nIaTibRJmp5ZwhYU6VwFEDVsD0+j5agpqky/U9s2yC0";
/** Services a timer starts on Ubuntu (what `systemctl show` says as TriggeredBy). */
const TIMER = new Set(["anacron.service", "apt-daily.service", "apt-daily-upgrade.service", "fstrim.service", "logrotate.service", "man-db.service", "motd-news.service"]);

type Unit = { unit: string; description: string; load: string; active: string; sub: string; enabled: string };

const step = (id: string, label: string, status: string, detail: string, extra: Record<string, unknown> = {}) => ({ id, label, status, detail, hint: null, ...extra });

export const handlers: MockHandlers = {
  services_poll: ({ node, files }: { node: string; files: boolean }) => {
    const units = (ops.services_list({ node }) as Unit[]).map((u) => ({
      ...u,
      kind: TIMER.has(u.unit) || u.sub === "exited" ? "oneshot" : "simple",
      triggeredBy: TIMER.has(u.unit) ? [u.unit.replace(".service", ".timer")] : [],
      result: u.active === "failed" ? "exit-code" : "success",
      condition: true,
    }));
    return { units, files: files ? units.map((u) => [u.unit, u.enabled]) : null };
  },
  login_methods: () =>
    NODES.map((n) => ({
      node: n, name: n, method: "password", detail: "password", summary: `${n} · shared cluster password`,
      agent: "SSH_AUTH_SOCK", agentRunning: true, keys: [], password: true, jump: null, notes: [],
    })),
  node_wol_status: ({ node }: { node: string }) => ({
    node, mac: null, iface: "enP7s7", enabled: null, detail: "Not readable without sudo. NetworkManager leaves it to the driver default.",
    routes: NODES.filter((n) => n !== node).slice(0, 1).map((n) => ({ kind: "peer", node: n, name: n })), fix: null, persist: null,
  }),
  node_probe: ({ node }: { node: { id: string; name: string; host: string; ip: string | null; port: number; user: string } }) => {
    const addr = `${node.ip ?? node.host}:${node.port}`;
    const head = [step("resolve", "Find the node", "ok", `Uses its address, ${node.ip ?? node.host}`), step("tcp", "Reach SSH", "ok", `${addr} answers`)];
    // cell3 as at bring-up: first contact, its key not trusted yet.
    if (node.id === "cell3") {
      return [
        ...head,
        step("hostkey", "Host key", "warn", `Not trusted yet (first connection). The node's key is ${CLONED}`,
          { fingerprint: CLONED, address: addr, hint: "Compare it on the node: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub. If it shows the same, choose Trust…" }),
        ...["Log in", "Login method", "sudo", "Python 3", "Docker", "GPU driver", "Fabric addresses", "Wake-on-LAN"].map((label, i) =>
          step(["auth", "method", "sudo", "python", "docker", "gpu", "fabric", "wol"][i], label, "skip", "After you trust the host key", { blocked: true })),
      ];
    }
    const n = NODES.indexOf(node.id) + 1;
    return [
      ...head,
      step("hostkey", "Host key", "ok", `Matches the key SparkBox trusts (${node.id === "gx10-0000" ? GX10 : CLONED})`),
      step("auth", "Log in", "ok", `Logged in as ${node.user}`, { authMethod: "password" }),
      step("method", "Login method", "ok", "Password (your shared cluster password, from the Keychain)", { authMethod: "password" }),
      step("sudo", "sudo", "ok", "Works with the cluster password"),
      step("python", "Python 3", "ok", "Python 3.12.3"),
      step("docker", "Docker", "ok", "Docker 29.6.2"),
      step("gpu", "GPU driver", "ok", "NVIDIA GB10, driver 580.178.04"),
      step("fabric", "Fabric addresses", "ok", `enp1s0f1np1 192.168.100.${n}/24, enP2p1s0f1np1 192.168.101.${n}/24`),
      step("wol", "Wake-on-LAN", "skip", "Not readable without sudo. NetworkManager leaves it to the driver default.", { blocked: false }),
    ];
  },
  known_hosts_trust: () => null,
};
