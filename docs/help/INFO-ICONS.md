# SparkBox Help: where the info icons go

This is the complete map of info icons. Every window, tab and Settings section has one, and so does each field a beginner wouldn't understand. The owner's rule: **open anything in SparkBox, and an info icon can tell you all about it.**

Line numbers in this file are hints from the tree on 2026-10-30. Find a place by its label.

## How to read the tables

| Column | Meaning |
|---|---|
| **File** | The source file. Each file belongs to one writer package, shown in the section heading |
| **Place** | Where the icon goes, by its visible label |
| **Do** | `add`, `swap` or `link` (below) |
| **Topic** | What the icon opens: an article id (`load-a-model-on-one-node`) or a tip id (`models.load.gpu-share`). A row with an article heading (`node-window#gpu`) is waiting for a tip of its own: `src/help/icons.test.ts` lists those, and refuses new ones |

Topics with a dot are entries in `docs/help/tips.json`, under the owning package's prefix. The writer adds that entry. Every tip names the article it leads to, so **Read the full guide** always lands on the right heading. See `STYLE.md`, section 12.

### The three actions

SparkBox already has about 80 small info popovers: 57 `InfoTip`s and about 25 `SectionTitle info=` props. One thing gets one icon, so these become part of the new system instead of sitting beside it.

| Do | Meaning |
|---|---|
| `add` | Nothing there today. Put `<HelpTip topic="…"/>` right after the label |
| `swap` | An existing popover with fixed text. Replace it with `<HelpTip topic="…"/>`. Move its text to `tips.json`, rewritten to `STYLE.md` where needed. Keep every true fact: a longer one goes into the article |
| `link` | An existing popover whose text uses live values (a node's name, a limit from code). Keep it as it is, and add `topic="…"`, so it also shows the How-To links and **Read the full guide** |

Adding icons is the only code change a writer may make. Never change behavior. A row whose label no longer exists is a defect in this file: put the icon on the nearest true label and report it.

### Notes for the help-system builder

- **One component, one look.** `InfoTip` in `src/components/ui.tsx` already opens on click, closes on <kbd>Esc</kbd> and on a click away, and keeps inside the window. Build on it: `HelpTip` is `InfoTip` with `topic`. `InfoTip` gains an optional `topic`, so that `link` rows work. With a topic it adds the How-To links and **Read the full guide** below its text. With no children it reads its title and body from the topic.
- `SectionTitle` gains an `infoTopic` prop for the same reason.
- **Title bars:** the `help` field in `src/windows/registry.tsx`, from `windowHelp` in `manifest.json`.
- **Settings headings:** the `help` field in `src/windows/settings/index.ts`, from `settingsHelp` in `manifest.json`.
- A topic that names no tip resolves to the article's `summary` as the body.
- `HelpTip` inside a table header or a `FormRow` label must not sort the column or focus the field when clicked.

---

## 1. Window title bars

Every inner window gets a title-bar info button from the `help` field in `src/windows/registry.tsx`. The help-system builder adds these. The map is `windowHelp` in `manifest.json`.

| Window kind | Title | Topic |
|---|---|---|
| `dashboard` | Dashboard | `dashboard` |
| `node` | cell1, cell2… | `node-window` |
| `containers` | Containers | `containers` |
| `topology` | Topology | `topology` |
| `switch` | CRS804 DDQ, Firewalla… | `switch-window`. The Firewalla devices show `firewalla-view`, from `helpFor` in `registry.tsx` |
| `chat` | Chat | `chat` |
| `server` | Server | `server` |
| `providers` | Providers | `providers` |
| `vpn` | VPN | `vpn` |
| `models` | Models | `models-window` |
| `terminal` | Terminal | `terminal` |
| `rdp` | RDP | `rdp` |
| `files` | Files | `files` |
| `workloads` | Workloads | `workloads` |
| `services` | Services | `services` |
| `fleet` | Fleet Command | `fleet-command` |
| `activity` | Activity log | `activity-log` |
| `inspect` | (container) on (node) | `inspect-window` |
| `settings` | Settings | `settings-window` |
| `planned` | Planned feature | `whats-coming` |
| `tasks` | Tasks | `tasks` |
| `updates` | Updates | `updates` |
| `connect` | Connect my Sparks | `connect-my-sparks` |
| `alerts` | Alerts | `alerts-window` |
| `cluster-metrics` | Cluster metrics | `cluster-metrics` |
| `editor` | (file) on (place) | `editor` |
| `endpoints` | Endpoints | `endpoints` |
| `ollama` | Ollama | `ollama` |
| `fabric` | Fabric | `fabric` |
| `images` | Images | `images` |
| `help` | Help | `using-help` |

## 2. Settings section headings

Every Settings section gets an info button beside its heading from the `help` field in `src/windows/settings/index.ts`. The help-system builder adds these. The map is `settingsHelp` in `manifest.json`.

| Section id | Label | Topic |
|---|---|---|
| `general` | General | `settings-general` |
| `appearance` | Appearance | `settings-appearance` |
| `login` | Cluster login | `settings-cluster-login` |
| `nodes` | Nodes | `settings-nodes` |
| `switches` | Switches & gateway | `settings-switches` |
| `data` | Data source | `settings-data-source` |
| `scenarios` | Demo scenarios | `settings-demo-scenarios` |
| `notifications` | Notifications | `settings-notifications` |
| `alerts` | Alerts | `settings-alerts` |
| `terminal` | Terminal | `settings-terminal` |
| `models` | Models | `settings-models` |
| `security` | Security & privacy | `settings-security` |
| `fence` | Fence | `settings-fence` |
| `diagnostics` | Diagnostics | `settings-diagnostics` |

## 3. Places with no icon, on purpose

These stay clear. Their full explanation is one click away in Help, and an icon on each row or chip would be noise.

| Place | Why | Explained in |
|---|---|---|
| Sidebar rows | Each opens a window that has its own button | `sparkbox-window` |
| Top bar and status bar chips | They have tooltips, and clicking opens the right window | `top-bar-and-status-bar` |
| The command palette | It's search | `command-palette` |
| Confirm dialogs | The reader must focus on the decision. The dialog says what happens | The How-To for that action |
| Table body rows | The column header or the window's button covers them | That window's feature guide |
| Launcher and Welcome | They're the way in. Welcome links to Getting Started | `sparkbox-window` |

---

## 4. W-START: connection, safety and access settings

Prefix `start.`

| File | Place | Do | Topic |
|---|---|---|---|
| `src/windows/ConnectWizard.tsx` | Step 1 heading, **The account SparkBox logs in with** | add | `start.connect.account` |
| | Group **Cluster account**, "Where the password goes" | swap | `start.connect.password` |
| | Step 2 heading, **Your Sparks** | add | `start.connect.nodes` |
| | Column **Address (IP)** | add | `start.nodes.ip-fallback` |
| | Column **Hostname** | add | `start.nodes.hostname` |
| | Column **SSH port** | add | `start.nodes.ssh-port` |
| | Step 3 heading, **Checking each node** | add | `check-your-nodes` |
| | Step 4 heading, **Reaching the cluster from anywhere** | add | `start.connect.vpn` |
| | Step 5 heading, **Switches and the gateway** | add | `add-switches-and-gateway` |
| | Row **REST user** | add | `start.switches.rest-user` |
| | Row **REST password** | add | `start.switches.rest-password` |
| | Step 6 heading, **Ready** or **Almost there** | add | `connect-your-cluster` |
| `src/windows/settings/LoginSection.tsx` | Group **Account on every node**, "About the cluster password" | swap | `start.login.password` |
| | Group **How SparkBox logs in**, "About the login order" | swap | `start.login.order` |
| | Row **SSH agent** | add | `start.login.ssh-agent` |
| | Row **Key file** | add | `start.login.key-file` |
| | Row **Key passphrase** | add | `start.login.passphrase` |
| | Row **~/.ssh/config** | add | `start.login.ssh-config` |
| | Row **Password** (fall back to the cluster password) | add | `start.login.password-fallback` |
| | Group **Each node** | add | `start.login.each-node` |
| | **Set up key login** group heading | add | `set-up-key-login` |
| | **Set up key login**, row **Password login** | add | `start.login.password-off` |
| `src/windows/settings/NodesSection.tsx` | Group **Inventory**, "About the inventory" | swap | `start.nodes.inventory` |
| | Column **Hostname** | add | `start.nodes.hostname` |
| | Column **IP address** | add | `start.nodes.ip-fallback` |
| | Column **SSH port** | add | `start.nodes.ssh-port` |
| | Label **Wake: On** under a node | add | `start.nodes.wake` |
| | Button **Test** (first row) | add | `start.nodes.test` |
| `src/windows/settings/SwitchesSection.tsx` | Row **Address** (first group) | add | `start.switches.address` |
| | Row **REST user** | add | `start.switches.rest-user` |
| | Row **REST password** | add | `start.switches.rest-password` |
| | Row **Connection** (first group) | add | `start.switches.test` |
| | Row **MSP domain** | add | `start.switches.msp-domain` |
| | Row **API token** | add | `start.switches.msp-token` |
| | Row **Connection** (Firewalla cloud) | add | `start.switches.msp-test` |
| `src/windows/settings/DataSourceSection.tsx` | Group **Needed for Live**, row **Cluster password** | add | `save-the-cluster-password` |
| `src/windows/settings/SecuritySection.tsx` | Group **Look only**, "About look-only mode" | swap | `start.security.look-only` |
| | Group **Admin mode**, "About Admin mode" | swap | `start.security.admin` |
| | Row **Lock after** | add | `start.security.lock-after` |
| | Row **Changes on this Mac** | add | `start.security.mac-changes` |
| | Row **Asking who you are** | add | `start.security.asking` |
| | Group **SSH tunnels**, "About SSH tunnels" | swap | `start.security.tunnels` |
| | Group **Activity log**, "About the activity log" | swap | `start.security.activity-log` |
| | Row **Keep entries for** | add | `start.security.retention` |
| | Group **Diagnostics**, "About diagnostics" | swap | `start.security.diagnostics` |
| `src/windows/settings/FenceSection.tsx` | Group **Fence**, "About the fence" | swap | `start.fence.about` |
| | Row **State** | add | `start.fence.state` |
| | Row **Key** and **Trusted here** (one icon, on **Key**) | add | `start.fence.key` |
| | Group **Open a fence** (release app) | add | `start.fence.open` |
| | Row **Hours** | add | `start.fence.hours` |
| | The class rows (one icon, on the first) | add | `start.fence.classes` |
| | Group **Controller lease** | add | `leases-and-identity#controller-lease` |
| | Group **Identity** | add | `leases-and-identity#identity-pin` |

`FenceSection.tsx` changes in the foundation step. Take labels from the file as it stands then.

## 5. W-DESK: the desktop, Terminal and Files

Prefix `desk.`

| File | Place | Do | Topic |
|---|---|---|---|
| `src/windows/TerminalWindow.tsx` | Toolbar, after **Broadcast** | add | `desk.terminal.broadcast` |
| `src/windows/terminal/QuickConnect.tsx` | Heading of the connect list | add | `open-a-terminal` |
| `src/windows/terminal/LockedBanner.tsx` | After the banner's text | add | `desk.terminal.locked` |
| `src/windows/FilesWindow.tsx` | Toolbar, after **Move to Trash** | add | `delete-and-restore-files` |
| | Toolbar, after **Hidden files** | add | `desk.files.hidden` |
| `src/windows/files/TransferQueue.tsx` | Heading **Transfers** | add | `desk.files.transfers` |
| `src/windows/files/ConflictSheet.tsx` | Dialog title | add | `desk.files.conflict` |
| `src/windows/EditorWindow.tsx` | Toolbar, after **Revert** | add | `desk.editor.versions` |
| | The read-only bar | add | `desk.editor.read-only` |
| | The **Review changes** dialog title | add | `desk.editor.review` |
| `src/windows/settings/GeneralSection.tsx` | Row **Keep running in the menu bar** | add | `desk.general.menu-bar` |
| | Row **Start at login** | add | `desk.general.login` |
| | Row **Show upcoming features** | add | `desk.general.upcoming` |
| `src/windows/settings/AppearanceSection.tsx` | Row **Theme** | add | `desk.appearance.theme` |
| | Row **Reduce motion** | add | `desk.appearance.motion` |
| | Row **Increase contrast** | add | `desk.appearance.contrast` |
| `src/windows/settings/ScenariosSection.tsx` | Group **Scenario**, "About scenarios" | swap | `desk.scenarios.about` |
| `src/windows/settings/TerminalSection.tsx` | Group **Shells**, "About idle shells" | swap | `desk.terminal.idle` |
| | Group **Look**, "About these settings" | swap | `desk.terminal.look` |
| | Row **Scrollback** | add | `desk.terminal.scrollback` |
| | Row **Copy on select** | add | `desk.terminal.copy-on-select` |
| | Group **Keyboard**, "About the Option key" | link | `desk.terminal.option` |

## 6. W-MONITOR: monitoring, alerts and network

Prefix `mon.`

### Overview windows

| File | Place | Do | Topic |
|---|---|---|---|
| `src/windows/Dashboard.tsx` | Nodes table, at its right end | add | `mon.dash.nodes` |
| | Section **Serving** (`info=`) | swap | `mon.dash.serving` |
| | Tile **Capacity**, "About capacity" | swap | `mon.dash.capacity` |
| | Tile **GPUs serving** | add | `mon.dash.gpus-serving` |
| | Tile **Fabric ↓ / ↑** | add | `mon.dash.fabric` |
| | "About this chart" | swap | `mon.dash.chart` |
| `src/windows/Topology.tsx` | "About the diagram" | swap | `mon.topology.diagram` |
| | After the color choice (**Health**, **GPU load**, **GPU temp**, **Memory**) | add | `mon.topology.color-by` |
| `src/windows/ClusterMetrics.tsx` | After the view choice (**Charts**, **GPU heatmap**) | add | `mon.metrics.views` |
| | Section **Serving** (`info=`) | swap | `mon.metrics.serving` |

### Node window: one per tab

| File | Place | Do | Topic |
|---|---|---|---|
| `src/windows/NodeDetail.tsx` | Chart bar, "About these charts" | swap | `mon.node.charts` |
| | Toolbar, after **DGX Dashboard** | add | `mon.node.dgx-dashboard` |
| | Toolbar, after **Maintenance** | add | `mon.node.maintenance` |
| `src/windows/node/Overview.tsx` | **Overview** tab, first chart title | add | `mon.node.overview` |
| `src/windows/node/HardwareTabs.tsx` | **CPU** tab, chart title | add | `mon.node.cpu` |
| | **Memory** tab, section **How the pool is split** (`info=`) | swap | `mon.node.memory` |
| | **Disks** tab, section **Disks** (`info=`) | swap | `mon.node.disks` |
| `src/windows/node/GpuTab.tsx` | **GPU** tab, section **Processes on the GPU** (`info=`) | swap | `mon.node.gpu` |
| `src/windows/node/NetworkTab.tsx` | **Network** tab, section **Fabric health** (`info=`) | swap | `mon.node.fabric-health` |
| | Each port section (`info=`) | swap | `mon.node.port` |
| `src/windows/node/SystemTab.tsx` | **System** tab, section **Boots** (`info=`) | swap | `mon.node.boots` |
| | Section **Watching** | add | `mon.node.watching` |

### Alerts: one per tab

| File | Place | Do | Topic |
|---|---|---|---|
| `src/windows/AlertsWindow.tsx` | **Active** tab, group **Firing** | swap | `mon.alerts.firing` |
| | Group **Recovering** | swap | `mon.alerts.recovering` |
| | Group **Planned** | swap | `mon.alerts.planned` |
| | Group **Acknowledged** | swap | `mon.alerts.acknowledged` |
| | Group **Information** | swap | `mon.alerts.information` |
| | **Silenced** tab, "About silences" | swap | `mon.alerts.silenced` |
| | **History** tab, toolbar | add | `mon.alerts.history` |
| | **Events** tab, "About kernel events" | swap | `mon.alerts.events` |
| | **Deliveries** tab, "About deliveries" | swap | `mon.alerts.deliveries` |
| | Toolbar, after **Acknowledge** | add | `mon.alerts.acknowledge` |
| | Toolbar, after **Silence ▾** | add | `mon.alerts.silence` |
| `src/windows/settings/NotificationsSection.tsx` | Group **Paging** | swap | `mon.notify.paging` |
| | Group **Where alerts go** (`info=`) | swap | `mon.notify.channels` |
| | **ntfy** channel form heading | add | `mon.notify.ntfy` |
| | **Pushover** channel form heading | add | `mon.notify.pushover` |
| | **Webhook** channel form heading | add | `mon.notify.webhook` |
| | **Email** channel form heading | add | `mon.notify.email` |
| | Switch **Send from this Mac only (not copied to the nodes)** | add | `mon.notify.mac-only` |
| | "About the dead-man's switch" | swap | `mon.notify.deadman` |
| | Row **Critical alerts only** | add | `mon.notify.critical-only` |
| | Row **Repeat critical alerts** | add | `mon.notify.repeat` |
| | Row **Quiet hours** | add | `mon.notify.quiet` |
| | Group **Always-on monitoring** (`info=`) | swap | `mon.notify.always-on` |
| | "About the metrics endpoint" | swap | `mon.notify.metrics` |
| `src/windows/settings/AlertRulesSection.tsx` | Section **Alert rules** (`info=`) | swap | `mon.rules.about` |
| | Header **For**, in the first area's table | add | `mon.rules.for` |
| | Beside **Would fire now** | add | `mon.rules.would-fire` |
| | Button **Per node…** (first row) | add | `mon.rules.per-node` |
| | Area heading **Nodes** | add | `mon.rules.nodes` |
| | Area heading **Disk** | add | `mon.rules.disk` |
| | Area heading **GPU** | add | `mon.rules.gpu` |
| | Area heading **Fabric** | add | `mon.rules.fabric` |
| | Button **13 counters** | add | `mon.rules.counters` |
| | Area heading **Containers & services** | add | `mon.rules.containers` |
| | Area heading **Serving** | add | `mon.rules.serving` |

Each rule's name has its own icon too. Its topic is built from the rule's id, `mon.rule.<id>`, so no row lists them. `src/help/icons.test.ts` holds every rule to a tip and an entry in `alert-rules-reference`.

### Network windows

| File | Place | Do | Topic |
|---|---|---|---|
| `src/windows/FabricWindow.tsx` | **Matrix** tab, "About the fabric verdict" | link | `mon.fabric.verdict` |
| | A cell's panel, each counter | link | `mon.fabric.counter` |
| | **History** tab, toolbar | add | `mon.fabric.history` |
| | Section **NCCL transport** (`info=`) | swap | `mon.fabric.nccl` |
| | Section **Cabling** (`info=`) | swap | `mon.fabric.cabling` |
| `src/windows/fabric/FabricDoctor.tsx` | **Doctor** tab, toolbar | add | `mon.fabric.doctor` |
| | Check **MTU audit** | swap | `mon.fabric.check-mtu` |
| | Check **Jumbo-frame ping** | swap | `mon.fabric.check-ping` |
| | Check **Bandwidth (ib_write_bw)** | swap | `mon.fabric.check-bandwidth` |
| | Check **Latency (ib_send_lat)** | swap | `mon.fabric.check-latency` |
| | Check **All-reduce (nccl-tests)** | swap | `mon.fabric.check-allreduce` |
| | "About the expected bandwidth" | swap | `mon.fabric.expected` |
| | Each half's section (`info=`) | swap | `mon.fabric.results` |
| | Section **Both halves** (`info=`) | swap | `mon.fabric.both-halves` |
| | Button **Clear baselines…** | add | `mon.fabric.baselines` |
| `src/windows/fabric/RocePanel.tsx` | Panel heading | add | `fabric-and-rdma` |
| `src/windows/switch/RouterOsView.tsx` | **Overview** tab, heading area | add | `mon.switch.overview` |
| | **Ports** tab, toolbar | add | `mon.switch.ports` |
| | Toolbar, after **Disable…** | add | `mon.switch.disable` |
| | Toolbar, after **Bounce…** | add | `mon.switch.bounce` |
| | **Optics** tab, "About receive power" | swap | `mon.switch.rx-power` |
| `src/windows/switch/NeighborsTab.tsx` | **Neighbors** tab, "About these tables" | swap | `mon.switch.tables` |
| | Section **Bridge hosts** (`info=`) | swap | `mon.switch.bridge-hosts` |
| | Section **Neighbors** (`info=`) | swap | `mon.switch.neighbors` |
| `src/windows/switch/PortDrawer.tsx` | "Why this port is guarded" | link | `mon.switch.guarded` |
| | "About receive power" | swap | `mon.switch.rx-power` |
| | Section **Switch says / node says** | add | `mon.switch.both-sides` |
| `src/windows/switch/FirewallaView.tsx` | Heading area | add | `firewalla-view` |
| | "Why ports are managed there" (the Gold) | swap | `mon.switch.firewalla-ports` |
| | The port panel, under it | add | `mon.switch.x-panel` |
| | "Where each fact comes from", under the legend | add | `mon.switch.x-cloud` |
| | Section **LAN health, from each node's side** | add | `mon.switch.lan-health` |
| `src/windows/switch/SwitchXPanel.tsx` | Legend, "What the lights mean" | add | `mon.switch.x-light` |
| `src/windows/switch/SwitchXDrawer.tsx` | Section **Heard through this port** | add | `mon.switch.x-heard` |
| | Section **Your label** | add | `mon.switch.x-label` |
| `src/windows/VpnWindow.tsx` | Card, "About this connection" | link | `mon.vpn.connection` |
| | Section **Reaching the cluster** (`info=`) | swap | `mon.vpn.reaching` |
| | Section **When to use the tunnel** (`info=`) | swap | `mon.vpn.when` |
| | Switch **Direct at home** | add | `mon.vpn.direct-at-home` |
| | Section **Expose to this Mac** (`info=`) | swap | `mon.vpn.expose` |
| | Toolbar, after **Screenshot-safe** | add | `mon.vpn.screenshot-safe` |
| | "About the VPN profile" | swap | `mon.vpn.profile` |

## 7. W-MODELS: models

Prefix `models.`

### The Load page

Every field here gets an icon. This is the page a beginner fears most.

| File | Place | Do | Topic |
|---|---|---|---|
| `src/windows/models/LoadPage.tsx` | Group **Model**, row **Engine** | add | `models.load.engine` |
| | Row **Folder** | add | `models.load.folder` |
| | Row **Recipe** | add | `use-a-recipe-for-a-load` |
| | Row **Name in the API** | add | `models.load.api-name` |
| | Group **Nodes**, "About nodes" | link | `models.load.nodes` |
| | Group **Memory plan**, "About the memory plan" | swap | `models.load.memory-plan` |
| | Checkbox **Override memory guardrail** | add | `models.load.override` |
| | Group **Split and limits** | add | `splitting-a-model` |
| | Row **Tensor × pipeline parallel** | add | `models.load.tensor-pipeline` |
| | Row **Tensor parallel** (SGLang) | add | `models.load.tensor-pipeline` |
| | Row **Expert parallel** | add | `models.load.expert-parallel` |
| | Row **Max context** | add | `models.load.max-context` |
| | Row **GPU memory share** | add | `models.load.gpu-share` |
| | Group **Capabilities**, row **Tool calling** | add | `models.load.tool-calling` |
| | Row **Reasoning parser** | add | `models.load.reasoning` |
| | Row **Prefix caching** | add | `models.load.prefix-caching` |
| | Row **KV cache type** | add | `models.load.kv-cache-type` |
| | Row **Requests at once** | add | `models.load.requests-at-once` |
| | Group **Access**, row **Port** | add | `models.load.port` |
| | Row **Listen on** | add | `models.load.listen-on` |
| | Row **API key** | add | `models.load.api-key` |
| | Row **Keep running** | add | `models.load.keep-running` |
| | Row **Allow remote code** | add | `models.load.remote-code` |
| | Row **Expert: raw vLLM args** | add | `models.load.extra-options` |
| | Row **Image** | add | `models.load.image` |
| | Group **Before it starts** | add | `models.load.before-it-starts` |
| | Row **What vLLM needs** | add | `models.load.what-vllm-needs` |
| | Row **Page cache** (the **Drop the page cache first** checkbox) | add | `models.load.drop-page-cache` |
| | Row **earlyoom** | add | `models.load.earlyoom` |
| | Row **/dev/shm** | add | `models.load.dev-shm` |
| | Row **Cache folders** | add | `models.load.cache-folders` |
| | Row **Left by an earlier load** | add | `clear-what-an-earlier-load-left-behind` |
| | Group **What will run**, "About what will run" | swap | `models.load.what-will-run` |

### The tabs

| File | Place | Do | Topic |
|---|---|---|---|
| `src/windows/models/LibraryTab.tsx` | **Library** tab toolbar | add | `models-library` |
| | Toolbar, after **Find models** | add | `models.library.find-models` |
| | Footer, "About copies" | swap | `models.library.copies` |
| | Beside the node columns (first node header) | add | `models.library.verified-found` |
| `src/windows/models/HubTab.tsx` | **Hugging Face** tab toolbar | add | `models-hugging-face` |
| | Header **Weights** | add | `models.hub.weights` |
| | Header **Fits on** | add | `models.hub.fits-on` |
| | Heading **Recommended for GB10** | add | `models.hub.recommended` |
| | After **Check with AI** | add | `models.hub.check-with-ai` |
| | After the badge **gated** | add | `models.hub.gated` |
| | After the badge **MLX · Apple only** | add | `formats-and-quantization#mlx` |
| `src/windows/models/FitTable.tsx` | Section **Fit on 4 Sparks** (`info=`, "About the fit table") | link | `models-hugging-face#fit-table` |
| | Choice **KV bf16** / **KV fp8** | add | `models.fit.kv-type` |
| `src/windows/models/LoadedTab.tsx` | **Loaded** tab toolbar | add | `models-loaded` |
| | The **Failed** row's **Explain…** | add | `fix-a-load-that-failed` |
| | Button **Forget** | add | `models.loaded.forget` |
| `src/windows/models/DeploymentSummary.tsx` | After **Failed** | add | `fix-a-load-that-failed` |
| `src/windows/models/BenchTab.tsx` | **Benchmarks** tab, "About the sweep" | link | `models-benchmarks` |
| | "About runs" | link | `models.bench.runs` |
| | Row **Profile** | add | `models.bench.profile` |
| | Row **Requests at once** | add | `models.bench.requests` |
| `src/windows/models/JobsTab.tsx` | **Jobs** tab, "About jobs" | swap | `models.jobs.about` |
| `src/windows/models/PrepareTab.tsx` | **Prepare cluster** tab toolbar | add | `models-prepare-cluster` |
| | Header **vLLM image** | add | `models.prepare.image` |
| | Header **Fabric (ConnectX-7)** | add | `models.prepare.fabric` |
| `src/windows/settings/ModelsSection.tsx` | Group **Where models live**, "About the model folder" | link | `models.settings.folder` |
| | Group **Engines**, "About the engine images" | swap | `models.settings.images` |
| | Group **Recipes**, "About recipes" | add | `use-a-recipe-for-a-load` |
| | Group **Hugging Face**, "About the Hugging Face token" | swap | `get-a-hugging-face-token` |
| | Group **AI feasibility check**, "About the AI feasibility check" | swap | `models.settings.advisor` |

## 8. W-AI: endpoints, Ollama, Chat, Server and Providers

Prefix `ai.`

| File | Place | Do | Topic |
|---|---|---|---|
| `src/windows/EndpointsWindow.tsx` | Tile **Ready** | add | `ai.endpoints.ready` |
| | Tile **Generating** | add | `ai.endpoints.generating` |
| | Tile **Requests** | add | `ai.endpoints.requests` |
| `src/windows/models/EndpointCard.tsx` | Cell **Generate** | add | `ai.card.generate` |
| | Cell **Prompt** | add | `ai.card.prompt` |
| | Cell **Running · waiting** | add | `ai.card.running` |
| | Cell **KV cache** | add | `ai.card.kv-cache` |
| | Cell **TTFT p50 · p95** | add | `ai.card.ttft` |
| | Cell **ITL p50** | add | `ai.card.itl` |
| | The check line under the card ("17+25 → 42") | add | `tell-if-a-model-is-working` |
| `src/windows/models/EndpointHistory.tsx` | "About this history" | swap | `ai.card.history` |
| `src/windows/OllamaWindow.tsx` | Toolbar, after **Refresh** | add | `use-ollama` |
| | Header **In memory** | add | `ai.ollama.in-memory` |
| | Beside **Pull** | add | `ai.ollama.pull` |
| `src/windows/ChatWindow.tsx` | Toolbar, after **Effort** | add | `ai.chat.effort` |
| | Toolbar, after the temperature field | add | `ai.chat.temperature` |
| | Toolbar, after **System** | add | `ai.chat.system-prompt` |
| `src/windows/chat/ModelPicker.tsx` | Picker header | add | `ai.chat.picker` |
| `src/windows/ServerWindow.tsx` | Group **Settings**, "About the server's settings" | swap | `ai.server.settings` |
| | Row **Require a SparkBox key** | add | `ai.server.require-key` |
| | Row **Start with SparkBox** | add | `ai.server.autostart` |
| | Row **Allowed web origins** | add | `ai.server.origins` |
| | Group **Access keys** | add | `create-an-api-key` |
| | Group **Connect your apps** | add | `connect-an-app` |
| | Group **Test the server**, "About the test" | link | `ai.server.test` |
| | Group **Aliases**, "About aliases" | swap | `ai.server.aliases` |
| | Section **Requests** | add | `ai.server.requests` |
| `src/windows/ProvidersWindow.tsx` | Toolbar, after **Scan cluster** | add | `ai.providers.scan` |
| | Heading **On the cluster now** | add | `ai.providers.on-cluster` |
| | Switch **Enabled** (first card) | add | `ai.providers.enabled` |
| | Label **API key** (first card) | add | `ai.providers.api-key` |
| | Label **Default model** (first card) | add | `ai.providers.default-model` |
| | Switch **Chat models only** | add | `ai.providers.chat-only` |
| | Switch **Allow plain HTTP on my network** | add | `ai.providers.plain-http` |
| | Add-a-server form heading | add | `providers#add-another-server` |

## 9. W-OPS: node operations, updates and records

Prefix `ops.`

| File | Place | Do | Topic |
|---|---|---|---|
| `src/windows/Workloads.tsx` | After the view choice, **Processes** | add | `see-and-stop-processes` |
| | After the view choice, **Listening ports** | add | `ops.workloads.ports` |
| | "About this list" | link | `ops.workloads.list` |
| | "About listening ports" | swap | `ops.workloads.ports-detail` |
| | Toolbar, after **Force stop** | add | `ops.workloads.signals` |
| | Toolbar, after **Reveal with sudo** | add | `ops.workloads.reveal` |
| | "About priority" | swap | `ops.workloads.priority` |
| | Header **Runs in** | add | `ops.workloads.runs-in` |
| `src/windows/Containers.tsx` | After the view choice, **List** / **Deployments** | add | `ops.containers.views` |
| | Header **Exit reason**, "About exits" | swap | `ops.containers.exits` |
| | Toolbar, after **Run test container** | add | `ops.containers.test` |
| `src/windows/Images.tsx` | After the choice **All**, **Unused**, **Dangling** | add | `ops.images.show` |
| | "About images" | link | `ops.images.about` |
| `src/windows/InspectWindow.tsx` | Toolbar | add | `inspect-window` |
| `src/windows/Services.tsx` | Toolbar, after **disable** | add | `ops.services.enable-disable` |
| | Toolbar, after the **More** menu of actions | add | `ops.services.more` |
| | "About running on several nodes" | swap | `ops.services.several-nodes` |
| | Heading of the journal | add | `ops.services.journal` |
| `src/windows/FleetCommand.tsx` | After the view choice, **Run** | add | `run-a-command-on-every-node` |
| `src/components/VersionComparison.tsx` | **Version consistency**, "About version consistency" | swap | `ops.fleet.versions` |
| `src/windows/FleetCommand.tsx` | After the choice **Parallel**, **One at a time**, **Canary first** | add | `ops.fleet.how` |
| | Switch **Stop if a node fails** | add | `ops.fleet.stop-on-failure` |
| | Switch **as root** | add | `ops.fleet.as-root` |
| | Field **Time limit** | add | `ops.fleet.time-limit` |
| | Row of snippets | add | `ops.fleet.snippets` |
| `src/windows/UpdatesWindow.tsx` | Header **Pending** | add | `see-pending-updates` |
| | Header **Security** | add | `ops.updates.security` |
| | Header **Needs care** | add | `ops.updates.needs-care` |
| | Header **Restart** | add | `ops.updates.restart` |
| | Header **DGX OS** | add | `ops.updates.dgx-os` |
| | Header **Firmware** | add | `ops.updates.firmware` |
| | Header **Snaps** | add | `ops.updates.snaps` |
| | Header **Held** | add | `ops.updates.held` |
| | Toolbar, after **Check all nodes** | add | `check-for-updates` |
| | Toolbar, after **Simulate** | add | `simulate-an-update` |
| | Toolbar, after **Upgrade security only** | add | `ops.updates.security-only` |
| | Toolbar, after **Hold** | add | `ops.updates.hold` |
| | Toolbar, after **Pin the GPU stack…** | add | `pin-the-gpu-stack` |
| | Section **Still using old libraries (needrestart)** | add | `ops.updates.needrestart` |
| | Section **Deferred by phasing (not installed this time)** | add | `ops.updates.phasing` |
| `src/windows/NodeDetail.tsx` | Toolbar, after **Restart…** and **Shut down…**, or **Wake** | add | `ops.node.power` |
| `src/windows/TasksWindow.tsx` | Toolbar | add | `tasks-and-jobs` |
| `src/windows/ActivityLog.tsx` | After the choice **Changes** / **Everything** | add | `ops.activity.views` |
| | "About the activity log" | swap | `ops.activity.about` |
| | "About Demo entries" | swap | `ops.activity.demo` |
| `src/windows/settings/DiagnosticsSection.tsx` | Group **Doctor report** | add | `collect-diagnostics` |
| | Group **Evidence** | add | `capture-evidence` |
| | Row **Group** | add | `ops.evidence.phase` |
| | Row **Label** | add | `ops.evidence.label` |
| | Group **SSH host keys** | add | `trust-a-host-key` |

## 10. W-REF: reference and shared surfaces

Prefix `ref.`

| File | Place | Do | Topic |
|---|---|---|---|
| `src/windows/Planned.tsx` | Heading **What it will do** | add | `whats-coming` |
| `src/components/ShortcutsHelp.tsx` | Sheet heading **Keyboard shortcuts** | add | `keyboard-shortcuts` |
| `src/components/WindowErrorBoundary.tsx` | After **This window hit an error** | add | `troubleshooting` |

W-REF writes no other tips. Its articles are the targets the others link to.

---

## 11. Coverage check

The critics walk this list against the running app.

- **Windows:** all 29 kinds in `registry.tsx`, plus `help`.
- **Settings sections:** all 14 in `settings/index.ts`.
- **Tabs and views:**
  - Models: **Library**, **Hugging Face**, **Loaded**, **Benchmarks**, **Jobs**, **Prepare cluster**, and the Load page
  - Node: **Overview**, **CPU**, **Memory**, **GPU**, **Disks**, **Network**, **System**
  - Alerts: **Active**, **Silenced**, **History**, **Events**, **Deliveries**
  - Fabric: **Matrix**, **Doctor**, **History**
  - CRS804: **Overview**, **Ports**, **Optics**, **Neighbors**
  - Workloads: **Processes**, **Listening ports**
  - Fleet Command: **Run**, **Versions**
  - Containers: **List**, **Deployments**
  - Activity log: **Changes**, **Everything**
  - Cluster metrics: **Charts**, **GPU heatmap**
  - Settings › Alerts: all seven areas
  - Connect my Sparks: all six steps
- **Fields a beginner needs explained:** at least **GPU memory share**, **Max context**, **Listen on**, **API key**, **Requests at once**, **KV cache type**, **Keep running**, **Allow remote code**, **Drop the page cache first**, **Tensor × pipeline parallel**, **earlyoom**, **/dev/shm**, **Lock after**, **IP address**, **REST user**, **Quiet hours**, **as root**, **Canary first**, **Direct at home** and **Allowed web origins**.

Settings › Nodes › Fabric and load role: `start.nodes.fabric`, linking to `settings-nodes#fabric-and-load-role`.

### RDP

- `src/windows/RdpWindow.tsx`: the toolbar info icon uses `ops.rdp.check` and opens `rdp`, with `check-and-repair-rdp` as its How-To.

## How To library (November 6)

| File | Place | Do | Topic |
|---|---|---|---|
| `src/windows/HowToWindow.tsx` | Library toolbar | add | `ops.howto.library` |
| `src/windows/HowToWindow.tsx` | Let SparkBox help | add | `ops.howto.actions` |
| `src/windows/HowToWindow.tsx` | Ask AI to revise a section | add | `ops.howto.ai` |
