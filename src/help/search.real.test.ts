// Help's search, held to the real articles. The queries are what a person new to all this types,
// so a change to the ranking or to an article's words can't quietly lose them.
import { describe, expect, it } from "vitest";
import { allArticles, searchDocs } from "./content";
import { manifest } from "./meta";
import { buildIndex, landing, search, searchOrNearest, type Hit } from "./search";
import tuned from "./search.tuned.json";

/** A query, and the articles that would answer it. One of them must be in the top three. */
const BATTERY: [string, string[]][] = [
 ["how do i download a model", ["download-a-model"]],
 // The app's own hint sends the reader here by these words (GPU_HINT in src-tauri/src/onboard.rs).
 ["If the GPU driver doesn't answer", ["get-a-gpu-driver-answering"]],
 ["nvidia-smi didn't answer", ["get-a-gpu-driver-answering","errors-connection"]],
 ["downlaod model", ["download-a-model"]],
 ["dowload a modle", ["download-a-model"]],
 ["get a model", ["download-a-model","find-a-model-that-fits"]],
 ["hugging face", ["models-hugging-face","download-a-model","get-a-hugging-face-token"]],
 ["hugginface token", ["get-a-hugging-face-token"]],
 ["huging face login", ["get-a-hugging-face-token"]],
 ["gated model", ["get-a-hugging-face-token","download-a-model"]],
 ["which model should i use", ["find-a-model-that-fits"]],
 ["what model fits my spark", ["find-a-model-that-fits"]],
 ["how do i run a model", ["your-first-model","load-a-model-on-one-node"]],
 ["start a model", ["load-a-model-on-one-node","your-first-model"]],
 ["run a model", ["your-first-model","load-a-model-on-one-node"]],
 ["run model on all four nodes", ["load-a-model-on-several-nodes"]],
 ["run model across cluster", ["load-a-model-on-several-nodes","splitting-a-model"]],
 ["multi node", ["load-a-model-on-several-nodes","splitting-a-model"]],
 ["stop a model", ["unload-a-model"]],
 ["free up gpu memory", ["unload-a-model","memory-on-a-spark"]],
 ["delete model", ["delete-a-model"]],
 ["remove a model from disk", ["delete-a-model"]],
 ["copy model to other nodes", ["copy-a-model-to-other-nodes"]],
 ["model wont load", ["fix-a-load-that-failed","errors-models"]],
 ["load failed", ["fix-a-load-that-failed","errors-models"]],
 ["out of memory", ["fix-a-load-that-failed","memory-on-a-spark","errors-models"]],
 ["is my model working", ["tell-if-a-model-is-working"]],
 ["talk to the model", ["chat-with-a-cluster-model","chat"]],
 ["how do i chat", ["chat-with-a-cluster-model","chat","chat-with-a-cloud-model"]],
 ["tokens per second", ["benchmark-a-model","models-benchmarks","tokens-and-context"]],
 ["benchamrk", ["benchmark-a-model","models-benchmarks"]],
 ["what is a token", ["tokens-and-context","glossary"]],
 ["what is context", ["tokens-and-context"]],
 ["max context", ["choose-load-settings","tokens-and-context"]],
 ["gpu memory share", ["choose-load-settings","memory-on-a-spark"]],
 ["what is quantization", ["formats-and-quantization"]],
 ["quantisation", ["formats-and-quantization"]],
 ["gguf", ["formats-and-quantization","use-ollama"]],
 ["vllm", ["engines"]],
 ["what is an engine", ["engines"]],
 ["ollama", ["ollama","use-ollama"]],
 ["restart a node", ["restart-a-node"]],
 ["reboot", ["restart-a-node"]],
 ["rebot machine", ["restart-a-node"]],
 ["shutdown", ["shut-down-and-wake-a-node"]],
 ["turn off a spark", ["shut-down-and-wake-a-node"]],
 ["power off", ["shut-down-and-wake-a-node"]],
 ["wake on lan", ["shut-down-and-wake-a-node"]],
 ["why did my node restart", ["find-out-why-a-node-restarted"]],
 ["node crashed", ["find-out-why-a-node-restarted"]],
 ["maintanence mode", ["use-maintenance-mode"]],
 ["node offline", ["troubleshooting","errors-connection"]],
 ["cant connect", ["errors-connection","troubleshooting","connect-your-cluster"]],
 ["connect to my cluster", ["connect-your-cluster"]],
 ["add a new node", ["add-or-change-a-node"]],
 ["admin mode", ["unlock-admin-mode","admin-mode"]],
 ["touch id", ["unlock-admin-mode","admin-mode"]],
 ["unlock", ["unlock-admin-mode"]],
 ["what is the fence", ["the-fence","settings-fence"]],
 ["The fence doesn't allow this change.", ["errors-admin-and-safety"]],
 ["No fence is open", ["errors-admin-and-safety"]],
 ["password", ["save-the-cluster-password","keychain-and-secrets","settings-cluster-login"]],
 ["change my password", ["save-the-cluster-password","settings-cluster-login"]],
 ["where is my password stored", ["keychain-and-secrets"]],
 ["ssh key", ["set-up-key-login"]],
 ["host key changed", ["trust-a-host-key","errors-connection"]],
 ["permission denied", ["errors-connection","errors-files-and-terminal"]],
 ["look only", ["use-look-only-mode","admin-mode"]],
 ["demo mode", ["demo-and-live","switch-between-demo-and-live"]],
 ["use real cluster", ["switch-between-demo-and-live","connect-your-cluster","demo-and-live"]],
 ["disk full", ["check-disk-space"]],
 ["how much disk space left", ["check-disk-space"]],
 ["gpu temperature", ["node-window","read-the-dashboard","dashboard"]],
 ["gpu usage", ["node-window","read-the-dashboard","dashboard","read-the-charts"]],
 ["charts", ["read-the-charts","cluster-metrics"]],
 ["update the cluster", ["install-updates"]],
 ["instal updates", ["install-updates"]],
 ["apt upgrade", ["install-updates"]],
 ["nvidia driver", ["pin-the-gpu-stack","packages-and-updates"]],
 ["pin gpu", ["pin-the-gpu-stack"]],
 ["vpn", ["connect-the-vpn","vpn"]],
 ["wireguard", ["connect-the-vpn","vpn"]],
 ["connect from outside my house", ["connect-the-vpn"]],
 ["ssh into a node", ["open-a-terminal","ssh-sudo-and-host-keys"]],
 ["open terminal", ["open-a-terminal"]],
 ["copy files to my mac", ["copy-files"]],
 ["upload a file", ["copy-files"]],
 ["scp", ["copy-files"]],
 ["edit a config file", ["edit-a-file"]],
 ["recover deleted file", ["delete-and-restore-files"]],
 ["trash", ["delete-and-restore-files"]],
 ["read logs", ["services","containers","find-what-sparkbox-did","restart-a-service"]],
 ["kill a process", ["see-and-stop-processes"]],
 ["docker", ["containers-and-images","manage-containers","check-docker-images"]],
 ["restart container", ["manage-containers"]],
 ["restart a service", ["restart-a-service"]],
 ["systemd", ["restart-a-service","services-and-processes","services"]],
 ["run command on all nodes", ["run-a-command-on-every-node"]],
 ["openai key", ["add-a-cloud-provider"]],
 ["use chatgpt", ["add-a-cloud-provider","chat-with-a-cloud-model"]],
 ["anthropic claude", ["add-a-cloud-provider","chat-with-a-cloud-model","whats-coming"]],
 ["claude code", ["whats-coming"]],
 ["api key", ["create-an-api-key"]],
 ["connect open webui", ["connect-an-app"]],
 ["use from my phone", ["use-a-model-from-another-computer"]],
 ["use a model from another computer", ["use-a-model-from-another-computer"]],
 ["use my model from my laptop", ["use-a-model-from-another-computer"]],
 ["where is the api key for a model", ["use-a-model-from-another-computer","endpoints-and-apis"]],
 ["give a model a short name", ["give-a-model-a-short-name"]],
 ["alias", ["give-a-model-a-short-name","server"]],
 ["switch models without changing my app", ["give-a-model-a-short-name"]],
 ["use my cluster from an app", ["connect-an-app","run-sparkbox-as-a-server"]],
 ["alerts on my phone", ["get-alerts-on-your-phone"]],
 ["notificaitons", ["get-notifications-on-this-mac","settings-notifications"]],
 ["silence an alert", ["respond-to-an-alert"]],
 ["change alert threshold", ["change-alert-rules"]],
 ["always on monitoring", ["set-up-always-on-monitoring","always-on-agent"]],
 ["keyboard shortcuts", ["keyboard-shortcuts"]],
 ["back up settings", ["back-up-sparkbox"]],
 ["reset", ["reset-sparkbox"]],
 ["dark mode", ["settings-appearance"]],
 ["change the theme", ["settings-appearance"]],
 ["make text bigger", ["settings-appearance","windows-and-layouts","keyboard-shortcuts"]],
 ["what is rdma", ["fabric-and-rdma"]],
 ["fabric slow", ["test-the-fabric","check-the-fabric","errors-network"]],
 ["jumbo frames mtu", ["check-the-fabric","fabric-and-rdma","fabric-doctor"]],
 ["switch port", ["manage-a-switch-port","see-what-a-port-connects-to"]],
 ["firewalla", ["firewalla-view","add-switches-and-gateway"]],
 ["crs804", ["switch-window"]],
 ["dgx dashboard", ["open-the-dgx-dashboard"]],
 ["evidence", ["capture-evidence"]],
 ["send logs to support", ["collect-diagnostics"]],
 ["how do i use this app", ["getting-started","using-help"]],
 ["what does sparkbox do", ["what-is-sparkbox"]],
 ["im new", ["getting-started"]],
 ["save window layout", ["arrange-your-windows","windows-and-layouts"]],
 ["tile windows", ["arrange-your-windows","windows-and-layouts"]],
 ["command palette", ["command-palette"]],
 // Plain words beyond the first list: the reader's own, not SparkBox's.
 ["download stuck", ["follow-a-download","errors-models"]],
 ["my download is not moving", ["follow-a-download"]],
 ["download is slow", ["follow-a-download"]],
 ["download stopped", ["follow-a-download","errors-models"]],
 ["what model should i use", ["find-a-model-that-fits"]],
 ["best model for my cluster", ["find-a-model-that-fits"]],
 ["recommended models", ["find-a-model-that-fits","models-hugging-face"]],
 ["how much memory does a model need", ["find-a-model-that-fits","memory-on-a-spark"]],
 ["my first model", ["your-first-model"]],
 ["how do i start", ["getting-started"]],
 ["beginner", ["getting-started"]],
 ["set up sparkbox", ["getting-started","connect-your-cluster"]],
 ["try it without a cluster", ["tour-the-demo"]],
 ["node is down", ["troubleshooting","errors-connection"]],
 ["model is not responding", ["tell-if-a-model-is-working","fix-a-load-that-failed"]],
 ["why is my model slow", ["benchmark-a-model","tell-if-a-model-is-working"]],
 ["turn off the model", ["unload-a-model"]],
 ["free disk space", ["check-disk-space","delete-a-model"]],
 ["is it safe to restart", ["restart-a-node"]],
 ["lock sparkbox", ["admin-mode","unlock-admin-mode"]],
 ["what changed on my cluster", ["find-what-sparkbox-did","activity-log"]],
 ["who did this", ["find-what-sparkbox-did","activity-log"]],
 ["phone notification", ["get-alerts-on-your-phone"]],
 ["email alerts", ["get-alerts-on-your-phone","settings-notifications"]],
 ["bigger text", ["settings-appearance","windows-and-layouts","keyboard-shortcuts"]],
 ["light theme", ["settings-appearance"]],
 ["network is slow", ["check-the-fabric","test-the-fabric"]],
 ["how fast is the network", ["check-the-fabric","test-the-fabric"]],
 ["what is a container", ["containers-and-images"]],
 ["what is sudo", ["ssh-sudo-and-host-keys","glossary"]],
 ["run as root", ["run-a-command-on-every-node","read-a-root-only-file","ssh-sudo-and-host-keys"]],
 ["share a model with my apps", ["connect-an-app","run-sparkbox-as-a-server"]],
];

/**
 * Vaguer questions, most of them never used to tune the ranking. They can't all land: this list
 * is a floor, so a change that makes search worse in general fails, whichever queries it hits.
 */
const BROAD: [string, string[]][] = [
 ["how long does a download take", ["follow-a-download"]],
 ["my download failed", ["follow-a-download","errors-models"]],
 ["cancel download", ["follow-a-download"]],
 ["where are my models stored", ["models-library","files-and-folders","settings-models"]],
 ["how do i pick the right model", ["find-a-model-that-fits"]],
 ["how big a model can i run", ["find-a-model-that-fits","memory-on-a-spark","model-questions"]],
 ["run a really big model", ["load-a-model-on-several-nodes"]],
 ["use two nodes for one model", ["load-a-model-on-several-nodes"]],
 ["unload", ["unload-a-model"]],
 ["my model keeps crashing", ["fix-a-load-that-failed"]],
 ["garbage output", ["fix-a-load-that-failed"]],
 ["chat with my model", ["chat-with-a-cluster-model"]],
 ["ask a question", ["chat-with-a-cluster-model","chat"]],
 ["how much does it cost", ["chat-with-a-cloud-model","add-a-cloud-provider"]],
 ["what is a node", ["your-cluster","glossary"]],
 ["what is vllm", ["engines","glossary"]],
 ["what is the difference between demo and live", ["demo-and-live"]],
 ["how do i get out of demo", ["switch-between-demo-and-live"]],
 ["sidebar", ["sparkbox-window","glossary"]],
 ["padlock", ["admin-mode","unlock-admin-mode"]],
 ["why is everything grey", ["unlock-admin-mode","admin-mode"]],
 ["why cant i click this button", ["unlock-admin-mode","admin-mode","troubleshooting"]],
 ["password keeps failing", ["save-the-cluster-password","errors-connection"]],
 ["sudo failed", ["errors-connection","save-the-cluster-password","check-your-nodes"]],
 ["computer wont turn on", ["shut-down-and-wake-a-node","troubleshooting"]],
 ["node wont wake up", ["shut-down-and-wake-a-node"]],
 ["reset everything", ["reset-sparkbox"]],
 ["move to a new mac", ["back-up-sparkbox"]],
 ["red alert", ["respond-to-an-alert","alerts-and-incidents","alerts-window"]],
 ["too many notifications", ["change-alert-rules","get-notifications-on-this-mac"]],
 ["stop getting alerts while i work", ["use-maintenance-mode"]],
 ["monitoring when mac is asleep", ["set-up-always-on-monitoring"]],
 ["cable problem", ["check-the-fabric","see-what-a-port-connects-to"]],
 ["which port is which node plugged into", ["see-what-a-port-connects-to"]],
 ["how fast are my cables", ["test-the-fabric","check-the-fabric"]],
 ["delete a file", ["delete-and-restore-files"]],
 ["open a file on a node", ["files","edit-a-file"]],
 ["log in to a spark", ["open-a-terminal"]],
 ["see what is running", ["see-and-stop-processes","workloads"]],
 ["what is eating my memory", ["see-and-stop-processes","memory-on-a-spark"]],
 ["security updates", ["install-updates","see-pending-updates"]],
 ["will an update break my gpu", ["pin-the-gpu-stack","packages-and-updates"]],
 ["preview an update first", ["simulate-an-update"]],
 ["shortcut for help", ["keyboard-shortcuts"]],
 ["what does ready mean", ["status-words"]],
 ["what is proven", ["whats-proven"]],
 ["what is coming next", ["whats-coming"]],
 ["my cluster is slow", ["troubleshooting","check-the-fabric","benchmark-a-model"]],
 ["nothing is loading", ["troubleshooting","fix-a-load-that-failed"]],
 ["how do i add another spark", ["add-or-change-a-node"]],
 ["remove a node", ["add-or-change-a-node"]],
 ["download finished but model missing", ["follow-a-download","models-library","errors-models"]],
 ["where do downloaded models go", ["download-a-model","files-and-folders","models-library","settings-models"]],
 ["try a small model first", ["your-first-model","use-ollama"]],
 ["what model works best on dgx spark", ["find-a-model-that-fits"]],
 ["is this model too big", ["find-a-model-that-fits","memory-on-a-spark"]],
 ["not enough memory to load", ["fix-a-load-that-failed","memory-on-a-spark","choose-load-settings"]],
 ["how many users can use a model at once", ["choose-load-settings","benchmark-a-model"]],
 ["make my model faster", ["benchmark-a-model","choose-load-settings"]],
 ["copy model to all sparks", ["copy-a-model-to-other-nodes"]],
 ["stop everything", ["unload-a-model","shut-down-and-wake-a-node","use-look-only-mode"]],
 ["log out", ["admin-mode","unlock-admin-mode"]],
 ["lock it again", ["unlock-admin-mode","admin-mode"]],
 ["touch id not working", ["unlock-admin-mode","troubleshooting","errors-admin-and-safety"]],
 ["trust this computer", ["trust-a-host-key"]],
 ["key changed warning", ["trust-a-host-key","errors-connection"]],
 ["update my mac app", ["updates","whats-coming","install-updates"]],
 ["kernel update", ["install-updates","pin-the-gpu-stack","packages-and-updates"]],
 ["restart all nodes", ["restart-a-node","run-a-command-on-every-node"]],
 ["node wont come back after restart", ["restart-a-node","troubleshooting","errors-updates-and-power"]],
 ["see gpu temperature", ["read-the-dashboard","node-window"]],
 ["how hot is my gpu", ["read-the-dashboard","node-window","read-the-charts"]],
 ["disk almost full", ["check-disk-space","respond-to-an-alert"]],
 ["clean up old models", ["delete-a-model","check-disk-space"]],
 ["clean up docker", ["check-docker-images","manage-containers"]],
 ["open a shell", ["open-a-terminal"]],
 ["paste a command", ["open-a-terminal","run-a-command-on-every-node"]],
 ["run same thing on every node", ["run-a-command-on-every-node"]],
 ["transfer files between nodes", ["copy-files"]],
 ["save my notes in a file", ["edit-a-file","files"]],
 ["undo delete", ["delete-and-restore-files"]],
 ["make a new api key", ["create-an-api-key"]],
 ["use with python", ["connect-an-app"]],
 ["connect claude", ["add-a-cloud-provider","connect-an-app","chat-with-a-cloud-model"]],
 ["get notified when something breaks", ["get-notifications-on-this-mac","get-alerts-on-your-phone"]],
 ["text message alerts", ["get-alerts-on-your-phone"]],
 ["alert keeps coming back", ["respond-to-an-alert","change-alert-rules","troubleshooting"]],
 ["turn off an alert", ["respond-to-an-alert","change-alert-rules"]],
 ["reach cluster from a coffee shop", ["connect-the-vpn"]],
 ["vpn wont connect", ["connect-the-vpn","errors-network","vpn"]],
 ["check my cables", ["check-the-fabric","see-what-a-port-connects-to"]],
 ["speed test", ["test-the-fabric"]],
 ["save this arrangement of windows", ["arrange-your-windows"]],
 ["open window on second monitor", ["arrange-your-windows","windows-and-layouts"]],
 ["what are the keyboard keys", ["keyboard-shortcuts"]],
 ["send a bug report", ["collect-diagnostics"]],
 ["start over", ["reset-sparkbox"]],
 ["set up my four sparks", ["connect-your-cluster","connect-my-sparks"]],
 ["first time opening the app", ["getting-started","connect-your-cluster"]],
 ["how to get a llama model", ["download-a-model","use-ollama","find-a-model-that-fits"]],
 ["huggingface", ["models-hugging-face","get-a-hugging-face-token","download-a-model"]],
 ["do i need an account to download", ["get-a-hugging-face-token"]],
 ["is the download done", ["follow-a-download"]],
 ["finish loading", ["load-a-model-on-one-node","tell-if-a-model-is-working"]],
 ["load is stuck", ["fix-a-load-that-failed","load-a-model-on-one-node"]],
 ["why did my load fail", ["fix-a-load-that-failed"]],
 ["red memory bar", ["load-a-model-on-one-node","choose-load-settings","models-load-page"]],
 ["stop serving", ["unload-a-model","models-loaded"]],
 ["remove model", ["delete-a-model"]],
 ["benchmark", ["benchmark-a-model","models-benchmarks"]],
 ["try a prompt", ["chat-with-a-cluster-model","chat"]],
 ["cloud model", ["chat-with-a-cloud-model","add-a-cloud-provider"]],
 ["my api key leaked", ["create-an-api-key"]],
 ["server port", ["run-sparkbox-as-a-server","ports-reference"]],
 ["node name", ["add-or-change-a-node","settings-nodes"]],
 ["ip address changed", ["add-or-change-a-node"]],
 ["wake a sleeping node", ["shut-down-and-wake-a-node"]],
 ["node frozen", ["recover-a-frozen-node"]],
 ["reboot everything", ["restart-a-node"]],
 ["apt", ["install-updates","packages-and-updates"]],
 ["driver update", ["pin-the-gpu-stack","install-updates"]],
 ["check what will change before updating", ["simulate-an-update"]],
 ["gpu memory", ["memory-on-a-spark","choose-load-settings"]],
 ["cpu usage", ["node-window","read-the-dashboard","workloads","read-the-charts"]],
 ["cluster history", ["read-the-charts","cluster-metrics"]],
 ["view logs of a container", ["manage-containers"]],
 ["restart docker", ["manage-containers","restart-a-service"]],
 ["end a process", ["see-and-stop-processes"]],
 ["run as administrator", ["run-a-command-on-every-node","read-a-root-only-file","open-a-terminal"]],
 ["edit netplan", ["edit-a-file"]],
 ["download a file from a node", ["copy-files"]],
 ["alert silenced by accident", ["respond-to-an-alert"]],
 ["set alert limit", ["change-alert-rules"]],
 ["quiet at night", ["get-notifications-on-this-mac"]],
 ["no notifications on my mac", ["get-notifications-on-this-mac"]],
 ["remote login", ["connect-the-vpn","open-a-terminal"]],
 ["fabric test", ["test-the-fabric","fabric-doctor"]],
 ["port status on switch", ["manage-a-switch-port","switch-window","see-what-a-port-connects-to","status-words"]],
 ["firewall", ["firewalla-view","connect-the-vpn"]],
 ["back up", ["back-up-sparkbox"]],
 ["uninstall sparkbox", ["reset-sparkbox"]],
 ["theme", ["settings-appearance"]],
 ["search help", ["using-help"]],
 ["what can sparkbox do", ["what-is-sparkbox"]],
 ["glossary", ["glossary"]],
 ["what is not finished yet", ["whats-coming","whats-proven"]],
 ["explain the dashboard", ["read-the-dashboard","dashboard"]],
 ["what do i do first", ["getting-started"]],
 ["demo tour", ["tour-the-demo"]],
 ["real cluster instead of demo", ["switch-between-demo-and-live","connect-your-cluster"]],
 ["save password", ["save-the-cluster-password"]],
 ["node not found", ["errors-connection","troubleshooting","check-your-nodes"]],
 ["cant reach node", ["errors-connection","troubleshooting"]],
 ["gemma", ["find-a-model-that-fits","use-ollama","download-a-model"]],
 ["license for a model", ["get-a-hugging-face-token","download-a-model"]],
 ["how much space does a model take", ["find-a-model-that-fits","check-disk-space","download-a-model"]],
 ["download to another node", ["download-a-model","copy-a-model-to-other-nodes"]],
 ["model on every spark", ["copy-a-model-to-other-nodes","load-a-model-on-several-nodes"]],
 ["two sparks one model", ["load-a-model-on-several-nodes","splitting-a-model"]],
 ["port already in use", ["errors-models","load-a-model-on-one-node","ports-reference"]],
 ["memory plan", ["models-load-page","load-a-model-on-one-node"]],
 ["context length", ["tokens-and-context","choose-load-settings"]],
 ["requests at once", ["choose-load-settings","models-load-page"]],
 ["model answers gibberish", ["fix-a-load-that-failed","troubleshooting"]],
 ["no reply from model", ["tell-if-a-model-is-working","fix-a-load-that-failed","chat-with-a-cluster-model"]],
 ["switch models", ["restart-or-change-a-model","unload-a-model"]],
 ["open webui", ["connect-an-app"]],
 ["openai compatible api", ["run-sparkbox-as-a-server","endpoints-and-apis","connect-an-app"]],
 ["base url", ["connect-an-app"]],
 ["cost of cloud chat", ["chat-with-a-cloud-model"]],
 ["revoke key", ["create-an-api-key"]],
 ["what uses the gpu", ["see-and-stop-processes","workloads"]],
 ["high temperature alert", ["respond-to-an-alert","alert-rules-reference"]],
 ["acknowledge", ["respond-to-an-alert"]],
 ["ntfy", ["get-alerts-on-your-phone"]],
 ["pushover", ["get-alerts-on-your-phone"]],
 ["agent install", ["set-up-always-on-monitoring"]],
 ["rdma", ["fabric-and-rdma"]],
 ["roce", ["fabric-and-rdma","check-the-fabric"]],
 ["mtu", ["test-the-fabric","check-the-fabric","glossary"]],
 ["which cable goes where", ["see-what-a-port-connects-to","check-the-fabric"]],
 ["amneziawg", ["connect-the-vpn","vpn"]],
 ["wol", ["shut-down-and-wake-a-node"]],
 ["maintenance", ["use-maintenance-mode"]],
 ["pending updates", ["see-pending-updates"]],
 ["dry run", ["simulate-an-update"]],
 ["hold a package", ["pin-the-gpu-stack"]],
 ["restart docker container", ["manage-containers"]],
 ["failed service", ["restart-a-service"]],
 ["sudo in terminal", ["open-a-terminal","read-a-root-only-file"]],
 ["netplan", ["edit-a-file"]],
 ["trash can", ["delete-and-restore-files"]],
 ["command history", ["find-what-sparkbox-did","activity-log","run-a-command-on-every-node"]],
 ["doctor report", ["collect-diagnostics","settings-diagnostics"]],
 ["evidence before and after", ["capture-evidence"]],
 ["palette", ["command-palette"]],
 ["separate window", ["arrange-your-windows","windows-and-layouts"]],
];
const BROAD_FLOOR = 0.95;

/**
 * Every question that has been used to tune search: the critics' lists, and each held-out list
 * after it was read. search.heldout.test.ts holds the ones nobody has read.
 */
const TUNED = tuned as [string, string[]][];
const TUNED_FLOOR = 0.95;

const ix = buildIndex(searchDocs());

function rank(query: string, want: string[]): number {
  return search(ix, query).findIndex((h) => want.includes(h.doc.id));
}

const hitFor = (query: string, id: string): Hit => {
  const h = search(ix, query).find((x) => x.doc.id === id);
  if (!h) throw new Error(`${query}: ${id} not found`);
  return h;
};

describe("beginner queries on the real articles", () => {
  it("every article the battery expects exists", () => {
    const known = new Set(ix.docs.map((d) => d.id));
    expect([...BATTERY, ...BROAD].flatMap(([, want]) => want).filter((id) => !known.has(id))).toEqual([]);
  });

  it("find the right article in the top three, every time", () => {
    const misses = BATTERY.flatMap(([q, want]) => {
      const r = rank(q, want);
      return r >= 0 && r < 3 ? [] : [`#${r + 1} ${q} -> ${search(ix, q).slice(0, 3).map((h) => h.doc.id).join(", ")} (want ${want.join(" or ")})`];
    });
    expect(misses, misses.join("\n")).toEqual([]);
  });

  it("find the right article for most vaguer questions too", () => {
    const hits = BROAD.filter(([q, want]) => {
      const r = rank(q, want);
      return r >= 0 && r < 3;
    });
    expect(hits.length / BROAD.length).toBeGreaterThanOrEqual(BROAD_FLOOR);
  });

  it("find the right article for what has been tuned on, whatever the articles say later", () => {
    const hits = TUNED.filter(([q, want]) => {
      const at = searchOrNearest(ix, q).hits.findIndex((h) => want.includes(h.doc.id));
      return at >= 0 && at < 3;
    });
    expect(hits.length / TUNED.length).toBeGreaterThanOrEqual(TUNED_FLOOR);
  });

  it("name articles that exist in the tuned list", () => {
    const known = new Set(ix.docs.map((d) => d.id));
    expect(TUNED.flatMap(([, want]) => want).filter((id) => !known.has(id))).toEqual([]);
  });

  it("answer a beginner's plainest complaints with the page that helps", () => {
    const top = (q: string, n = 3) => search(ix, q).slice(0, n).map((h) => h.doc.id);
    expect(top("my model is not answering", 1)).toEqual(["tell-if-a-model-is-working"]);
    expect(top("my model is not answering")).toContain("fix-a-load-that-failed");
    expect(top("forgot password", 1)).toEqual(["save-the-cluster-password"]);
    expect(landing(hitFor("forgot password", "save-the-cluster-password"), "forgot password")).toBe("if-you-forgot-the-password");
    expect(top("my spark is not responding").some((id) => ["recover-a-frozen-node", "troubleshooting", "errors-connection", "restart-a-node"].includes(id))).toBe(true);
    expect(top("my spark is not responding", 1)).not.toEqual(["respond-to-an-alert"]);
  });

  it("know the words a reader uses for SparkBox's", () => {
    expect(search(ix, "hard drive full")[0].doc.id).toBe("check-disk-space");
    expect(search(ix, "change colors").slice(0, 3).map((h) => h.doc.id)).toContain("settings-appearance");
    expect(search(ix, "my model is too large").slice(0, 3).map((h) => h.doc.id)).toContain("find-a-model-that-fits");
    expect(search(ix, "adminmode").slice(0, 3).map((h) => h.doc.id)).toContain("admin-mode");
  });

  it("put the How-To above the page of messages for a plain question", () => {
    for (const q of ["how do i run a model", "start a model", "run a model", "which model should i use"]) {
      expect(search(ix, q).slice(0, 3).map((h) => h.doc.id), q).not.toContain("errors-models");
    }
  });
});

describe("words are matched as they are, not as the start of longer ones", () => {
  it("doesn't turn mode into model", () => {
    const top = search(ix, "admin mode").slice(0, 5).map((h) => h.doc.id);
    expect(top).toContain("admin-mode");
    expect(top).not.toContain("models-window");
    expect(top).not.toContain("errors-models");
  });

  it("doesn't turn id into identity", () => {
    const top = search(ix, "touch id").slice(0, 3).map((h) => h.doc.id);
    expect(top).toContain("unlock-admin-mode");
    expect(top[0]).not.toBe("leases-and-identity");
  });

  it("doesn't turn use into user, or app into appearance", () => {
    expect(search(ix, "how do i use this app")[0].doc.id).not.toBe("settings-appearance");
  });

  it("still matches the word being typed", () => {
    expect(search(ix, "downl")[0].doc.id).toBe("download-a-model");
    expect(search(ix, "glos")[0].doc.id).toBe("glossary");
  });
});

describe("a quoted message", () => {
  it("finds its page, and lands on that message", () => {
    const message = "No fence is open, so changes on the cluster are refused.";
    const hit = search(ix, message)[0];
    expect(hit.doc.id).toBe("errors-admin-and-safety");
    expect(landing(hit, message)).toMatch(/^no-fence-is-open-so-changes-on-the-cluster-are-refused/);
  });
});

describe("what a plain question means", () => {
  const top = (q: string, n = 3) => search(ix, q).slice(0, n).map((h) => h.doc.id);

  it("logs are a record, not a way in", () => {
    const LOGIN = ["settings-cluster-login", "ssh-sudo-and-host-keys", "set-up-key-login"];
    for (const q of ["service logs on cell2", "see the logs on my spark", "where are the logs in the app", "read the log on cell2"]) {
      expect(top(q, 2).filter((id) => LOGIN.includes(id)), q).toEqual([]);
    }
    expect(top("service logs on cell2")).toEqual(expect.arrayContaining(["restart-a-service"]));
    expect(top("where are the logs in the app").some((id) => ["manage-containers", "activity-log", "find-what-sparkbox-did", "collect-diagnostics"].includes(id))).toBe(true);
  });

  it("logging in is still a way in", () => {
    expect(top("how do i log in to a node")).toContain("open-a-terminal");
    expect(top("what user does sparkbox log in as")).toEqual(expect.arrayContaining(["settings-cluster-login"]));
    expect(top("how sparkbox logs in")).toEqual(expect.arrayContaining(["settings-cluster-login"]));
  });

  it("a question about what's going on isn't asking for a definition", () => {
    for (const q of ["what is using my gpu", "whats using my gpu", "what is taking all my gpu", "what is using my cpu"]) {
      expect(top(q)[0], q).toBe("see-and-stop-processes");
      expect(top(q)[0], q).not.toBe("glossary");
    }
    for (const q of ["what is happening on my cluster", "whats going on with my nodes"]) {
      expect(top(q).some((id) => ["read-the-dashboard", "dashboard"].includes(id)), q).toBe(true);
      expect(top(q), q).not.toContain("glossary");
    }
  });

  it("a word next to one on a Glossary heading isn't that heading", () => {
    expect(search(ix, "what is using the kernel")[0].doc.id).not.toBe("glossary");
    expect(search(ix, "what does doing an update do")[0].doc.id).not.toBe("glossary");
  });

  it("being locked out leads to Admin mode, however it's put", () => {
    for (const q of [
      "this app wont let me change anything", "sparkbox refuses to make changes", "it doesnt let me change anything", "my changes are blocked",
      "changes are not allowed", "sparkbox refused my change", "why cant i change anything", "access denied when i change a setting",
    ]) {
      expect(top(q), q).toEqual(expect.arrayContaining(["unlock-admin-mode", "admin-mode"]));
    }
  });

  it("a message about the fence is still about the fence", () => {
    expect(search(ix, "No fence is open, so changes on the cluster are refused.")[0].doc.id).toBe("errors-admin-and-safety");
    expect(top("the fence refused my change").some((id) => ["the-fence", "settings-fence", "open-a-fence", "errors-admin-and-safety"].includes(id))).toBe(true);
  });

  it("says what a new reader says", () => {
    expect(top("how do i get a model")).toContain("download-a-model");
    expect(top("what do i do first")[0]).toBe("getting-started");
    expect(top("my computer went to sleep")).toContain("keep-this-mac-awake");
    expect(top("my mac fell asleep")).toContain("keep-this-mac-awake");
    expect(top("i dont know anything about servers")).toContain("getting-started");
    expect(top("i clicked something and now nothing works")).toContain("troubleshooting");
  });

  it("tells a node's updates from SparkBox's own", () => {
    const article = allArticles().find((a) => a.meta.id === "updates")!;
    expect(article.body).toMatch(/SparkBox doesn't update itself/);
    expect(top("update sparkbox")).toContain("updates");
  });
});

describe("a typo", () => {
  it("is mended, whatever phrases.json holds", () => {
    for (const [q, want] of [
      ["laod a model", "load-a-model-on-one-node"], ["resrart a node", "restart-a-node"], ["delte a file", "delete-and-restore-files"],
      ["sevice logs", "restart-a-service"], ["modles", "models-window"], ["clutser", "your-cluster"], ["hugginface download", "download-a-model"],
    ] as const) {
      expect(search(ix, q).slice(0, 3).map((h) => h.doc.id), q).toContain(want);
    }
  });

  it("finds as much as the right spelling does", () => {
    for (const [typo, right] of [["laod a model", "load a model"], ["modle wont load", "model wont load"], ["resrart a node", "restart a node"], ["conect the vpn", "connect the vpn"]]) {
      expect(search(ix, typo).map((h) => h.doc.id).slice(0, 5), typo).toEqual(search(ix, right).map((h) => h.doc.id).slice(0, 5));
    }
  });
});

describe("the test build's pages", () => {
  const FENCE = ["open-a-fence", "the-fence", "settings-fence"];

  it("don't answer someone locked out of their own app", () => {
    for (const q of ["this app wont let me change anything", "why cant i change anything", "cant make changes"]) {
      const top = search(ix, q).slice(0, 3).map((h) => h.doc.id);
      expect(top.filter((id) => FENCE.includes(id)), q).toEqual([]);
      expect(top.some((id) => ["unlock-admin-mode", "admin-mode", "use-look-only-mode"].includes(id)), q).toBe(true);
    }
  });

  it("come when the query asks for them", () => {
    for (const q of ["fence", "open a fence", "test build", "let the test build make changes"]) {
      expect(FENCE, q).toContain(search(ix, q)[0].doc.id);
    }
  });

  it("stay out of the top of the Help menu", () => {
    const howTos = manifest.helpMenu.find((e) => e.label === "How-To Guides")?.submenu ?? [];
    const before = howTos.slice(0, howTos.findIndex((e) => e.toc));
    expect(before.map((e) => e.article)).not.toContain("open-a-fence");
  });
});

describe("what is a word", () => {
  const glossary = ix.docs.find((d) => d.id === "glossary")!;
  const entries = glossary.headings.filter((h) => h.level >= 3 || h.text.length > 1).filter((h) => h.text.length > 1 && h.text !== "Numbers and symbols");

  it("the Glossary is there to sample", () => {
    expect(entries.length).toBeGreaterThan(200);
  });

  it("every entry answers its own question from the top three, and opens on that entry", () => {
    const wrong = entries.flatMap((h) => {
      const q = `what is ${h.text}`;
      const top = search(ix, q).slice(0, 3);
      const hit = top.find((x) => x.doc.id === "glossary");
      if (!hit) return [`${q}: not in the top three`];
      const at = landing(hit, q);
      return at === h.anchor ? [] : [`${q}: opens at ${at}`];
    });
    expect(wrong).toEqual([]);
  });

  it("however the question is put", () => {
    for (const q of ["what is a gpu", "what's a GPU?", "what are gpus", "what does gpu mean", "define gpu", "meaning of gpu"]) {
      expect(search(ix, q).slice(0, 3).map((h) => h.doc.id), q).toContain("glossary");
    }
    expect(search(ix, "what is the fence")[0].doc.id).toBe("the-fence");
  });
});

describe("where a result opens", () => {
  it("at the top of any article searched by its title", () => {
    const wrong = ix.docs.flatMap((d) => {
      const hit = search(ix, d.title).find((h) => h.doc.id === d.id);
      if (!hit) return [`${d.title}: not found`];
      const at = landing(hit, d.title);
      return at ? [`${d.title}: opens at ${at}`] : [];
    });
    expect(wrong).toEqual([]);
  });

  it("at the top of a How-To the query names, not partway down", () => {
    for (const [q, id] of [
      ["check disk space", "check-disk-space"], ["chat with a cluster model", "chat-with-a-cluster-model"], ["install updates", "install-updates"], ["the fence", "the-fence"],
    ] as const) {
      expect(landing(hitFor(q, id), q), q).toBeUndefined();
    }
  });

  it("never at a section every article has", () => {
    for (const q of ["before you start", "steps", "download what you should see"]) {
      for (const h of search(ix, q, 20)) {
        const a = landing(h, q);
        expect(a === "steps" || a === "before-you-start" || a === "what-you-should-see", `${q}: ${h.doc.id}#${a}`).toBe(false);
      }
    }
  });

  it("at the entry on a catalogue page", () => {
    const hit = hitFor("node offline", "troubleshooting");
    expect(landing(hit, "node offline")).toBe("a-node-shows-offline");
    expect(landing(hitFor("what is a token", "glossary"), "what is a token")).toBe("token");
  });

  it("goes straight to a section the query names", () => {
    expect(landing(hitFor("cancel a download", "follow-a-download"), "cancel a download")).toBe("cancel-a-download");
  });
});

describe("the count", () => {
  it("is every article that matched, however many are listed", () => {
    expect(search(ix, "node").length).toBeGreaterThan(100);
    expect(search(ix, "node", 40).length).toBe(40);
  });
});
