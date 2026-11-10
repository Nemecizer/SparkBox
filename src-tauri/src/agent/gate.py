"""The notification gate: what the node agent pages, and why it didn't page the rest.

SparkBox runs the same algorithm in Rust (notify.rs Gate); both pass every case in
fixtures/gate-parity.json. The unit of state is K = (rule, entity). Every alert event gets
an outcome, so a page is never dropped without a trace:

  silenced    a silence matches it
  planned     its node is in a maintenance window SparkBox started
  duplicate   K was sent raised at the same severity under 5 min ago, with no resolve since
              (the only dedupe)
  held        a warning while critical-only is on or in quiet hours; the resolve of a
              warning that was paged, in quiet hours; the resolve of anything paged until it
              has stayed clear for 10 min (flap damping: a tick sends it then), except a node
              back online
  flapping    raised again within 10 min of its resolve, at no more than what was paged:
              the earlier clear is never sent, and repeats say "Still flapping"; a node gone
              again within 10 min of a return, after it was paged as flapping (G16b)
  coalesced   a warning whose incident was paged at warning or above in the last 30 min,
              here or by another sender (a `paged` event); the incident's next message
              lists it ("Also: …")
  eased       critical -> warning (not paged)
  not-paged   the resolve of something never paged, resolves turned off, a rule that
              doesn't notify, a category that's off
  sent        criticals (quiet hours included), escalations, resolves of what was paged
              once clear for 10 min, a node back online at once (never held), a node gone
              again within 10 min of that return (the second time as "<title> · flapping";
              after that its drops and returns aren't paged unless it stays away 10 min:
              "Still flapping: <title>"), "Still firing" repeats of unacknowledged criticals,
              and when quiet hours end one digest of the warnings they held that still fire

Sends due within 10 s for the same incident become one message titled by its worst item.
More than two messages in one flush become one grouped message ("4 alerts, 2 critical")
whose lines name every critical first.

Quiet hours are evaluated in the IANA timezone the Mac declared when they were saved
(zoneinfo); only when none was given does the node's own clock decide.

Standard library only: the node agent imports it.
"""
import datetime
import time
import uuid

try:
    import zoneinfo
except ImportError:  # no zoneinfo: quiet hours follow the node's clock
    zoneinfo = None

GROUP_WAIT_MS = 10_000
DUPLICATE_MS = 5 * 60_000
COALESCE_MS = 30 * 60_000
#: More messages than this in one flush go out as one.
GROUP_OVER = 2
#: Lines in a grouped message before "and N more" (every critical is named regardless).
GROUP_LINES = 6
DEFAULT_REPEAT_MIN = 60
#: A paged alert's resolve waits this long clear; raised again sooner, it's flapping.
FLAP_MS = 10 * 60_000
#: An acknowledgement that arrives before its alert is raised here waits this long for it.
ACK_WAIT_MS = 15 * 60_000
RANK = {"info": 0, "warn": 1, "crit": 2}
#: "cell3 didn't come back after the restart": about the maintenance window itself.
OVERDUE_RULES = ("node-reboot",)
#: "cell3 is back online" is sent at once, never damped (notify.rs BACK_ONLINE_RULES).
BACK_ONLINE_RULES = ("node-offline", "peer-unreachable")


# ---- Settings and quiet hours

def minutes(hhmm):
    """'22:00' -> 1320, or None when it isn't a time of day."""
    try:
        h, m = (int(x) for x in str(hhmm).strip().split(":"))
    except ValueError:
        return None
    return h * 60 + m if 0 <= h < 24 and 0 <= m < 60 else None


def in_window(now_min, start, end):
    s, e = minutes(start), minutes(end)
    if s is None or e is None or s == e:
        return False
    return s <= now_min < e if s < e else now_min >= s or now_min < e


_zones = {}


def zone(name):
    """A zoneinfo zone by IANA name, or None (none given, unknown, or no tz database)."""
    if not name or zoneinfo is None:
        return None
    if name not in _zones:
        try:
            _zones[name] = zoneinfo.ZoneInfo(name)
        except (zoneinfo.ZoneInfoNotFoundError, ValueError, OSError):
            _zones[name] = None
    return _zones[name]


def clock(ms, tz_name=None):
    """'10:42' in the declared zone (else the node's), for messages."""
    tz = zone(tz_name)
    if tz is not None:
        return datetime.datetime.fromtimestamp(ms / 1000.0, tz).strftime("%H:%M")
    return time.strftime("%H:%M", time.localtime(ms / 1000.0))


class Settings:
    """When to notify, from agent.json: {criticalOnly, resolved, repeatMinutes, quiet:
    {start, end, tz}, categories, enabled}. r2's notify.json policy (quietHours,
    quietStart, quietEnd and one switch per category) reads the same way."""

    def __init__(self, data=None):
        d = data or {}
        self.enabled = d.get("enabled", True) is not False
        self.critical_only = bool(d.get("criticalOnly"))
        self.resolved = d.get("resolved", True) is not False
        repeat = d.get("repeatMinutes", DEFAULT_REPEAT_MIN)
        self.repeat_ms = int(repeat * 60_000) if isinstance(repeat, (int, float)) and not isinstance(repeat, bool) and repeat > 0 else None
        q = d.get("quiet")
        if q is None and d.get("quietHours"):
            q = {"start": d.get("quietStart", "22:00"), "end": d.get("quietEnd", "07:00")}
        self.quiet = q if isinstance(q, dict) and q.get("start") and q.get("end") else None
        cats = d.get("categories")
        if not isinstance(cats, dict):
            cats = {k: d[k] for k in ("node", "disk", "gpu", "fabric", "containers", "serving") if isinstance(d.get(k), bool)}
        self.categories = cats

    def category_on(self, category):
        return self.categories.get(category or "node", True) is not False

    def quiet_tz(self):
        """The zone quiet hours follow: its IANA name; "missing" when none (or an unknown
        one) was given, so the node's clock decides; None without quiet hours."""
        if not self.quiet:
            return None
        tz = self.quiet.get("tz")
        return tz if tz and zone(tz) is not None else "missing"

    def quiet_at(self, now_ms):
        if not self.quiet:
            return False
        tz = zone(self.quiet.get("tz"))
        if tz is not None:
            t = datetime.datetime.fromtimestamp(now_ms / 1000.0, tz)
            now_min = t.hour * 60 + t.minute
        else:
            lt = time.localtime(now_ms / 1000.0)
            now_min = lt.tm_hour * 60 + lt.tm_min
        return in_window(now_min, self.quiet["start"], self.quiet["end"])


# ---- Entities and incidents

def entity_key(entity):
    """The entity part of K, spelled as SparkBox spells it: node:cell3, endpoint:llama,
    switch:crs804, disk:cell1:/, gpu:cell1:gpu0, net:cell2/enP2p1s0f1np1,
    container:cell1/vllm-head, service:cell1/ollama.service, port:crs804/qsfp28-2-1."""
    if isinstance(entity, str):
        return entity
    kind, ident = entity.get("kind"), entity.get("id")
    scope = entity.get("node") or entity.get("switch") or ""
    if kind in ("node", "endpoint", "switch"):
        return "%s:%s" % (kind, ident)
    if kind == "port":
        return "port:%s/%s" % (entity.get("switch") or scope, ident)
    if kind in ("disk", "gpu"):
        return "%s:%s:%s" % (kind, scope, ident)
    return "%s:%s/%s" % ("net" if kind == "interface" else kind, scope, ident)


def parse_entity(key):
    """An entity key back to (kind, scope, id), whichever separator it was written with."""
    kind, _, rest = str(key or "").partition(":")
    kind = "net" if kind == "interface" else kind
    if kind in ("node", "endpoint", "switch"):
        return kind, None, rest
    for sep in (":", "/"):
        if sep in rest:
            scope, _, ident = rest.partition(sep)
            return kind, scope, ident
    return kind, None, rest


def same_entity(a, b):
    return a == b or parse_entity(a) == parse_entity(b)


#: Rules about a node being reachable and reporting: rooted at node:<id>.
NODE_RULES = ("node-offline", "node-reboot", "node-stale", "agent-silent", "agent-config-stale", "peer-unreachable",
              "agent-wedged", "fabric-lost")
#: Interface rules rooted at the link they're about.
LINK_RULES = ("roce-iface", "fabric-flap", "net-errors", "kernel-mlx5", "fabric-mtu")
#: Switch-port rules rooted at the link cabled to the port.
PORT_LINK_RULES = ("port-errors", "optic-rx-low", "fabric-cabling")


def half_index(ip, order, iface):
    if ip and ip.startswith("192.168.101."):
        return 1
    if ip and ip.startswith("192.168.100."):
        return 0
    return order.index(iface) % 2 if iface in order else 0


class Roots:
    """Which incident an alert belongs to: the app's health/incident.rs root_of, run on the
    same cases (health/fixtures/incident-roots.json). One optic is one incident
    (link:<node>/<iface>, with the switch port cabled to it); link alerts on both halves of
    one node are the node's (node:<id>: one cable, optic or port behind both); a node and
    what falls with it is node:<id>; a model, its member containers and a stage it lost are endpoint:<id> or the
    node or GPU that took it down; the switch and its unmapped ports are switch:<id>.

    ifaces: [{node, iface, ip, switchPort}]; gpu_fault: nodes with a fatal GPU fault;
    endpoints: [{id, headNode, nodes, containers, missingNodes}]; active: the alerts firing."""

    def __init__(self, ifaces=(), gpu_fault=(), endpoints=(), active=()):
        self.port_ifaces = {}
        order = {}
        for x in ifaces:
            order.setdefault(x["node"], []).append(x["iface"])
        for x in ifaces:
            if x.get("switchPort"):
                self.port_ifaces.setdefault(x["switchPort"], []).append(
                    (x["node"], x["iface"], half_index(x.get("ip"), order[x["node"]], x["iface"])))
        for v in self.port_ifaces.values():
            v.sort(key=lambda t: t[2])
        self.gpu_fault = set(gpu_fault)
        self.endpoints = list(endpoints)
        self.links_active, self.links_down, self.nodes_down = set(), set(), set()
        self.links_by_node = {}  # node -> its interfaces with a link alert (two: both halves)
        for a in active:
            e = a.get("entity") or a
            rule, node, ident = a.get("rule"), e.get("node") or "", e.get("id")
            firing = a.get("state", "firing") == "firing"
            if firing and (rule == "gpu-unreadable" or (rule == "kernel-xid" and a.get("severity") == "crit")):
                self.gpu_fault.add(node)
            if rule in LINK_RULES and e.get("kind", "interface") == "interface":
                self.links_active.add("%s/%s" % (node, ident))
                self.links_by_node.setdefault(node, set()).add(ident)
            if rule == "fabric-down":
                self.links_down.add("%s/%s" % (node, ident))
            gone = rule in ("node-offline", "node-reboot", "node-stale", "fabric-down", "peer-unreachable") or (rule == "port-down" and e.get("node"))
            if gone and firing:
                self.nodes_down.add(node)

    def endpoint_root(self, ep):
        members = list(ep.get("missingNodes") or []) + list(ep.get("nodes") or []) + [ep.get("headNode")]
        down = next((n for n in members if n in self.nodes_down), None)
        if down:
            return "node:%s" % down
        faulty = next((n for n in members if n in self.gpu_fault), None)
        if faulty:
            return "gpu:%s" % faulty
        return "endpoint:%s" % ep["id"]

    def root_of(self, alert):
        e = alert.get("entity") or alert
        rule, kind, ident = alert.get("rule") or "", e.get("kind"), e.get("id")
        node, switch = e.get("node") or "", e.get("switch")
        if rule in NODE_RULES and kind == "node":
            return "node:%s" % ident
        if kind == "interface":
            if rule == "fabric-down":
                return "node:%s" % node
            if rule in LINK_RULES and "%s/%s" % (node, ident) in self.links_down:
                return "node:%s" % node
            if rule in LINK_RULES and node and any(i != ident for i in self.links_by_node.get(node, ())):
                return "node:%s" % node  # both halves
            if rule in LINK_RULES and node:
                return "link:%s/%s" % (node, ident)
        if kind == "port":
            if rule == "port-down" and node:
                return "node:%s" % node
            if rule in PORT_LINK_RULES:
                cabled = [t for t in self.port_ifaces.get("%s/%s" % (switch or "", ident), []) if not node or t[0] == node]
                # The half with a RoCE alert, else the first half cabled there.
                pick = next((t for t in cabled if "%s/%s" % (t[0], t[1]) in self.links_active), cabled[0] if cabled else None)
                if pick and len(self.links_by_node.get(pick[0], ())) > 1:
                    return "node:%s" % pick[0]
                return "link:%s/%s" % pick[:2] if pick else "switch:%s" % (switch or "")
            return "switch:%s" % (switch or ident)
        if kind == "switch":
            return "switch:%s" % (switch or ident)
        if kind == "gpu":
            return "gpu:%s" % node
        if kind == "endpoint":
            ep = next((x for x in self.endpoints if x.get("id") == ident), None)
            return self.endpoint_root(ep) if ep else "endpoint:%s" % ident
        if kind == "container" and rule in ("crash-loop", "container-exited"):
            ep = next((x for x in self.endpoints if ident in (x.get("containers") or []) and (not x.get("nodes") or node in x["nodes"])), None)
            if ep:
                return self.endpoint_root(ep)
        if kind == "node":
            return "node:%s" % ident  # mem-high, kernel-oom…: the node's own incident (gate-parity C1)
        return "%s:%s/%s" % (kind, switch or node, ident)


# ---- The gate

def new_id():
    return "m-%s" % uuid.uuid4().hex[:12]


def key(ev):
    return "%s|%s" % (ev.get("rule"), ev.get("entity"))


def node_of(ev):
    """The node an event is about: its `node`, else from its entity key."""
    if ev.get("node"):
        return ev["node"]
    kind, scope, ident = parse_entity(ev.get("entity"))
    return ident if kind == "node" else scope if kind in ("net", "container", "gpu", "disk", "service") else None


class Gate:
    """decide(events, now) -> one outcome per alert event; tick(now) -> "Still firing"
    repeats; flush(now) -> the messages whose 10 s group window is over.

    An event is {kind, rule, entity (a key), severity, incident, title, body, node, label,
    category, notify, alertId}; kind is raised | escalated | deescalated | resolved, or one
    that sets state: ack {rule, entity}, silence {rule|null, entity, reason},
    maintenance {node, until (ms) | None, reason}, maintenance-end {node}, paged {incident,
    severity, title, by} (another sender paged that incident).
    `ctx` adds the silences and maintenance the agent holds: ctx.silenced(ev) and
    ctx.planned(ev, now) return a reason or None."""

    def __init__(self, settings=None, ctx=None, state=None):
        self.settings = settings or Settings()
        self.ctx = ctx
        s = state or {}
        self.keys = s.get("keys") or {}            # "rule|entity" -> what was sent for K
        self.incidents = s.get("incidents") or {}  # root -> {pagedSeverity, pagedAtMs, also: [titles]}
        self.silences = s.get("silences") or []    # from silence events
        self.maintenance = s.get("maintenance") or {}  # node -> {untilMs, reason}, from maintenance events
        self.pending = s.get("pending") or []      # groups waiting out the window
        self.acks = s.get("acks") or {}            # "rule|entity" -> ms: acknowledged before it was raised here
        self.quiet_was = bool(s.get("quietWas"))   # quiet hours were on at the last tick
        self.digest = s.get("digest")              # the morning digest, sent with the next flush

    def state(self):
        """Everything to keep across a restart (state.json), so a restart neither repeats
        pages nor forgets what to resolve."""
        return {"keys": self.keys, "incidents": self.incidents, "silences": self.silences,
                "maintenance": self.maintenance, "pending": self.pending, "acks": self.acks,
                "quietWas": self.quiet_was, "digest": self.digest}

    def find(self, ev):
        """K for an event, whichever separator its entity was written with (an ack from the Mac)."""
        k = self.keys.get(key(ev))
        if k is None:
            k = next((x for x in self.keys.values() if x.get("rule") == ev.get("rule") and same_entity(x.get("entity"), ev.get("entity"))), None)
        return k

    def silenced(self, ev):
        for s in self.silences:
            if (s.get("rule") in (None, ev.get("rule"))) and same_entity(s.get("entity"), ev.get("entity")):
                return s.get("reason") or "silenced"
        return self.ctx.silenced(ev) if self.ctx else None

    def planned(self, ev, now):
        if ev.get("rule") in OVERDUE_RULES:
            return None
        m = self.maintenance.get(node_of(ev) or "")
        if m and (m.get("untilMs") is None or now < m["untilMs"]):
            return m.get("reason") or "maintenance"
        return self.ctx.planned(ev, now) if self.ctx else None

    def decide(self, events, now):
        out = []
        for ev in events:
            kind = ev.get("kind")
            if kind == "silence":
                self.silences.append({"rule": ev.get("rule"), "entity": ev.get("entity"), "reason": ev.get("reason")})
            elif kind == "maintenance":
                until = ev.get("untilMs", ev.get("until"))
                self.maintenance[ev.get("node") or node_of(ev)] = {"untilMs": until, "reason": ev.get("reason")}
            elif kind == "maintenance-end":
                self.maintenance.pop(ev.get("node") or node_of(ev), None)
            elif kind == "ack":
                k = self.find(ev)
                if k and firing(k):
                    k["acked"] = True
                else:
                    self.acks[key(ev)] = ev.get("ackedMs") or now
            elif kind == "paged":
                self.paged_incident({"incident": ev.get("incident")}, ev.get("severity") or "warn", now)
            elif kind in ("raised", "escalated", "deescalated", "resolved"):
                out.append(self.one(ev, now))
        return out

    def one(self, ev, now):
        kind, sev = ev["kind"], ev.get("severity") or "warn"
        s = self.settings
        res = {"rule": ev.get("rule"), "entity": ev.get("entity"), "kind": kind, "severity": sev, "outcome": None}

        def outcome(name, reason=None):
            res["outcome"] = name
            if reason:
                res["reason"] = reason
            return res

        k = self.keys.get(key(ev))
        if kind in ("raised", "escalated"):
            flapped = False
            if k is not None and not firing(k):
                # Its resolve is still waiting out the 10 min (or a node came back less than 10 min
                # ago): within them, raised at no more than what was paged, it's flapping; above
                # that, or later, it starts afresh.
                flapped = kind == "raised" and now - k["clearedMs"] < FLAP_MS and RANK.get(sev, 0) <= RANK.get(k.get("pagedSeverity"), 0)
                if not flapped:
                    del self.keys[key(ev)]
                    k = None
            if k is None:
                k = self.keys[key(ev)] = {"rule": ev.get("rule"), "entity": ev.get("entity"), "raisedMs": now, "paged": False,
                                          "pagedSeverity": None, "lastPageMs": None, "sentKind": None, "sentSeverity": None,
                                          "sentAtMs": None, "acked": False, "title": ev.get("title"), "flaps": 0}
                acked = self.acks.pop(key(ev), None)
                if acked is not None and now - acked < ACK_WAIT_MS:
                    k["acked"] = True
            elif kind == "escalated" or RANK.get(sev, 0) > RANK.get(k.get("severity"), 0):
                k["acked"] = False  # an escalation un-acknowledges
            if kind == "raised" and not k.get("title"):
                k["title"] = ev.get("title")
            k.update({"severity": sev, "incident": ev.get("incident"), "ev": slim(ev), "clearedMs": None, "resolve": None, "awayMs": None})
            why = self.silenced(ev)
            if why:
                return outcome("silenced", why)
            why = self.planned(ev, now)
            if why:
                return outcome("planned", why)
            node = ev.get("rule") in BACK_ONLINE_RULES
            title = ev.get("title")
            if flapped:
                k["flaps"] = (k.get("flaps") or 0) + 1
                k["held"] = None
                ago = span(now - (k.get("lastClearMs") or now))
                if not node:
                    return outcome("flapping", "Raised again %s after it cleared" % ago)
                if k["flaps"] > 2:
                    # Paged as flapping already: neither its going away nor its return is, unless it
                    # stays away 10 min (tick).
                    k["awayMs"] = now
                    return outcome("flapping", "Away again %s after it came back; paged as flapping" % ago)
                # Its return was told: gone again, it's paged; the second time as flapping (G16b).
                if k["flaps"] == 2:
                    title = "%s · flapping" % (title or ev.get("rule"))
            if not s.enabled or ev.get("notify") is False or sev == "info":
                return outcome("not-paged", "doesn't notify")
            if not s.category_on(ev.get("category")):
                return outcome("not-paged", "category off")
            if (kind == "raised" and k.get("sentKind") == "raised" and k.get("sentSeverity") == sev
                    and k.get("sentAtMs") is not None and now - k["sentAtMs"] < DUPLICATE_MS):
                return outcome("duplicate")
            if sev != "crit" and kind == "raised":
                if s.critical_only:
                    return outcome("held", "critical only")
                if s.quiet_at(now):
                    k["held"] = ev.get("title")  # for the digest when quiet hours end
                    return outcome("held", "quiet hours")
                inc = self.incidents.get(ev.get("incident") or "")
                if inc and inc.get("pagedAtMs") is not None and now - inc["pagedAtMs"] < COALESCE_MS \
                        and RANK.get(inc.get("pagedSeverity"), 0) >= RANK["warn"]:
                    inc.setdefault("also", []).append(ev.get("title") or ev.get("rule"))
                    return outcome("coalesced", ev.get("incident"))
            k.update({"paged": True, "lastPageMs": now, "sentKind": kind, "sentSeverity": sev, "sentAtMs": now, "held": None,
                      "pagedSeverity": sev if RANK.get(sev, 0) >= RANK.get(k.get("pagedSeverity"), 0) else k["pagedSeverity"]})
            self.paged_incident(ev, sev, now)
            res["messageId"] = self.queue(ev, kind, sev, title, now)
            return outcome("sent")

        if kind == "deescalated":
            if k:
                k.update({"severity": sev, "ev": slim(ev)})
            return outcome("eased")

        # resolved: a later raise of K starts afresh, never a duplicate. What was paged waits
        # 10 min clear before its resolve goes out (tick), so a flap never says "Resolved";
        # a node back online goes at once, and K remembers it for 10 min (G16b).
        if k is None or not firing(k) or not k.get("paged"):
            self.keys.pop(key(ev), None)
            return outcome("not-paged")
        if self.silenced(ev):
            self.keys.pop(key(ev), None)
            return outcome("silenced", self.silenced(ev))
        if not s.resolved:
            self.keys.pop(key(ev), None)
            return outcome("not-paged", "resolves off")
        if s.quiet_at(now) and k.get("pagedSeverity") != "crit":
            self.keys.pop(key(ev), None)
            return outcome("held", "quiet hours")
        if ev.get("rule") in BACK_ONLINE_RULES:
            away = k.get("awayMs") is not None
            k.update({"clearedMs": now, "lastClearMs": now, "awayMs": None, "acked": False, "resolve": None})
            if away:
                # A flapping node's going away wasn't paged: nor is its return.
                return outcome("not-paged", "Flapping: its going away wasn't paged either")
            # Held, it would say a node is down that's up: sent now.
            k.update({"sentKind": "resolved", "sentSeverity": sev, "sentAtMs": now})
            res["messageId"] = self.queue(ev, "resolved", sev, ev.get("title"), now)
            return outcome("sent", "Back online: sent at once")
        k.update({"clearedMs": now, "lastClearMs": now, "acked": False,
                  "resolve": dict(slim(ev), rule=ev.get("rule"), entity=ev.get("entity"), severity=sev)})
        return outcome("held", "Sent once it has stayed clear for 10 min")

    def paged_incident(self, ev, sev, now):
        inc = self.incidents.setdefault(ev.get("incident") or key(ev), {})
        if inc.get("pagedAtMs") is None or now - inc["pagedAtMs"] >= COALESCE_MS or RANK.get(sev, 0) >= RANK.get(inc.get("pagedSeverity"), 0):
            inc["pagedSeverity"] = sev
        inc["pagedAtMs"] = now

    def queue(self, ev, kind, sev, title, now):
        """Into its incident's group; sent when the group's 10 s are up."""
        incident = ev.get("incident") or key(ev)
        group = next((g for g in self.pending if g["incident"] == incident), None)
        if group is None:
            group = {"incident": incident, "dueMs": now + GROUP_WAIT_MS, "messageId": new_id(), "items": []}
            self.pending.append(group)
        group["items"].append({"kind": kind, "severity": sev, "rule": ev.get("rule"), "entity": ev.get("entity"),
                               "title": title or "%s on %s" % (ev.get("rule"), ev.get("entity")), "body": ev.get("body") or "",
                               "node": node_of(ev), "alertId": ev.get("alertId"), "t": now})
        return group["messageId"]

    def tick(self, now):
        """The morning digest when quiet hours have just ended, the resolves that have stayed
        clear for 10 min, then "Still firing: <title>" ("Still flapping" once it has flapped)
        for each critical that was paged, isn't acknowledged, silenced or planned, and hasn't
        been paged for repeatMinutes."""
        out = self.digest_if_quiet_ended(now)
        quiet = self.settings.quiet_at(now)
        for k in list(self.keys.values()):
            if firing(k) or now - k["clearedMs"] < FLAP_MS:
                continue
            del self.keys[key(k)]
            if not k.get("resolve"):
                continue  # a node's return, told when it came (G16b)
            r = k.get("resolve") or {}
            o = {"rule": k["rule"], "entity": k["entity"], "kind": "resolved", "severity": r.get("severity"), "outcome": "sent",
                 "reason": "Clear for 10 min"}
            if quiet and k.get("pagedSeverity") != "crit":
                o.update(outcome="held", reason="quiet hours")
            else:
                o["messageId"] = self.queue(r, "resolved", r.get("severity") or "warn", r.get("title"), now)
            out.append(o)
        out.extend(self.lasting_aways(now, quiet))
        every = self.settings.repeat_ms
        if not every:
            return out
        for k in list(self.keys.values()):
            if not firing(k) or k.get("severity") != "crit" or not k.get("paged") or k.get("acked") or k.get("lastPageMs") is None:
                continue
            if now - k["lastPageMs"] < every:
                continue
            ev = dict(k.get("ev") or {}, rule=k["rule"], entity=k["entity"])
            if self.silenced(ev) or self.planned(ev, now):
                continue
            k["lastPageMs"] = now
            k["awayMs"] = None  # this says it's away, so its return is told
            word = "Still flapping" if k.get("flaps") else "Still firing"
            mid = self.queue(ev, "repeat", "crit", "%s: %s" % (word, k.get("title") or ev.get("title")), now)
            out.append({"rule": k["rule"], "entity": k["entity"], "kind": "repeat", "severity": "crit", "outcome": "sent", "messageId": mid})
        for k_, at in list(self.acks.items()):
            if now - at >= ACK_WAIT_MS:
                del self.acks[k_]
        return out

    def lasting_aways(self, now, quiet):
        """A flapping node whose going away wasn't paged and that is still away 10 min later:
        "Still flapping: <title>", and its return will be told (G16b; notify.rs lasting_aways).
        A critical always; a warning not in quiet hours or with critical only on."""
        out = []
        for k in list(self.keys.values()):
            if not firing(k) or k.get("awayMs") is None or now - k["awayMs"] < FLAP_MS or k.get("acked"):
                continue
            ev = dict(k.get("ev") or {}, rule=k["rule"], entity=k["entity"])
            if self.silenced(ev) or self.planned(ev, now):
                continue
            sev = k.get("severity") or "crit"
            if sev != "crit" and (quiet or self.settings.critical_only):
                continue
            k.update({"awayMs": None, "lastPageMs": now})
            mid = self.queue(ev, "repeat", sev, "Still flapping: %s" % (k.get("title") or ev.get("title")), now)
            out.append({"rule": k["rule"], "entity": k["entity"], "kind": "repeat", "severity": sev, "outcome": "sent",
                        "reason": "Still away after %s" % span(FLAP_MS), "messageId": mid})
        return out

    def digest_if_quiet_ended(self, now):
        """When quiet hours end: one message naming the warnings held during them that are
        still firing ("While quiet: 2 warnings still firing"); they count as paged from then on
        (notify.rs digest_if_quiet_ended)."""
        quiet = self.settings.quiet_at(now)
        ended = self.quiet_was and not quiet
        self.quiet_was = quiet
        if not ended or self.settings.critical_only:
            return []
        items, out = [], []
        for k in self.keys.values():
            title, k["held"] = k.get("held"), None
            if not title or not firing(k) or k.get("severity") != "warn":
                continue
            ev = dict(k.get("ev") or {}, rule=k["rule"], entity=k["entity"])
            if self.silenced(ev) or self.planned(ev, now):
                continue
            k.update({"paged": True, "lastPageMs": now, "title": title,
                      "pagedSeverity": k.get("pagedSeverity") if RANK.get(k.get("pagedSeverity"), 0) > RANK["warn"] else "warn"})
            items.append((k, ev, title))
        if not items:
            return []
        mid = new_id()
        titles = [t for _, _, t in items]
        lines = titles[:GROUP_LINES] + (["and %d more" % (len(titles) - GROUP_LINES)] if len(titles) > GROUP_LINES else [])
        n = len(items)
        self.digest = {"title": "While quiet: %d warning%s still firing" % (n, "" if n == 1 else "s"), "body": "\n".join(lines),
                       "lines": lines, "severity": "warn", "state": "firing", "node": None, "rule": None, "alertId": None,
                       "messageId": mid, "parts": [ev.get("alertId") or key(k) for k, ev, _ in items], "tsMs": now}
        for k, _, _ in items:
            out.append({"rule": k["rule"], "entity": k["entity"], "kind": "digest", "severity": "warn", "outcome": "sent",
                        "reason": "Quiet hours ended: still firing", "messageId": mid})
        return out

    def flush(self, now):
        """The messages whose window is over: one per incident, or a single grouped one when
        there are more than two."""
        due = [g for g in self.pending if g["dueMs"] <= now]
        digest, self.digest = ([self.digest] if self.digest else []), None
        if not due:
            return digest
        self.pending = [g for g in self.pending if g["dueMs"] > now]
        messages = [self.incident_message(g, now) for g in due]
        if len(messages) <= GROUP_OVER:
            return messages + digest
        order = sorted(messages, key=lambda m: 2 if m["state"] == "resolved" else -RANK.get(m["severity"], 0))
        crit = sum(1 for m in order if m["state"] != "resolved" and m["severity"] == "crit")
        cleared = sum(1 for m in order if m["state"] == "resolved")
        title = "%d alerts" % len(order) + (", %d critical" % crit if crit else "") + (", %d cleared" % cleared if cleared else "")
        titles = [m["title"] for m in order]
        keep = max(GROUP_LINES, crit)
        lines = titles[:keep] + (["and %d more" % (len(titles) - keep)] if len(titles) > keep else [])
        # Every event in these groups was told its group's id: the message keeps the first
        # one and lists the others.
        return [{"title": title, "body": "\n".join(lines), "lines": lines, "severity": "crit" if crit else "warn",
                 "state": "firing" if cleared < len(order) else "resolved", "node": None, "rule": None, "alertId": None,
                 "messageId": order[0]["messageId"], "merged": [m["messageId"] for m in order[1:]],
                 "parts": [p for m in order for p in m["parts"]], "kinds": [k for m in order for k in m["kinds"]], "tsMs": now}] + digest

    def incident_message(self, group, now):
        items = sorted(group["items"], key=lambda i: -RANK.get(i["severity"], 0))
        lead = items[0]
        inc = self.incidents.get(group["incident"]) or {}
        lines = [i["title"] for i in items[1:]] + ["Also: %s" % t for t in inc.pop("also", [])]
        body = "\n".join(([lead["body"]] if lead["body"] else []) + lines)
        return {"title": lead["title"], "body": body, "lines": lines, "severity": lead["severity"],
                "state": "resolved" if all(i["kind"] == "resolved" for i in items) else "firing",
                "node": lead["node"], "rule": lead["rule"], "alertId": lead["alertId"], "incident": group["incident"],
                "messageId": group["messageId"], "parts": [i["alertId"] or "%s|%s" % (i["rule"], i["entity"]) for i in items],
                "kinds": [i["kind"] for i in items], "tsMs": now}


def firing(k):
    """K is firing (not a resolve waiting out its 10 min clear)."""
    return k.get("clearedMs") is None


def span(ms):
    """"30 s", "4 min", "2 h" (notify.rs fmt_span)."""
    s = ms // 1000
    if s < 90:
        return "%d s" % s
    if s < 90 * 60:
        return "%d min" % ((s + 30) // 60)
    return "%d h" % ((s + 1800) // 3600)


def slim(ev):
    """What K keeps of its latest event, for repeats (small enough for state.json)."""
    return {f: ev.get(f) for f in ("severity", "incident", "title", "body", "label", "category", "node", "alertId")}
