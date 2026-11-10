"""Peer watching for the node agent: tells a network split from a node outage.

Every 15 s each agent polls every other node on every path it has (the LAN address and
each fabric address): GET /healthz on the agent port and, when that fails, SSH on :22 (its
"SSH-" greeting, or a refused connection: the host answered; a node hung out of memory
accepts the connection but never greets). It keeps the result per peer and per path, and
reads each answering peer's own view of the cluster from its /healthz.

Leader: an agent that reaches its gateway (the witness that it isn't the one cut off),
hasn't lost the fabric, and sees no lower-index agent answering and healthy (reaching its
own gateway and not cut off, by that agent's /healthz). No quorum: when cell1, cell2 and
cell3 die together, gx10-0000 still pages, and says the view is a minority one ("seen from
gx10-0000 only"). In a true split where both halves reach the gateway each half pages the
other: a duplicate the Mac dedupes by (rule, entity), never silence. Only the leader pages
about peers.

What the leader says about a peer (after 60 s):
  /healthz answers on some path, every fabric path fails  warning   fabric-lost    "cell3 isn't reachable over the fabric"
  no /healthz anywhere (or 503 wedged), :22 answers       warning   agent-wedged   "The agent on cell3 isn't answering"
  nothing answers on any path                             critical  peer-unreachable
What every agent says about itself (after 60 s):
  no peer answers over the fabric, some over the LAN      critical  fabric-lost    "cell2 lost the fabric"
  no peer answers at all and the gateway doesn't either   critical  fabric-lost    "cell2 is cut off from the cluster"
The fabric switch: when every agent heard over the LAN (at least two, this one included)
says it lost the fabric (no peer over it for a minute, or every fabric link down: a switch
that lost power drops them all at once), the switch is down, not the nodes. The lowest-index of them that
reaches its gateway raises one critical fabric-switch-down ("CRS804 DDQ isn't answering:
all 4 nodes lost the fabric at 10:42"); each node's own fabric-lost becomes a warning in
that switch's incident, which its gate coalesces.
An agent that has lost the fabric or is cut off doesn't report peers missing (the switch's
pager excepted: it still pages a peer nothing answers over the LAN).

Standard library only: the node agent imports it.
"""
import json
import socket
import struct
import threading
import time
import urllib.error
import urllib.request

EVERY = 15.0
#: A path that hasn't answered for this long is down; a peer down on every path is missing.
MISSING_S = 60.0
HEALTHZ_TIMEOUT = 3.0
TCP_TIMEOUT = 2.0
#: A node that lost the fabric waits this long for its peers' views before it pages that
#: itself (polls arrive every 15 s, so two rounds and a half): the switch may be the cause.
SWITCH_GRACE_S = 45.0


def majority(n):
    return n // 2 + 1


class Path:
    def __init__(self, addr, kind, start):
        self.addr, self.kind = addr, kind
        # Counted as answering from when the watch starts, so a node down at boot is
        # reported after the same minute as one that goes down later.
        self.healthz_mono = start
        self.ssh_mono = start
        self.ssh_tested = False  # :22 is only tried when /healthz fails
        self.ok_ms = None        # when /healthz last answered (wall clock)
        self.ssh_answered = None  # monotonic time :22 last answered
        self.wedged = False

    def healthz_ok(self, mono):
        return self.healthz_mono is not None and mono - self.healthz_mono < MISSING_S

    def ssh_ok(self, mono):
        return self.ssh_mono is not None and mono - self.ssh_mono < MISSING_S

    def answers(self, mono):
        return self.healthz_ok(mono) or self.ssh_ok(mono)


class Peer:
    def __init__(self, data, index, port, start):
        self.id = data["id"]
        self.name = data.get("name") or self.id
        self.index = int(data["index"]) if isinstance(data.get("index"), int) else index
        self.port = int(data.get("port") or port)
        lan = data.get("lan") or data.get("addr")
        fabric = [a for a in data.get("fabric") or [] if a]
        self.paths = ([Path(lan, "lan", start)] if lan else []) + [Path(a, "fabric", start) for a in fabric if a != lan]
        self.view = None  # its last /healthz body

    def has(self, kind):
        return any(p.kind == kind for p in self.paths)

    def over(self, kind, mono):
        """True/False: /healthz answers over the LAN or any fabric path; None: no such path."""
        paths = [p for p in self.paths if p.kind == kind]
        return any(p.healthz_ok(mono) for p in paths) if paths else None

    def seen_over(self, kind, mono):
        """What was actually heard (not the minute's benefit of the doubt at start): True when
        /healthz answered over that kind of path within the minute, False when it hasn't,
        None with no such path or before the first answer or poll."""
        paths = [p for p in self.paths if p.kind == kind]
        if not paths:
            return None
        if any(p.ok_ms is not None and p.healthz_ok(mono) for p in paths):
            return True
        return False if any(p.ssh_tested for p in paths) else None

    def agent_ok(self, mono):
        return any(p.healthz_ok(mono) for p in self.paths)

    def reachable(self, mono):
        return any(p.answers(mono) for p in self.paths)

    def reachable_over(self, kind, mono):
        return any(p.answers(mono) for p in self.paths if p.kind == kind)

    def wedged(self, mono):
        return not self.agent_ok(mono) and any(p.wedged for p in self.paths)


def http_healthz(addr, port, timeout=HEALTHZ_TIMEOUT):
    """(status, body) of a peer's /healthz; raises OSError when nothing answers."""
    host = "[%s]" % addr if ":" in addr else addr
    try:
        with urllib.request.urlopen("http://%s:%d/healthz" % (host, port), timeout=timeout) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        try:
            body = json.loads(e.read() or b"{}")
        except ValueError:
            body = {}
        finally:
            e.close()
        return e.code, body
    except ValueError:
        return 200, {}


def tcp_answers(addr, port, timeout=TCP_TIMEOUT):
    """Something at addr answered on port: it accepted, or refused (the host is up)."""
    try:
        with socket.create_connection((addr, port), timeout=timeout):
            return True
    except ConnectionRefusedError:
        return True
    except OSError:
        return False


def ssh_answers(addr, port=22, timeout=TCP_TIMEOUT):
    """The node is alive on SSH: sshd greeted ("SSH-…"), or the kernel refused the port. A
    node hung out of memory still accepts the connection in the kernel but never greets."""
    try:
        with socket.create_connection((addr, port), timeout=timeout) as s:
            s.settimeout(timeout)
            return s.recv(8).startswith(b"SSH-")
    except ConnectionRefusedError:
        return True
    except OSError:
        return False


def default_gateway(route_text):
    """The IPv4 default gateway from /proc/net/route, or None."""
    for line in route_text.splitlines()[1:]:
        f = line.split()
        if len(f) >= 3 and f[1] == "00000000" and f[2] != "00000000":
            try:
                return socket.inet_ntoa(struct.pack("<L", int(f[2], 16)))
            except (ValueError, struct.error):
                continue
    return None


def gateway_answers(addr, tcp=tcp_answers, ping=None):
    """The gateway is there: DNS, HTTPS or HTTP answers, or a ping does."""
    if not addr:
        return False
    if any(tcp(addr, port, TCP_TIMEOUT) for port in (53, 443, 80)):
        return True
    return bool(ping and ping(addr))


class PeerWatch:
    """The cluster as this agent sees it. `poll()` probes (concurrently, in the caller's
    thread or a watch thread of its own); everything else reads the last results."""

    def __init__(self, me, index, nodes, port, fetch=http_healthz, tcp=ssh_answers, gateway=None, start=None):
        """nodes: agent.json peers, this node included or not ({id, name, index, lan, fabric, port});
        gateway: a callable that says whether this node's gateway answers."""
        start = time.monotonic() if start is None else start
        self.me, self.index, self.port = me, index, port
        listed = [n for n in nodes if n.get("id")]
        self.peers = [Peer(p, i, port, start) for i, p in enumerate(listed) if p["id"] != me]
        self.names = {p["id"]: p.get("name") or p["id"] for p in listed}
        self.size = len({p["id"] for p in listed} | {me})
        self.fetch, self.tcp, self.gateway = fetch, tcp, gateway
        self.gateway_mono = start
        self.lock = threading.Lock()

    # -- probing

    def probe(self, peer, path, mono, wall_ms):
        try:
            status, body = self.fetch(path.addr, peer.port)
        except (OSError, ValueError):
            status, body = None, None
        wedged = status == 503 and bool((body or {}).get("wedged"))
        ok = status == 200 and (body or {}).get("node") in (peer.id, None)
        ssh = False if ok else self.tcp(path.addr, 22, TCP_TIMEOUT)
        with self.lock:
            if ok:
                path.healthz_mono, path.ok_ms, path.wedged, path.ssh_tested = mono, wall_ms, False, False
                if isinstance(body, dict):
                    peer.view = body
                return
            path.wedged = path.wedged or wedged
            path.ssh_tested = True
            if ssh:
                path.ssh_mono = path.ssh_answered = mono

    def poll(self, mono=None, wall_ms=None):
        """One round: every path of every peer at once, and the gateway."""
        mono = time.monotonic() if mono is None else mono
        wall_ms = int(time.time() * 1000) if wall_ms is None else wall_ms
        threads = [threading.Thread(target=self.probe, args=(peer, path, mono, wall_ms), daemon=True)
                   for peer in self.peers for path in peer.paths]
        if self.gateway is not None:
            threads.append(threading.Thread(target=self.probe_gateway, args=(mono,), daemon=True))
        for t in threads:
            t.start()
        deadline = time.monotonic() + HEALTHZ_TIMEOUT + TCP_TIMEOUT + 1
        for t in threads:
            t.join(max(deadline - time.monotonic(), 0))
        return mono

    def probe_gateway(self, mono):
        if self.gateway():
            with self.lock:
                self.gateway_mono = mono

    # -- what it means

    def gateway_ok(self, mono):
        return self.gateway is None or mono - self.gateway_mono < MISSING_S

    def isolated(self, mono, fabric_here=True):
        """None, "fabric" (no peer over the fabric, some over the LAN) or "network" (no peer
        and no gateway)."""
        with self.lock:
            if not self.peers:
                return None
            if not any(p.reachable(mono) for p in self.peers):
                return "network" if not self.gateway_ok(mono) else None
            with_fabric = [p for p in self.peers if p.has("fabric")]
            if fabric_here and with_fabric and not any(p.reachable_over("fabric", mono) for p in with_fabric) \
                    and any(p.reachable_over("lan", mono) for p in self.peers):
                return "fabric"
            return None

    def seen(self, mono):
        """The nodes this agent sees, itself first: for "seen from gx10-0000 only"."""
        with self.lock:
            return [self.me] + [p.id for p in self.peers if p.reachable(mono)]

    def minority(self, mono):
        """It sees fewer than a majority of the nodes, itself included."""
        return len(self.seen(mono)) < majority(self.size)

    def leader(self, mono, fabric_here=True):
        """This agent pages about peers: it reaches its gateway, hasn't lost the fabric or been
        cut off, and no lower-index agent answers and is healthy (reaches its gateway and
        isn't cut off, by its own /healthz). An older agent that doesn't report its gateway
        counts as reaching it."""
        if not self.gateway_ok(mono) or self.isolated(mono, fabric_here):
            return False
        with self.lock:
            for p in self.peers:
                v = p.view or {}
                if p.index < self.index and p.agent_ok(mono) and not v.get("isolated") and v.get("gateway") is not False:
                    return False
        return True

    def switch_down(self, mono, fabric_here=True, links_down=False):
        """The fabric switch is down, not the nodes: this agent lost the fabric (no peer over it,
        or `links_down`: every fabric link here is down), and so did every agent it hears over
        the LAN (at least one), by its /healthz. (pager, nodes, since_ms): the lowest-index of
        them that reaches its gateway pages it (None when none does), the nodes that lost the
        fabric in inventory order, and when a fabric path last answered. None otherwise."""
        if not links_down and self.isolated(mono, fabric_here) != "fabric":
            return None
        with self.lock:
            heard = [p for p in self.peers if p.over("lan", mono) and p.view]
            if not heard or any(p.view.get("isolated") != "fabric" and p.view.get("fabricDown") is not True for p in heard):
                return None
            group = sorted([(self.index, self.me, self.gateway_ok(mono))] + [(p.index, p.id, p.view.get("gateway") is not False) for p in heard])
            since = max((x.ok_ms for p in self.peers for x in p.paths if x.kind == "fabric" and x.ok_ms), default=None)
        pager = next((n for _, n, gw in group if gw), None)
        return pager, [n for _, n, _ in group], since

    def view(self, mono):
        """{node: {lan, fabric, ssh, lastOkMs, wedged}} for /healthz and status."""
        with self.lock:
            return {p.id: {"lan": p.seen_over("lan", mono), "fabric": p.seen_over("fabric", mono),
                           # None while /healthz answers (:22 isn't tried then).
                           "ssh": any(x.ssh_answered is not None and mono - x.ssh_answered < MISSING_S for x in p.paths)
                           if any(x.ssh_tested for x in p.paths) else None,
                           "lastOkMs": max((x.ok_ms for x in p.paths if x.ok_ms), default=None), "wedged": p.wedged(mono)}
                    for p in self.peers}

    def judge(self, mono, fabric_here=True, links_down=False):
        """(leader, isolated, findings): findings are (rule, severity, peer, why) for the
        peers this agent pages about (none unless it leads) plus its own. Under a switch
        verdict this node's fabric-lost is a warning ("switch"), the pager adds
        ("fabric-switch-down", "crit", None, {pager, nodes, sinceMs}) and leads over the LAN."""
        isolated = self.isolated(mono, fabric_here)
        sw = self.switch_down(mono, fabric_here, links_down)
        pager = sw is not None and sw[0] == self.me
        lead = self.leader(mono, fabric_here) or pager
        out = []
        if isolated:
            out.append(("fabric-lost", "warn" if sw else "crit", None, "switch" if sw else isolated))
        if pager:
            out.append(("fabric-switch-down", "crit", None, {"pager": sw[0], "nodes": sw[1], "sinceMs": sw[2]}))
        if lead:
            with self.lock:
                for p in self.peers:
                    if not p.paths:
                        continue
                    if not p.reachable(mono):
                        out.append(("peer-unreachable", "crit", p, "unreachable"))
                    elif not p.agent_ok(mono):
                        out.append(("agent-wedged", "warn", p, "wedged" if p.wedged(mono) else "silent"))
                    elif p.over("fabric", mono) is False and p.over("lan", mono) and not sw:
                        out.append(("fabric-lost", "warn", p, "fabric"))
        return lead, isolated, out

    def missing(self, mono, fabric_here=True):
        """The peers this agent reports missing: only the leader, which pages them."""
        lead, _, findings = self.judge(mono, fabric_here)
        return [f[2].id for f in findings if lead and f[0] == "peer-unreachable"]


def words(names):
    """"cell1", "cell1 and cell2", "cell1, cell2 and cell3"."""
    names = list(names)
    return names[0] if len(names) == 1 else "%s and %s" % (", ".join(names[:-1]), names[-1]) if names else ""


def outage_title(down, seen, minority):
    """One page for the peers nothing answers: "cell3 isn't answering on any path", or
    "cell1, cell2 and cell3 aren't answering on any path (seen from gx10-0000 only)" when
    the view is a minority one."""
    verb = "isn't" if len(down) == 1 else "aren't"
    tail = " (seen from %s only)" % words(seen) if minority else ""
    return "%s %s answering on any path%s" % (words(down), verb, tail)
