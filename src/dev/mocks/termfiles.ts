// Terminal and Files backend for the browser preview: an in-memory copy of the demo world's
// files (the same model folders, /etc/hosts and netplan as src-tauri/src/files/demo.rs, plus a
// small Mac home), saves with conflict checks and backups, the Trash, transfers that take time,
// and a simulated shell and log stream for terminal tabs.
import type { MockHandlers } from "../../test/tauri";
import { channelSend } from "../../test/tauri";
// The owner's own glm53 launch script, as Demo's as-found homes carry it (files/demo.rs INNER_SH).
import INNER_SH from "../../../src-tauri/src/demo/fixtures/inner.sh?raw";

interface Item {
  dir?: boolean;
  size: number;
  mtime: number;
  mode: number;
  owner: string;
  text?: string;
  link?: string;
}

const USER = "sparkuser";
const HOME = `/home/${USER}`;
const MAC_HOME = `/Users/${USER}`;
const NODES = ["cell1", "cell2", "cell3", "gx10-0000"];
const LAN = ["192.168.50.51", "192.168.50.154", "192.168.50.98", "192.168.50.181"];
const now = () => Math.floor(Date.now() / 1000);
const T0 = now() - 3 * 86400;

const dir = (owner = USER, mode = 0o755): Item => ({ dir: true, size: 4096, mtime: T0, mode, owner });
const text = (t: string, owner = USER, mode = 0o644, mtime = T0): Item => ({ size: new TextEncoder().encode(t).length, mtime, mode, owner, text: t });
const blob = (size: number, owner = USER): Item => ({ size, mtime: T0, mode: 0o644, owner });

function hosts(node: string): string {
  let s = `127.0.0.1 localhost\n127.0.1.1 ${node}\n\n# Home LAN (enP7s7)\n`;
  NODES.forEach((n, i) => (s += `${LAN[i].padEnd(15)} ${n === "gx10-0000" ? n : `${n}-example`}.local\n`));
  s += "\n# 200G fabric (enp1s0f1np1, MTU 9000)\n";
  NODES.forEach((n, i) => (s += `${`192.168.100.${i + 1}`.padEnd(15)} ${n}-fabric\n`));
  s += "\n# Second fabric link (enP2p1s0f1np1)\n";
  NODES.forEach((n, i) => (s += `${`192.168.101.${i + 1}`.padEnd(15)} ${n}-fabric2\n`));
  return s;
}

function netplan(i: number): string {
  return `# ConnectX-7 fabric links to the CRS804 (written by the cluster setup)\nnetwork:\n  version: 2\n  ethernets:\n    enp1s0f1np1:\n      dhcp4: false\n      addresses: [192.168.100.${i + 1}/24]\n      mtu: 9000\n    enP2p1s0f1np1:\n      dhcp4: false\n      addresses: [192.168.101.${i + 1}/24]\n      mtu: 9000\n`;
}

/** How the owner started each node's rank (files/demo.rs node_launch_sh). */
const NODE_LAUNCH_SH = `#!/bin/bash
# Starts this node's rank of the 4-node GLM-5.3-Flash run: ./node-launch.sh <rank 0-3>
set -euo pipefail
RANK="\${1:?rank 0-3}"
HERE="$(cd "$(dirname "$0")" && pwd)"
IMAGE=vllm/vllm-openai:glm53-flash-arm64-cu130

docker rm -f glm53 2>/dev/null || true
mkdir -p ~/.cache/flashinfer ~/.triton ~/.cache/vllm

docker run -d --name glm53 \\
  --network host --ipc host --shm-size 16g --gpus all \\
  --env-file "$HERE/config.env" \\
  -e API_KEY="$(cat "$HERE/api_key")" \\
  -v ~/.cache/flashinfer:/root/.cache/flashinfer \\
  -v ~/.triton:/root/.triton \\
  -v "$HERE":/glm53:ro \\
  -v ~/models:/models:ro \\
  -v ~/.cache/vllm:/root/.cache/vllm \\
  "$IMAGE" /bin/bash /glm53/inner.sh "$RANK"

echo "glm53 rank $RANK started; follow it with: docker logs -f glm53"
`;

function seedNode(node: string): Map<string, Item> {
  const i = NODES.indexOf(node);
  const m = new Map<string, Item>();
  for (const d of ["/", "/etc", "/etc/netplan", "/etc/docker", "/etc/systemd", "/etc/systemd/system", "/home", "/var", "/var/log"]) m.set(d, dir("root"));
  for (const d of ["", "/models", "/.cache", "/.cache/huggingface", "/.cache/huggingface/hub", "/projects", "/projects/bench", "/.sparkbox", "/.local", "/.local/share"]) m.set(`${HOME}${d}`, dir());
  m.set("/etc/hosts", text(hosts(node), "root"));
  m.set("/etc/hostname", text(`${node}\n`, "root"));
  m.set("/etc/netplan/40-cx7-fabric.yaml", text(netplan(i), "root"));
  m.set("/etc/netplan/50-cloud-init.yaml", text("network:\n  version: 2\n  ethernets:\n    enP7s7:\n      dhcp4: true\n", "root", 0o600));
  // As on the real Sparks: the fabric plan the owner wrote, root-only.
  m.set("/etc/netplan/40-cx7.yaml", text(netplan(i), "root", 0o600));
  m.set("/etc/docker/daemon.json", text('{\n  "default-runtime": "nvidia",\n  "runtimes": {\n    "nvidia": { "path": "nvidia-container-runtime", "args": [] }\n  },\n  "log-opts": { "max-size": "50m", "max-file": "3" }\n}\n', "root"));
  m.set("/etc/systemd/system/sparkbox-fabric-mtu.service", text("[Unit]\nDescription=Set jumbo frames on the CX-7 fabric links\nAfter=network-online.target\n\n[Service]\nType=oneshot\nExecStart=/usr/sbin/ip link set enp1s0f1np1 mtu 9000\nExecStart=/usr/sbin/ip link set enP2p1s0f1np1 mtu 9000\n\n[Install]\nWantedBy=multi-user.target\n", "root"));
  m.set("/etc/resolv.conf", { ...blob(39, "root"), mode: 0o777, link: "../run/systemd/resolve/stub-resolv.conf" });
  m.set("/var/log/syslog", { ...blob(18_421_337 + i * 1_310_731, "syslog"), mtime: now() - 40, text: undefined });
  m.set("/var/log/dpkg.log", text("2026-10-24 06:12:40 upgrade docker-ce:arm64 5:28.3.1 5:28.3.2\n", "root"));
  // ~/glm53 as the Live tour found it on every node (the 4-node GLM-5.3-Flash run of Oct 26);
  // api_key and config.env are placeholders, as in Demo (files/demo.rs found_home).
  m.set(`${HOME}/glm53`, dir(USER, 0o775));
  m.set(`${HOME}/glm53/inner.sh`, text(INNER_SH, USER, 0o775));
  m.set(`${HOME}/glm53/node-launch.sh`, text(NODE_LAUNCH_SH, USER, 0o775));
  m.set(`${HOME}/glm53/api_key`, text("demo-placeholder-not-the-real-key\n", USER, 0o600));
  m.set(`${HOME}/glm53/config.env`, text("# Read by node-launch.sh (docker run --env-file). Placeholder in Demo: the real one stays on the node.\nPORT=8000\nNNODES=4\nHEAD_IP=192.168.100.1\nSERVED_NAME=glm-5.3-flash\nMODEL_LOCAL_NAME=GLM-5.3-Flash-NVFP4\nNCCL_SOCKET_IFNAME=enp1s0f1np1\n", USER, 0o600));
  m.set(`${HOME}/.bashrc`, text("# ~/.bashrc\nexport HF_HOME=~/.cache/huggingface\nalias gpu='nvidia-smi'\n"));
  m.set(`${HOME}/notes.txt`, text(`${node}\n- fabric: 192.168.100.${i + 1} / 192.168.101.${i + 1}\n- check MTU 9000 on both CX-7 links after updates\n`));
  m.set(`${HOME}/projects/bench/sweep.yaml`, text("# Throughput sweep for the served model\nmodel: llama-3.3-70b\nendpoint: http://192.168.100.1:8000/v1\nconcurrency: [1, 4, 16, 64]\nprompt_tokens: 1024\noutput_tokens: 256\nrepeat: 3\n"));
  m.set(`${HOME}/projects/bench/run_bench.py`, text('#!/usr/bin/env python3\n"""Runs the sweep in sweep.yaml against the vLLM endpoint."""\nimport yaml\n\n\ndef main(path="sweep.yaml"):\n    with open(path) as f:\n        cfg = yaml.safe_load(f)\n    for c in cfg["concurrency"]:\n        print(f"concurrency={c}")\n', USER, 0o755));
  m.set(`${HOME}/projects/bench/results.csv`, text("concurrency,prompt_tps,gen_tps,ttft_p50_ms\n1,412.0,38.1,182\n4,1460.2,131.7,240\n16,4120.8,402.3,515\n64,6890.1,688.9,1340\n"));
  const folders: [string, number, number, string[]][] = [
    ["meta-llama/Llama-3.3-70B-Instruct", 141_107_412_992, 30, ["cell1", "cell2", "cell3"]],
    ["Qwen/Qwen3.8-27B-FP8", 30_923_764_531, 7, ["cell1"]],
  ];
  for (const [repo, bytes, files, on] of folders) {
    if (!on.includes(node)) continue;
    const d = `${HOME}/models/${repo}`;
    m.set(`${HOME}/models/${repo.split("/")[0]}`, dir());
    m.set(d, dir());
    m.set(`${d}/config.json`, text('{\n  "architectures": ["LlamaForCausalLM"],\n  "hidden_size": 8192,\n  "num_hidden_layers": 80,\n  "torch_dtype": "bfloat16",\n  "vocab_size": 128256\n}\n'));
    m.set(`${d}/generation_config.json`, text('{\n  "temperature": 0.6,\n  "top_p": 0.9\n}\n'));
    m.set(`${d}/tokenizer.json`, blob(17_209_920));
    const shard = Math.floor((bytes - 17_300_000) / files);
    for (let k = 1; k <= files; k++) m.set(`${d}/model-${String(k).padStart(5, "0")}-of-${String(files).padStart(5, "0")}.safetensors`, blob(shard));
  }
  if (["cell1", "cell2", "cell3"].includes(node)) m.set(`${HOME}/models/latest`, { ...blob(33), mode: 0o777, link: "meta-llama/Llama-3.3-70B-Instruct" });
  return m;
}

function seedMac(): Map<string, Item> {
  const m = new Map<string, Item>();
  for (const d of ["/", "/Users", MAC_HOME, `${MAC_HOME}/Downloads`, `${MAC_HOME}/Documents`, `${MAC_HOME}/Projects`, `${MAC_HOME}/Projects/cluster-configs`]) m.set(d, dir(d === "/" || d === "/Users" ? "root" : USER));
  m.set(`${MAC_HOME}/.zshrc`, text("export PATH=\"$HOME/.local/bin:$PATH\"\n"));
  m.set(`${MAC_HOME}/Downloads/Llama-3.3-70B-Instruct-Q4_K_M.gguf`, { ...blob(42_520_412_160), mtime: now() - 7200 });
  m.set(`${MAC_HOME}/Downloads/sparkbox-1.0.dmg`, blob(48_211_002));
  m.set(`${MAC_HOME}/Downloads/notes.txt`, text("Bring the 200G DAC for cell3.\n", USER, 0o644, now() - 600));
  m.set(`${MAC_HOME}/Projects/cluster-configs/40-cx7-fabric.yaml`, text(netplan(2), USER, 0o644, now() - 3600));
  m.set(`${MAC_HOME}/Projects/cluster-configs/hosts`, text(hosts("cell1"), USER, 0o644, now() - 3600));
  m.set(`${MAC_HOME}/Projects/cluster-configs/notes.txt`, text("cell1\n- fabric: 192.168.100.1 / 192.168.101.1\n- MTU 9000 checked after the 580.95 driver update\n", USER, 0o644, now() - 300));
  m.set(`${MAC_HOME}/Projects/cluster-configs/projects`, { ...dir(), mtime: now() - 60 });
  m.set(`${MAC_HOME}/Projects/cluster-configs/projects/bench`, { ...dir(), mtime: now() - 60 });
  m.set(`${MAC_HOME}/Projects/cluster-configs/projects/bench/sweep.yaml`, text("model: llama-3.3-70b\nconcurrency: [1, 4, 16, 64, 128]\nrepeat: 5\n", USER, 0o644, now() - 60));
  return m;
}

const fs = new Map<string, Map<string, Item>>();
const tree = (place: string) => {
  let t = fs.get(place);
  if (!t) fs.set(place, (t = place === "mac" ? seedMac() : seedNode(place)));
  return t;
};
const homeOf = (place: string) => (place === "mac" ? MAC_HOME : HOME);
function norm(place: string, p: string): string {
  const h = homeOf(place);
  const full = p === "" || p === "~" ? h : p.startsWith("~/") ? `${h}/${p.slice(2)}` : p;
  const out: string[] = [];
  for (const part of full.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") out.pop();
    else out.push(part);
  }
  return `/${out.join("/")}`;
}
const parent = (p: string) => (p.lastIndexOf("/") <= 0 ? "/" : p.slice(0, p.lastIndexOf("/")));
const base = (p: string) => p.slice(p.lastIndexOf("/") + 1);
const modeStr = (it: Item) => `${it.link ? "l" : it.dir ? "d" : "-"}${[...Array(9)].map((_, i) => (it.mode & (0o400 >> i) ? "rwx"[i % 3] : "-")).join("")}`;
const sub = (t: Map<string, Item>, p: string) => [...t.keys()].filter((k) => k === p || k.startsWith(`${p}/`));
const sizeOf = (t: Map<string, Item>, p: string) => sub(t, p).reduce((s, k) => s + (t.get(k)!.dir || t.get(k)!.link ? 0 : t.get(k)!.size), 0);
function hash(s: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193) >>> 0;
  return h.toString(16).padStart(8, "0").repeat(8);
}
const fail = (message: string) => Promise.reject(message);

function listing(place: string, path: string) {
  const t = tree(place);
  const p = norm(place, path);
  if (!t.get(p)?.dir) return fail(`can't open ${p}: No such file or directory`);
  const entries = [...t.entries()]
    .filter(([k]) => k !== "/" && parent(k) === p)
    .map(([k, it]) => ({
      name: base(k), kind: it.link ? "link" : it.dir ? "dir" : "file", size: it.size, mtime: it.mtime, mode: modeStr(it),
      target: it.link ?? null, linkDir: !!it.link && !!t.get(norm(place, it.link.startsWith("/") ? it.link : `${p}/${it.link}`))?.dir, owner: it.owner,
    }));
  const used = [...t.values()].reduce((s, it) => s + (it.dir ? 0 : it.size), 0);
  const capacity = place === "mac" ? 994_662_584_320 : place === "gx10-0000" ? 1_022_202_216_448 : 4_000_787_030_016;
  return Promise.resolve({ place, path: p, entries, free: Math.max(0, capacity - used - (place === "mac" ? 512e9 : 1.08e12)), capacity });
}

function readFile(place: string, path: string) {
  const t = tree(place);
  const p = norm(place, path);
  const it = t.get(p);
  if (!it) return fail(`${p}: No such file or directory`);
  if (it.dir) return fail(`${p} is a folder`);
  // The backend's permission refusal (files/mod.rs permission_refusal), as the editor reads it.
  if (it.owner === "root" && !(it.mode & 0o004)) {
    const mode = (it.mode & 0o7777).toString(8);
    return Promise.reject({ kind: "permission", node: place, message: `Only root can read ${p.split("/").pop()} (root, ${mode}). Reading it needs sudo, which needs Admin.`, detail: JSON.stringify({ path: p, mode, owner: "root" }) });
  }
  const info = { path: p, size: it.size, mtime: it.mtime, mode: modeStr(it), owner: `${it.owner}:${it.owner}`, writable: it.owner === USER };
  if (it.text !== undefined) return Promise.resolve({ ...info, content: it.text, sha256: hash(it.text), large: false, head: null, tail: null });
  if (p.endsWith(".json") || p.includes("/var/log/")) {
    const line = (n: number) => `Oct 28 ${String(9 + (n >> 10)).padStart(2, "0")}:${String((n >> 4) % 60).padStart(2, "0")}:${String(n % 60).padStart(2, "0")} ${place} kernel: mlx5_core 0000:01:00.1 enp1s0f1np1: Link up\n`;
    return Promise.resolve({ ...info, content: "", sha256: "", large: true, head: Array.from({ length: 400 }, (_, n) => line(n)).join(""), tail: Array.from({ length: 400 }, (_, n) => line(40_000 + n)).join("") });
  }
  return fail(`${p} looks like a binary file`);
}

const backups = new Map<string, { path: string; mtime: number; size: number; text: string }[]>();

const termSessions = new Map<string, { node: string; target: { kind: string; container?: string; unit?: string; cwd?: string }; ch: unknown; line: string; timer?: number; cwd: string }>();
let termSeq = 0;
let copySeq = 0;
const copies = new Map<string, number>();

const enc = (s: string) => btoa(unescape(encodeURIComponent(s.replace(/\n/g, "\r\n"))));
const prompt = (s: { node: string; cwd: string }) => `\x1b]0;${USER}@${s.node}: ${s.cwd.replace(HOME, "~")}\x07\x1b[01;32m${USER}@${s.node}\x1b[0m:\x1b[01;34m${s.cwd.replace(HOME, "~")}\x1b[0m$ `;

function run(s: { node: string; cwd: string }, cmd: string): string {
  const [c, ...args] = cmd.trim().split(/\s+/);
  const t = tree(s.node);
  switch (c) {
    case "":
      return "";
    case "help":
      return "Browser preview: ls, cd, pwd, cat, hostname, nvidia-smi, docker ps, df -h, clear.\n";
    case "pwd":
      return `${s.cwd}\n`;
    case "hostname":
      return `${s.node}\n`;
    case "uptime":
      return ` ${new Date().toTimeString().slice(0, 8)} up 3 days,  4:12,  1 user,  load average: 0.84, 0.91, 0.88\n`;
    case "cd": {
      const p = norm(s.node, args[0] ? (args[0].startsWith("/") || args[0].startsWith("~") ? args[0] : `${s.cwd}/${args[0]}`) : "~");
      if (t.get(p)?.dir) s.cwd = p;
      else return `-bash: cd: ${args[0]}: No such file or directory\n`;
      return "";
    }
    case "ls": {
      const names = [...t.keys()].filter((k) => k !== "/" && parent(k) === s.cwd && !base(k).startsWith(".")).map((k) => (t.get(k)!.dir ? `\x1b[01;34m${base(k)}\x1b[0m` : t.get(k)!.link ? `\x1b[01;36m${base(k)}\x1b[0m` : base(k)));
      return names.length ? `${names.join("  ")}\n` : "";
    }
    case "cat": {
      const it = t.get(norm(s.node, args[0]?.startsWith("/") ? args[0] : `${s.cwd}/${args[0] ?? ""}`));
      return it?.text ?? `cat: ${args[0] ?? ""}: No such file or directory\n`;
    }
    case "nvidia-smi":
      return "+-----------------------------------------------------------------------------------------+\n| NVIDIA-SMI 580.95.05              Driver Version: 580.95.05      CUDA Version: 13.0     |\n|=========================================+========================+======================|\n|   0  NVIDIA GB10                    On  |   0000000F:01:00.0 Off |                  N/A |\n| N/A   62C    P0            71W /  N/A  | Not Supported          |     80%      Default |\n+-----------------------------------------+------------------------+----------------------+\n";
    case "docker":
      return "NAMES        STATUS        IMAGE\nvllm-head    Up 3 hours    nvcr.io/nvidia/vllm:26.05-py3\n";
    case "df":
      return "Filesystem      Size  Used Avail Use% Mounted on\n/dev/nvme0n1p2  3.7T  1.1T  2.4T  32% /\n";
    case "clear":
      return "\x1b[2J\x1b[3J\x1b[H";
    default:
      return `${c}: command not found (browser preview: type help)\n`;
  }
}

export const handlers: MockHandlers = {
  files_list: ({ place, path }) => listing(place, path),
  files_read: ({ place, path }) => readFile(place, path),
  // Read with sudo…: root reads it (the preview has no sudo; Admin mode is the gate).
  files_read_sudo: ({ place, path }) => {
    const it = tree(place).get(norm(place, path));
    if (!it || it.dir || it.text === undefined) return fail(`${norm(place, path)}: No such file or directory`);
    return Promise.resolve({ path: norm(place, path), size: it.size, mtime: it.mtime, mode: modeStr(it), owner: `${it.owner}:${it.owner}`, writable: false, content: it.text, sha256: hash(it.text), large: false, head: null, tail: null });
  },
  files_write: ({ place, path, content, sudo, expectSha256 }) => {
    const t = tree(place);
    const p = norm(place, path);
    const cur = t.get(p);
    if (expectSha256 && cur?.text !== undefined && hash(cur.text) !== expectSha256) {
      return Promise.reject({ kind: "changedOnDisk", message: `${p} changed on disk after you opened it.`, sha256: hash(cur.text), mtime: cur.mtime, size: cur.size });
    }
    if (cur && cur.owner !== USER && !sudo) return Promise.reject({ kind: "needsSudo", message: `${p} belongs to ${cur.owner}. Save it with sudo.` });
    if (cur?.text !== undefined) {
      const list = backups.get(`${place}:${p}`) ?? [];
      const backup = `${homeOf(place)}/.sparkbox/backups/${hash(p).slice(0, 16)}/${base(p)}.${Date.now()}`;
      list.unshift({ path: backup, mtime: now(), size: cur.size, text: cur.text });
      backups.set(`${place}:${p}`, list.slice(0, 5));
      t.set(backup, text(cur.text, USER, 0o600, now()));
    }
    t.set(p, { ...(cur ?? text("")), ...text(content, cur?.owner ?? USER, cur?.mode ?? 0o644, now()) });
    return { path: p, sha256: hash(content), mtime: now(), size: content.length, backup: null };
  },
  files_backups: ({ place, path }) => (backups.get(`${place}:${norm(place, path)}`) ?? []).map(({ text: _t, ...b }) => b),
  files_mkdir: ({ place, dir: d, name }) => {
    tree(place).set(norm(place, `${d}/${name}`), { ...dir(), mtime: now() });
    return null;
  },
  files_rename: ({ place, path, newName }) => {
    const t = tree(place);
    const from = norm(place, path);
    const to = `${parent(from)}/${newName}`;
    for (const k of sub(t, from)) {
      t.set(`${to}${k.slice(from.length)}`, t.get(k)!);
      t.delete(k);
    }
    return null;
  },
  files_trash: ({ place, paths }) => {
    const t = tree(place);
    const trash = place === "mac" ? `${MAC_HOME}/.Trash` : `${HOME}/.local/share/Trash/files`;
    t.set(trash, dir());
    return (paths as string[]).map((raw) => {
      const p = norm(place, raw);
      const dest = `${trash}/${base(p)}`;
      for (const k of sub(t, p)) {
        t.set(`${dest}${k.slice(p.length)}`, t.get(k)!);
        t.delete(k);
      }
      return { path: p, trashed: dest };
    });
  },
  files_untrash: ({ place, items }) => {
    const t = tree(place);
    for (const it of items as { path: string; trashed: string }[]) {
      for (const k of sub(t, it.trashed)) {
        t.set(`${it.path}${k.slice(it.trashed.length)}`, t.get(k)!);
        t.delete(k);
      }
    }
    return null;
  },
  files_delete: ({ place, paths }) => {
    const t = tree(place);
    for (const raw of paths as string[]) for (const k of sub(t, norm(place, raw))) t.delete(k);
    return null;
  },
  files_chmod: ({ place, path, mode }) => {
    const it = tree(place).get(norm(place, path));
    if (it) it.mode = mode;
    return null;
  },
  files_du: ({ place, paths }) => (paths as string[]).map((p) => ({ path: p, bytes: sizeOf(tree(place), norm(place, p)) })),
  files_preflight: ({ spec }) => {
    if (spec.to === "mac" && spec.from !== "mac") throw "Demo nodes are simulated: their files can't be copied to this Mac.";
    const src = tree(spec.from);
    const dst = tree(spec.to);
    const side = (t: Map<string, Item>, p: string) => {
      const it = t.get(p);
      return it ? { kind: it.dir ? "dir" : "file", bytes: sizeOf(t, p), entries: sub(t, p).length, mtime: it.mtime, tarBytes: sizeOf(t, p) } : null;
    };
    let bytes = 0;
    let entries = 0;
    const conflicts = [];
    for (const n of spec.names as string[]) {
      const s = side(src, norm(spec.from, `${spec.dir}/${n}`));
      if (!s) return fail(`${n} is no longer in ${spec.dir}.`);
      bytes += s.bytes;
      entries += s.entries;
      const d = side(dst, norm(spec.to, `${spec.toDir}/${n}`));
      if (d) conflicts.push({ name: n, source: s, dest: d });
    }
    return { bytes, entries, conflicts, free: 2_400_000_000_000, route: spec.from === spec.to ? "local" : spec.from !== "mac" && spec.to !== "mac" ? "viaMac" : "direct" };
  },
  files_transfer: ({ spec, onEvent }) => {
    // As the backend: a Demo node's files never land on this Mac.
    if (spec.to === "mac" && spec.from !== "mac") throw "Demo nodes are simulated: their files can't be copied to this Mac.";
    const id = `x${++copySeq}`;
    const src = tree(spec.from);
    const files: [string, number][] = [];
    for (const n of spec.names as string[]) {
      if (spec.policies[n] === "skip") continue;
      for (const k of sub(src, norm(spec.from, `${spec.dir}/${n}`))) files.push([k, src.get(k)!.dir ? 0 : src.get(k)!.size]);
    }
    const total = files.reduce((s, [, b]) => s + b, 0);
    const started = Date.now();
    const seconds = Math.max(1.5, total / 1.15e9);
    const timer = window.setInterval(() => {
      const frac = Math.min(1, (Date.now() - started) / 1000 / seconds);
      const done = total * frac;
      let acc = 0;
      const k = Math.max(0, files.findIndex(([, b]) => (acc += b) >= done && b > 0));
      channelSend(onEvent, { kind: "progress", done, total, files: frac >= 1 ? files.length : k, filesTotal: files.length, current: files[k]?.[0].split("/").slice(-2).join("/") ?? null });
      if (frac < 1) return;
      window.clearInterval(timer);
      copies.delete(id);
      const dst = tree(spec.to);
      const placed = [];
      for (const n of spec.names as string[]) {
        const from = norm(spec.from, `${spec.dir}/${n}`);
        let name = n;
        if (spec.policies[n] === "skip") {
          placed.push({ name: n, action: "skipped", as: n });
          continue;
        }
        if (spec.policies[n] === "keepBoth" || (dst.has(norm(spec.to, `${spec.toDir}/${n}`)) && !spec.policies[n])) {
          for (let i = 2; dst.has(norm(spec.to, `${spec.toDir}/${name}`)); i++) name = n.replace(/(\.[^.]+)?$/, ` ${i}$1`);
        }
        const to = norm(spec.to, `${spec.toDir}/${name}`);
        for (const key of sub(src, from)) dst.set(`${to}${key.slice(from.length)}`, { ...src.get(key)!, owner: spec.to === "mac" ? USER : src.get(key)!.owner });
        if (spec.op === "move") for (const key of sub(src, from)) src.delete(key);
        placed.push({ name: n, action: name === n ? (spec.policies[n] ? "replaced" : "copied") : "keptBoth", as: name });
      }
      channelSend(onEvent, { kind: "done", bytes: total, files: files.length, placed });
    }, 250);
    copies.set(id, timer);
    return id;
  },
  files_cancel: ({ id }) => {
    const timer = copies.get(id);
    if (timer) window.clearInterval(timer);
    copies.delete(id);
    return null;
  },

  terminal_open: ({ node, target, onEvent }) => {
    const id = `t${++termSeq}`;
    const s = { node, target, ch: onEvent, line: "", cwd: target.cwd ?? HOME };
    termSessions.set(id, s);
    const send = (t: string) => channelSend(onEvent, { kind: "data", data: enc(t) });
    if (target.kind === "containerLogs" || target.kind === "journal") {
      let n = 0;
      const tick = () => {
        n++;
        const hms = new Date().toTimeString().slice(0, 8);
        send(target.kind === "journal"
          ? `Oct 28 ${hms} ${node} ${String(target.unit).replace(".service", "")}[1388]: ${["Health check passed.", "Reloading requested from client PID 2210.", "Finished docker-image-prune.timer."][n % 3]}\n`
          : `INFO 10-28 ${hms} [loggers.py:123] Engine 000: Avg prompt throughput: ${(380 + (n % 7) * 21).toFixed(1)} tokens/s, Avg generation throughput: ${(31 + (n % 5)).toFixed(1)} tokens/s, Running: ${n % 4} reqs, Waiting: 0 reqs, GPU KV cache usage: ${(12 + (n % 9)).toFixed(1)}%, Prefix cache hit rate: ${(41 + (n % 11)).toFixed(1)}%\n`);
      };
      for (let k = 0; k < 18; k++) tick();
      (s as { timer?: number }).timer = window.setInterval(tick, 1200);
    } else {
      setTimeout(() => {
        if (target.kind === "shell") send(`Welcome to DGX OS 7.2.3 (Ubuntu 24.04.3 LTS) on ${node} (browser preview)\n\nLast login: Tue Oct 28 08:41:12 2026 from 192.168.50.20\n`);
        send(target.kind === "containerShell" ? `root@${String(target.container).slice(0, 12)}:/workspace# ` : prompt(s));
      }, 30);
    }
    return id;
  },
  terminal_input: ({ id, data }) => {
    const s = termSessions.get(id);
    if (!s) return Promise.reject("That terminal has closed.");
    const send = (t: string) => channelSend(s.ch, { kind: "data", data: enc(t) });
    for (const ch of data as string) {
      if (ch === "\r") {
        send("\n");
        send(run(s, s.line));
        s.line = "";
        send(s.target.kind === "containerShell" ? `root@${String(s.target.container).slice(0, 12)}:/workspace# ` : prompt(s));
      } else if (ch === "\x7f") {
        if (s.line) {
          s.line = s.line.slice(0, -1);
          send("\b \b");
        }
      } else if (ch === "\x03") {
        s.line = "";
        send(`^C\n${prompt(s)}`);
      } else if (ch >= " ") {
        s.line += ch;
        send(ch);
      }
    }
    return null;
  },
  terminal_resize: () => null,
  terminal_ack: () => null,
  terminal_close: ({ id }) => {
    const s = termSessions.get(id);
    if (s && (s as { timer?: number }).timer) window.clearInterval((s as { timer?: number }).timer);
    termSessions.delete(id);
    return null;
  },
  terminal_save_text: ({ name }) => `${MAC_HOME}/Downloads/${name} 2026-10-28 11.02.log`,
  // ── r2-live ──
  terminal_authorize: () => null,
  terminal_broadcast: () => null,
  files_leftovers: () => [],
  files_leftover_remove: () => null,
  login_settings_get: () => loginSettings,
  login_settings_set: ({ settings }) => (loginSettings = settings),
  login_passphrase_set: () => null,
  login_setup_key: ({ nodes }) => (nodes as string[]).map((node) => ({ node, verified: true, passwordOff: false, message: "Key login works" })),
  login_methods: () =>
    NODES.map((n) => {
      const pw = n === "gx10-0000";
      const detail = pw ? "password" : "~/.ssh/id_ed25519 via agent";
      return {
        node: n, name: n, method: pw ? "password" : "agent", detail, summary: `${n} · ${pw ? "password" : `key ${detail}`}`,
        agent: "SSH_AUTH_SOCK", agentRunning: true, keys: ["~/.ssh/id_ed25519"], password: true, jump: null, notes: [],
      };
    }),
  node_wol_status: ({ node }) => ({
    node, mac: `4c:bb:47:0a:1b:${(NODES.indexOf(node) + 0x2c).toString(16)}`, iface: "enP7s7", enabled: true,
    routes: NODES.filter((n) => n !== node).slice(0, 1).map((n) => ({ kind: "peer", node: n, name: n })), fix: null, persist: null,
  }),
};

// Login settings (Settings › Cluster login).
let loginSettings = { agent: "system", identityFile: null as string | null, useSshConfig: true, passwordFallback: true };
