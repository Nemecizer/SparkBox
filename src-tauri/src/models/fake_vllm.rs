//! Test-only vLLM for fake nodes: a `docker` shim (a dash script on the node's PATH) that answers
//! SparkBox's docker commands as Docker 29 does, and a `Script` for what a Mac can't answer itself
//! (/proc/meminfo, /dev/shm, `ss`, the page-cache drop, the whole pre-flight read).
//!
//! A `docker run` of C-L2 (`--entrypoint python3 … /run/sparkbox/launch.py [--key]`) really runs it:
//! the shim builds a root per container from the real `-v` mounts (`launch.py`, `deploy/`, `key`), puts a
//! `vllm` on the container's PATH that execs live/fixtures/vllm_standin.py, and starts
//! `python3 <root>/launch.py [--key]` in the container's own environment. No path through launch.py
//! is skipped: its refusals exit 78 here too, and the stand-in reads the key from the environment
//! launch.py gave it and vLLM's flags from the argv it started. Ranks on different fake nodes
//! rendezvous through `Knobs::rendezvous` and fail for each other (vllm_standin.py's notes).
//!
//! The shim keeps its containers in `$SB_FAKE_STATE/docker`: `<name>.id` (64 hex), `.labels` (k=v
//! lines), `.argv` (what `docker` was given, NUL-separated; kept after `rm`), `.mounts`, `.root`,
//! `.pid`, `.log` (the container's output), `.exit` (its real status), `.started` (its process's pid
//! and the clock, kept after `rm`), `.vllm` (it serves a model), `.created` (`run` couldn't start
//! it), `.restarts` (RestartCount, kept after `rm`), `.restarting` (waiting to start again) and
//! `.stopping` (`docker stop` was asked); `order` lists the names oldest first and `pids` every
//! process it started. Images are "ref id" lines in `images`.
//!
//! Docker's words below are written from moby's sources as of Docker 29, not captured from the
//! nodes: the image-inspect and rm-running errors are the ones tests assert. `docker info`'s store
//! line is captured: all four Sparks printed `OVERLAY2`, word for word, on 2026-11-01.
//!
//! - `image inspect -f '{{.Id}}' <ref>`: the id, else `Error: No such image: <ref>`, exit 1.
//! - `info -f '{{.Driver}} {{json .DriverStatus}}'`: `Knobs::store`.
//! - `run -d …`: records the argv. A `-v` source that doesn't exist is made as a folder, as Docker
//!   makes it, so a key file gone before the run is a folder in the container and launch.py refuses
//!   it. The id is printed once the container runs in a session of its own (as under dockerd, the
//!   SSH session ending doesn't stop it); a name in use is refused as Docker refuses it. A container
//!   that isn't a C-L2 launch runs an idle stand-in. `--restart unless-stopped` is honoured: a
//!   process that ends is started again (vllm_standin.py's `--contain`), `.State.Status` reads
//!   `restarting` between runs and `.RestartCount` counts. `Knobs::run_fails` is a start that fails,
//!   as `docker run --gpus all` did under Docker Desktop on this Mac, 2026-11-01: its reason, a blank
//!   line and the help hint on stderr, exit 125, and a Created container with ExitCode 128.
//! - `ps [-a] [-q] [--no-trunc] --filter label=k[=v]… [--format …]`, newest first.
//! - `inspect -f '{{.State.Status}} {{.State.ExitCode}}[ {{.RestartCount}}]' <name>`, `logs --tail N <name>`.
//! - `stop -t T <ids>`: TERM, up to T seconds, then KILL. The stand-in removes its /dev/shm
//!   segment on TERM and exits 0; a KILL leaves the segment, as it does for a real vLLM.
//! - `rm <ids>` refuses a running container with Docker's words, exit 1; `rm -f` kills it first.
//!
//! The node's `curl` only reaches 127.0.0.1 and its `ping` sends nothing (it answers from
//! `Knobs::jumbo_fails`): nothing a test sends leaves this Mac. Dropping the `Standin` stops every
//! process it started and removes its state.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::fake_node::{self, Node, Script};
use crate::live::Live;
use crate::models::plan::{self, DeploySpec};

const GIB: u64 = 1 << 30;

/// What the node and its stand-ins do.
#[derive(Clone, Debug)]
pub struct Knobs {
    /// Seconds before /health is 200 (weights load, then "Application startup complete.").
    pub warmup_s: f64,
    /// 17+25 comes back as "4 2 2 2 2 2 2 2".
    pub garbage: bool,
    /// The paths /openapi.json lists; empty: there is no /openapi.json (404).
    pub openapi: Vec<String>,
    /// A streamed reply sends a token every this many ms.
    pub stream_ms: u64,
    /// A chat reply (not streamed) thinks first, for this many tokens; None: it doesn't think.
    pub thinks: Option<(Thinks, u32)>,
    /// /proc/meminfo with nothing loaded (bytes).
    pub mem_total: u64,
    pub mem_free: u64,
    pub mem_cached: u64,
    pub mem_available: u64,
    /// What a running stand-in takes from MemFree and MemAvailable (bytes).
    pub load_bytes: u64,
    /// Docker's images: (ref, local id).
    pub images: Vec<(String, String)>,
    /// earlyoom's /etc/default/earlyoom when it runs; None: not installed.
    pub earlyoom: Option<String>,
    /// What `docker info -f '{{.Driver}} {{json .DriverStatus}}'` prints.
    pub store: String,
    /// Where the ranks of a load on several fake nodes meet: a folder every node of the test shares.
    pub rendezvous: Option<PathBuf>,
    /// A rank here fails this long after it starts (exit 1), and every other rank loses it.
    pub rank_dies_after_s: Option<f64>,
    /// A rank here starts and never gets anywhere.
    pub start_hangs: bool,
    /// `docker run` here fails to start its container with these words (exit 125), and leaves it
    /// Created, as Docker does.
    pub run_fails: Option<String>,
    /// `docker run` here never answers.
    pub run_hangs: bool,
    /// The key file goes after deploy-files and before `docker run`: the mount makes a folder of it.
    pub key_file_goes: bool,
    /// launch.py isn't given `--key`, whatever the run says: vLLM starts with no key, wide open.
    pub key_flag_dropped: bool,
    /// A rank here doesn't join until `Standin::let_join`.
    pub join_held: bool,
    /// What Ollama's /api/ps answers on the node; None: nothing listens on 11434.
    pub ollama_ps: Option<String>,
    /// C-P2's reply stops before this section's header, as when a node drops mid-read.
    pub preflight_stops_before: Option<String>,
    /// Interfaces a jumbo ping fails on.
    pub jumbo_fails: Vec<String>,
    /// The message queue says this LAN address, whatever VLLM_HOST_IP is (the owner's run).
    pub mq_on_lan: Option<String>,
    /// A rank here prints its key in its log.
    pub prints_key: bool,
    /// This node has no finished copy of the model folder, whatever the shared one holds.
    pub no_model: bool,
}

/// How a thinking model's thinking comes back (vllm_standin.py's --thinks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Thinks {
    /// In the content between <think> tags, as Qwen3 without a reasoning parser; not at all when
    /// the request's chat_template_kwargs turn enable_thinking off.
    Inline,
    /// The same, whatever the request says (a template without the switch).
    InlineAlways,
    /// In reasoning_content, content null until the answer, whatever the request says (gpt-oss,
    /// or any model behind a reasoning parser).
    Parsed,
    /// In the content with no opening tag, whatever the request says: the template wrote the
    /// <think> (DeepSeek-R1 without a reasoning parser), so a reply cut off reads as plain text.
    Untagged,
}

impl Thinks {
    fn flag(self) -> &'static str {
        match self {
            Thinks::Inline => "inline",
            Thinks::InlineAlways => "inline-always",
            Thinks::Parsed => "parsed",
            Thinks::Untagged => "untagged",
        }
    }
}

/// The image the Session B load uses, and its local id in tests.
pub const GLM53_IMAGE: &str = "vllm/vllm-openai:glm53-flash-arm64-cu130";

/// Docker's overlay2 store as `docker info -f` printed it on all four Sparks, 2026-11-01.
pub const OVERLAY2: &str = r#"overlay2 [["Backing Filesystem","extfs"],["Supports d_type","true"],["Using metacopy","false"],["Native Overlay Diff","true"],["userxattr","false"]]"#;
/// The containerd image store, which keeps a different id for the same image (P2.5). No Spark
/// runs it: the shape is moby's, for the refusal.
pub const CONTAINERD_STORE: &str = r#"overlayfs [["driver-type","io.containerd.snapshotter.v1"]]"#;

pub fn test_image_id() -> String {
    format!("sha256:{}", "a".repeat(64))
}

impl Default for Knobs {
    /// An idle GB10 as cell2 read on Oct 29 (121.7 GiB total, 116.9 available, 3 GiB cached),
    /// with the glm53-flash image, no earlyoom, and vLLM's native routes in /openapi.json.
    fn default() -> Self {
        Knobs {
            warmup_s: 0.0,
            garbage: false,
            openapi: ["/v1/chat/completions", "/v1/completions", "/v1/models", "/v1/messages", "/v1/messages/count_tokens", "/v1/responses"].map(String::from).to_vec(),
            stream_ms: 0,
            thinks: None,
            mem_total: 127_598_748 * 1024,
            mem_free: 119_861_600 * 1024,
            mem_cached: 3_196_204 * 1024,
            mem_available: 122_601_504 * 1024,
            load_bytes: 127_598_748 * 1024 / 4 * 3,
            images: vec![(GLM53_IMAGE.into(), test_image_id())],
            earlyoom: None,
            store: OVERLAY2.into(),
            rendezvous: None,
            rank_dies_after_s: None,
            start_hangs: false,
            run_fails: None,
            run_hangs: false,
            key_file_goes: false,
            key_flag_dropped: false,
            join_held: false,
            ollama_ps: None,
            preflight_stops_before: None,
            jumbo_fails: Vec::new(),
            mq_on_lan: None,
            prints_key: false,
            no_model: false,
        }
    }
}

/// The stand-in script, where the shim runs it from.
fn standin_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/live/fixtures/vllm_standin.py")
}

const DOCKER: &str = r#"# docker, as Docker 29 answers SparkBox (models/fake_vllm.rs).
d="$SB_FAKE_STATE/docker"; mkdir -p "$d"; touch "$d/order" "$d/images" "$d/pids"
standin='@STANDIN@'; parent='@PARENT@'
printf '%s\0' "$@" > "$d/.argv.$$"
trap 'rm -f "${d:?}/.argv.$$" "${d:?}/.labels.$$" "${d:?}/.mounts.$$"' EXIT

# The container a name or an id (or the start of one) means.
named() {
  [ -n "$1" ] || return 1
  for f in "$d"/*.id; do
    [ -e "$f" ] || continue
    n=${f##*/}; n=${n%.id}
    case "$1" in "$n"|"/$n") echo "$n"; return 0 ;; esac
    case "$(cat "$f")" in "$1"*) echo "$n"; return 0 ;; esac
  done
  return 1
}
alive() { p=$(cat "$d/$1.pid" 2>/dev/null) && [ -n "$p" ] && kill -0 "$p" 2>/dev/null; }
# Its status, written the moment dockerd saw it end.
ended() { i=0; while [ ! -s "$d/$1.exit" ] && [ $i -lt 40 ]; do sleep 0.05; i=$((i + 1)); done; cat "$d/$1.exit" 2>/dev/null || echo 137; }
state() {
  if alive "$1"; then echo "running 0"; return; fi
  if [ -e "$d/$1.created" ]; then echo "created $(cat "$d/$1.exit")"; return; fi
  e=$(ended "$1")
  if [ -e "$d/$1.restarting" ]; then echo "restarting $e"; else echo "exited $e"; fi
}
status() {
  if alive "$1"; then echo "Up 2 minutes"; return; fi
  if [ -e "$d/$1.created" ]; then echo "Created"; return; fi
  e=$(ended "$1")
  if [ -e "$d/$1.restarting" ]; then echo "Restarting ($e) 1 second ago"; else echo "Exited ($e) 1 minute ago"; fi
}
restarts() { cat "$d/$1.restarts" 2>/dev/null || echo 0; }
label() { sed -n "s/^$2=//p" "$d/$1.labels"; }
gone() { i=0; while alive "$1" && [ $i -lt 200 ]; do sleep 0.05; i=$((i + 1)); done; }
image_id() {
  while read -r ref iid; do
    case "$1" in "$ref"|"$iid") echo "$iid"; return 0 ;; esac
    [ "$ref" = "$1:latest" ] && { echo "$iid"; return 0; }
  done < "$d/images"
  return 1
}
help() { printf "\nRun 'docker %s --help' for more information\n" "$1" >&2; }

case "$1" in
image)
  [ "$2" = inspect ] || { echo "fake docker: unsupported: $*" >&2; exit 2; }
  shift 2; fmt=""
  case "$1" in -f|--format) fmt="$2"; shift 2 ;; esac
  [ -z "$fmt" ] || [ "$fmt" = '{{.Id}}' ] || { echo "fake docker: unsupported image inspect format: $fmt" >&2; exit 2; }
  rc=0
  for r; do
    image_id "$r" || { echo "Error: No such image: $r" >&2; rc=1; }
  done
  exit $rc ;;
info)
  shift; fmt=""
  case "$1" in -f|--format) fmt="$2" ;; esac
  case "$fmt" in
    '') echo "Server Version: 29.0.0" ;;
    '{{.Driver}} {{json .DriverStatus}}') cat "$d/store" ;;
    *) echo "fake docker: unsupported info format: $fmt" >&2; exit 2 ;;
  esac ;;
run)
  shift; name=""; entry=""; policy=no; : > "$d/.labels.$$"; : > "$d/.mounts.$$"
  while [ $# -gt 0 ]; do
    case "$1" in
      -d|--detach|--rm|--init|--privileged|-t|--tty|-i|--interactive|-it) ;;
      --name) name="$2"; shift ;;
      -l|--label) printf '%s\n' "$2" >> "$d/.labels.$$"; shift ;;
      -v|--volume)
        src=${2%%:*}; rest=${2#*:}; dst=${rest%%:*}
        # -v makes a source that isn't there, as a folder (Docker makes it as root).
        [ -e "$src" ] || mkdir -p "$src"
        printf '%s:%s\n' "$src" "$dst" >> "$d/.mounts.$$"; shift ;;
      --entrypoint) entry="$2"; shift ;;
      --restart) policy="$2"; shift ;;
      -e|--env|--network|--net|--ipc|--gpus|--ulimit|--shm-size|-p|--publish|-w|--workdir|-u|--user|--runtime|--device|--cap-add|--security-opt|--pid|-h|--hostname|--mount|--add-host) shift ;;
      -*) printf "unknown flag: %s\nSee 'docker run --help'.\n" "$1" >&2; exit 125 ;;
      *) break ;;
    esac
    shift
  done
  image="$1"; [ $# -gt 0 ] && shift
  if ! image_id "$image" > /dev/null; then
    printf "Unable to find image '%s' locally\ndocker: Error response from daemon: pull access denied for %s, repository does not exist or may require 'docker login'\n" "$image" "${image%%:*}" >&2
    help run; exit 125
  fi
  if [ -e "$d/run-hangs" ]; then
    while :; do sleep 1; done
  fi
  if [ -n "$name" ] && [ -e "$d/$name.id" ]; then
    printf 'docker: Error response from daemon: Conflict. The container name "/%s" is already in use by container "%s". You have to remove (or rename) that container to be able to reuse that name.\n' "$name" "$(cat "$d/$name.id")" >&2
    help run; exit 125
  fi
  id=$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')
  [ -n "$name" ] || name="sparkbox_$(printf '%s' "$id" | cut -c1-8)"
  mv "$d/.argv.$$" "$d/$name.argv"; mv "$d/.labels.$$" "$d/$name.labels"; mv "$d/.mounts.$$" "$d/$name.mounts"
  printf '%s\n' "$id" > "$d/$name.id"; echo "$name" >> "$d/order"; rm -f "${d:?}/${name:?}.exit" "${d:?}/${name:?}.started" "${d:?}/${name:?}.restarts"
  image_id "$image" > "$d/$name.image"
  printf '%s %s\n' "$entry" "$*" > "$d/$name.cmd"
  if [ -s "$d/run-fails" ]; then
    # The daemon made it, then couldn't start it: it stays, Created, with nothing in its log.
    echo 128 > "$d/$name.exit"; : > "$d/$name.created"; : > "$d/$name.log"
    printf 'docker: %s\n' "$(cat "$d/run-fails")" >&2
    help run; exit 125
  fi
  root="$d/$name.root"; rm -rf "${root:?}"; mkdir -p "$root/bin"
  if [ "$entry" = python3 ] && [ "$1" = /run/sparkbox/launch.py ]; then
    # The container's /run/sparkbox, from the real mounts: launch.py's root is its own folder.
    while IFS= read -r m; do
      src=${m%%:*}; dst=${m#*:}
      case "$dst" in
        /run/sparkbox/launch.py) cp "$src" "$root/launch.py" 2>/dev/null ;;
        /run/sparkbox/deploy) ln -s "$src" "$root/deploy" ;;
        /run/sparkbox/key)
          if [ -e "$d/key-file-goes" ]; then rm -f "${src:?}"; mkdir -p "$src"; fi
          ln -s "$src" "$root/key" ;;
      esac
    done < "$d/$name.mounts"
    shift
    [ -e "$d/key-flag-dropped" ] && set --
    : > "$d/$name.vllm"
    {
      echo '#!/bin/dash'
      printf "exec python3 -I '%s' --sb-state '%s' --sb-parent-pid %s --sb-mounts '%s' --sb-seen '%s' %s \"\$@\"\n" "$standin" "$SB_FAKE_STATE" "$parent" "$d/$name.mounts" "$d/$name.seen" "$(cat "$d/knobs" 2>/dev/null)"
    } > "$root/bin/vllm"
    chmod +x "$root/bin/vllm"
    set -- python3 "$root/launch.py" "$@"
  else
    set -- python3 -I "$standin" --sb-state "$SB_FAKE_STATE" --sb-parent-pid "$parent" --sb-idle
  fi
  # dockerd runs the container, not this session: --contain ignores the session's HUP and TERM until
  # it's in a session of its own, and `run -d` returns once the container runs. Ignored before it's
  # spawned: a hangup can land as soon as its pid exists, before a trap of its own would have run.
  trap '' HUP TERM
  python3 -I "$standin" --contain "$d" "$name" "$policy" "$parent" -- "$@" < /dev/null > "$d/$name.daemon.log" 2>&1 &
  trap - HUP TERM
  c=$!
  # Register even a daemon still starting its interpreter, so Standin::drop can clean it up.
  echo "$c" >> "$d/pids"
  # A detach acknowledgement is a fact, not a five-second guess. The caller's SSH timeout
  # bounds a stuck daemon; never return success without its atomic startup acknowledgement.
  while [ ! -s "$d/$name.started" ]; do
    if ! kill -0 "$c" 2>/dev/null; then
      wait "$c"; rc=$?
      echo "fake docker: container daemon exited before startup (status $rc)" >&2
      cat "$d/$name.daemon.log" >&2
      exit 125
    fi
    sleep 0.05
  done
  echo "$id" ;;
ps)
  shift; all=0; quiet=0; trunc=1; fmt=""; filters=""
  while [ $# -gt 0 ]; do
    case "$1" in
      -a|--all) all=1 ;;
      -q|--quiet) quiet=1 ;;
      -aq|-qa) all=1; quiet=1 ;;
      --no-trunc) trunc=0 ;;
      -f|--filter) case "$2" in label=*) filters="$filters ${2#label=}" ;; *) echo "fake docker: unsupported filter: $2" >&2; exit 2 ;; esac; shift ;;
      --format) fmt="$2"; shift ;;
      *) printf "unknown flag: %s\nSee 'docker ps --help'.\n" "$1" >&2; exit 125 ;;
    esac
    shift
  done
  for n in $(sed -n '1!G;h;$p' "$d/order"); do
    [ $all = 1 ] || alive "$n" || continue
    ok=1
    for l in $filters; do
      case "$l" in
        *=*) grep -qxF "$l" "$d/$n.labels" ;;
        *) grep -q "^$l=" "$d/$n.labels" ;;
      esac || { ok=0; break; }
    done
    [ $ok = 1 ] || continue
    if [ $trunc = 1 ]; then id=$(cut -c1-12 "$d/$n.id"); else id=$(cat "$d/$n.id"); fi
    if [ $quiet = 1 ]; then echo "$id"; continue; fi
    case "$fmt" in
      '{{.ID}} {{.Label "sparkbox.instance"}}') echo "$id $(sed -n 's/^sparkbox\.instance=//p' "$d/$n.labels")" ;;
      '{{.ID}}') echo "$id" ;;
      '{{.Names}}') echo "$n" ;;
      '{{.ID}} {{.Names}}') echo "$id $n" ;;
      '{{.ID}}|{{.Names}}|{{.Label "sparkbox.deployment"}}|{{.Label "sparkbox.instance"}}|{{.Status}}')
        echo "$id|$n|$(label "$n" sparkbox.deployment)|$(label "$n" sparkbox.instance)|$(status "$n")" ;;
      '{{.ID}}|{{.Names}}|{{.Status}}|{{.Command}}') echo "$id|$n|$(status "$n")|\"$(cat "$d/$n.cmd" 2>/dev/null)\"" ;;
      *) echo "fake docker: unsupported ps format: $fmt" >&2; exit 2 ;;
    esac
  done ;;
inspect)
  shift; fmt=""
  case "$1" in -f|--format) fmt="$2"; shift 2 ;; esac
  rc=0
  for c; do
    n=$(named "$c") || { echo "Error: No such object: $c" >&2; rc=1; continue; }
    case "$fmt" in
      '{{.State.Status}} {{.State.ExitCode}}') state "$n" ;;
      '{{.State.Status}} {{.State.ExitCode}} {{.RestartCount}}') echo "$(state "$n") $(restarts "$n")" ;;
      '{{.State.Status}}') state "$n" | cut -d' ' -f1 ;;
      '{{.Id}}') cat "$d/$n.id" ;;
      '{{.Id}} {{.State.Status}} {{.Image}} {{index .Config.Labels "sparkbox.instance"}} {{index .Config.Labels "sparkbox.deployment"}}')
        echo "$(cat "$d/$n.id") $(state "$n" | cut -d' ' -f1) $(cat "$d/$n.image") $(label "$n" sparkbox.instance) $(label "$n" sparkbox.deployment)" ;;
      *) echo "fake docker: unsupported inspect format: $fmt" >&2; exit 2 ;;
    esac
  done
  exit $rc ;;
logs)
  shift; tail=""
  while [ $# -gt 1 ]; do
    case "$1" in
      --tail|-n) tail="$2"; shift 2 ;;
      --tail=*) tail="${1#--tail=}"; shift ;;
      *) echo "fake docker: unsupported: logs $*" >&2; exit 2 ;;
    esac
  done
  n=$(named "$1") || { echo "Error response from daemon: No such container: $1" >&2; exit 1; }
  if [ -z "$tail" ] || [ "$tail" = all ]; then cat "$d/$n.log"; else tail -n "$tail" "$d/$n.log"; fi ;;
stop)
  shift; t=10
  while [ $# -gt 0 ]; do
    case "$1" in
      -t|--time|--timeout) t="$2"; shift 2 ;;
      --time=*|--timeout=*) t="${1#*=}"; shift ;;
      *) break ;;
    esac
  done
  rc=0
  for c; do
    n=$(named "$c") || { echo "Error response from daemon: No such container: $c" >&2; rc=1; continue; }
    : > "$d/$n.stopping"
    if alive "$n"; then
      p=$(cat "$d/$n.pid"); kill -TERM "$p" 2>/dev/null
      i=0; while alive "$n" && [ $i -lt $((t * 10)) ]; do sleep 0.1; i=$((i + 1)); done
      if alive "$n"; then kill -KILL "$p" 2>/dev/null; gone "$n"; fi
      ended "$n" > /dev/null
    fi
    echo "$c"
  done
  exit $rc ;;
rm)
  shift; force=0
  while [ $# -gt 0 ]; do
    case "$1" in
      -f|--force|-fv|-vf) force=1; shift ;;
      -v|--volumes) shift ;;
      *) break ;;
    esac
  done
  rc=0
  for c; do
    n=$(named "$c") || { [ $force = 1 ] || { echo "Error response from daemon: No such container: $c" >&2; rc=1; }; continue; }
    if alive "$n"; then
      if [ $force = 0 ]; then
        echo "Error response from daemon: cannot remove container \"/$n\": container is running: stop the container before removing or force remove" >&2
        rc=1; continue
      fi
      : > "$d/$n.stopping"
      kill -KILL "$(cat "$d/$n.pid")" 2>/dev/null; gone "$n"
      # A name cannot be reused until the simulated namespace's remaining engine is gone.
      ended "$n" > /dev/null
    fi
    rm -f "${d:?}/${n:?}.id" "${d:?}/${n:?}.labels" "${d:?}/${n:?}.pid" "${d:?}/${n:?}.exit" "${d:?}/${n:?}.log" "${d:?}/${n:?}.vllm" "${d:?}/${n:?}.mounts" "${d:?}/${n:?}.cmd" "${d:?}/${n:?}.created" "${d:?}/${n:?}.restarting" "${d:?}/${n:?}.stopping"
    rm -rf "${d:?}/${n:?}.root"
    grep -vxF "$n" "$d/order" > "$d/order.new"; mv "$d/order.new" "$d/order"
    echo "$c"
  done
  exit $rc ;;
*) echo "fake docker: unsupported: $*" >&2; exit 2 ;;
esac
"#;

/// curl on a fake node: only to this Mac's loopback, whatever the test's spec says.
const CURL: &str = r#"# curl, to 127.0.0.1 only (models/fake_vllm.rs): a fake node's requests never leave this Mac.
for a; do
  case "$a" in
    http://*|https://*)
      h=${a#*://}; h=${h%%/*}; h=${h%:*}
      case "$h" in 127.0.0.1|localhost) ;; *) echo "curl: (7) Failed to connect to $h: a fake node only reaches 127.0.0.1" >&2; exit 7 ;; esac ;;
  esac
done
exec /usr/bin/curl "$@"
"#;

/// du on a fake node: GNU's `-sbL`, the bytes a folder's files hold, which macOS's du has no flag for.
const DU: &str = r#"# du -sbL PATH, as coreutils prints it (models/fake_vllm.rs).
[ "$1" = -sbL ] || exec /usr/bin/du "$@"
[ -e "$2" ] || { echo "du: cannot access '$2': No such file or directory" >&2; exit 1; }
printf '%s\t%s\n' "$(find -L "$2" -type f -exec cat {} + | wc -c | tr -d ' ')" "$2"
"#;

/// ping on a fake node, as iputils answers C-J: nothing is sent, and the answer is the knob's.
const PING: &str = r#"# ping (models/fake_vllm.rs): a fake node's jumbo path works unless Knobs::jumbo_fails names the interface.
d="$SB_FAKE_STATE/docker"; iface=""; addr=""
while [ $# -gt 0 ]; do
  case "$1" in
    -I) iface="$2"; shift 2 ;;
    -c|-W|-M|-s) shift 2 ;;
    -*) echo "ping: invalid argument: '$1'" >&2; exit 2 ;;
    *) addr="$1"; shift ;;
  esac
done
printf '%s %s\n' "$iface" "$addr" >> "$d/pings"
if grep -qxF "$iface" "$d/jumbo-fails" 2>/dev/null; then
  echo "ping: local error: message too long, mtu=1500" >&2; exit 1
fi
printf 'PING %s(%s) from %s: 8972(9000) bytes of data.\n' "$addr" "$addr" "$iface"
"#;

struct Inner {
    home: PathBuf,
    state: PathBuf,
    /// The docker shim, to answer the pre-flight's container sections from its real state.
    docker_bin: PathBuf,
    knobs: Mutex<Knobs>,
    drops: AtomicUsize,
    /// The page cache was dropped: MemFree is about MemAvailable until the next knobs.
    dropped: AtomicBool,
}

/// The stand-in installed on one fake node.
pub struct Standin {
    inner: Arc<Inner>,
}

/// Puts the docker shim and the loopback-only curl on `node`'s PATH, with `knobs`. Use
/// `script()` as (part of) the node's Script, or start the node with `node()`.
pub fn install(node: &Node, knobs: Knobs) -> Standin {
    let shim = DOCKER.replace("@STANDIN@", &standin_path().display().to_string()).replace("@PARENT@", &std::process::id().to_string());
    node.bin("docker", &shim);
    node.bin("curl", CURL);
    node.bin("ping", PING);
    node.bin("du", DU);
    let inner = Arc::new(Inner { home: node.home.clone(), state: node.state_dir.clone(), docker_bin: node.bin_dir.join("docker"), knobs: Mutex::new(knobs.clone()), drops: AtomicUsize::new(0), dropped: AtomicBool::new(false) });
    let s = Standin { inner };
    s.write_knobs(&knobs);
    s
}

/// A fake node with the stand-in on it: `other` answers first (a sparkjob reply…), then the
/// stand-in's script, then the real shell.
pub async fn node_with(home: &Path, knobs: Knobs, other: Script) -> (Node, u16, Standin) {
    let slot: Arc<OnceLock<Script>> = Arc::default();
    let s = slot.clone();
    let script: Script = Arc::new(move |cmd, stdin| other(cmd, stdin).or_else(|| s.get().and_then(|f| f(cmd, stdin))));
    let (node, port) = fake_node::fake_node(home, script).await;
    let standin = install(&node, knobs);
    let _ = slot.set(standin.script());
    (node, port, standin)
}

pub async fn node(home: &Path, knobs: Knobs) -> (Node, u16, Standin) {
    node_with(home, knobs, fake_node::real_shell()).await
}

/// A port nothing on this Mac listens on right now.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

impl Standin {
    fn docker_dir(&self) -> PathBuf {
        self.inner.state.join("docker")
    }

    fn write_knobs(&self, k: &Knobs) {
        let d = self.docker_dir();
        std::fs::create_dir_all(&d).unwrap();
        let mut args = vec![format!("--sb-warmup {}", k.warmup_s), format!("--sb-stream-ms {}", k.stream_ms)];
        if k.garbage {
            args.push("--sb-garbage".into());
        }
        if let Some((how, tokens)) = k.thinks {
            args.push(format!("--sb-thinks {} --sb-think-tokens {tokens}", how.flag()));
        }
        if let Some(dir) = &k.rendezvous {
            args.push(format!("--sb-rendezvous {}", dir.display()));
        }
        if let Some(s) = k.rank_dies_after_s {
            args.push(format!("--sb-die-after {s}"));
        }
        if k.start_hangs {
            args.push("--sb-hang".into());
        }
        if let Some(ip) = &k.mq_on_lan {
            args.push(format!("--sb-mq-lan {ip}"));
        }
        if k.prints_key {
            args.push("--sb-print-key".into());
        }
        if k.join_held {
            args.push(format!("--sb-hold-join {}", d.join("hold-join").display()));
        }
        args.push(format!("--sb-openapi {}", if k.openapi.is_empty() { "none".to_string() } else { k.openapi.join(",") }));
        std::fs::write(d.join("knobs"), args.join(" ") + "\n").unwrap();
        let images: String = k.images.iter().map(|(r, i)| format!("{r} {i}\n")).collect();
        std::fs::write(d.join("images"), images).unwrap();
        std::fs::write(d.join("store"), format!("{}\n", k.store)).unwrap();
        std::fs::write(d.join("run-fails"), k.run_fails.clone().unwrap_or_default()).unwrap();
        std::fs::write(d.join("jumbo-fails"), k.jumbo_fails.iter().map(|i| format!("{i}\n")).collect::<String>()).unwrap();
        if k.join_held {
            args_hold(&d);
        }
        for (file, on) in [("run-hangs", k.run_hangs), ("key-file-goes", k.key_file_goes), ("key-flag-dropped", k.key_flag_dropped)] {
            if on {
                std::fs::write(d.join(file), "").unwrap();
            } else {
                let _ = std::fs::remove_file(d.join(file));
            }
        }
    }

    /// New knobs: images and memory at once, the stand-in's behavior for the next `docker run`.
    pub fn set_knobs(&self, k: Knobs) {
        self.write_knobs(&k);
        *self.inner.knobs.lock().unwrap() = k;
        self.inner.dropped.store(false, Ordering::SeqCst);
    }

    /// The knobs it has now, to put back after `set_knobs`.
    pub fn knobs(&self) -> Knobs {
        self.inner.knobs.lock().unwrap().clone()
    }

    /// The Script answering what the Mac can't (see the module notes); None for the rest.
    pub fn script(&self) -> Script {
        let inner = self.inner.clone();
        Arc::new(move |cmd, stdin| inner.answer(cmd, stdin))
    }

    /// Containers whose process runs, oldest first.
    pub fn running(&self) -> Vec<String> {
        self.inner.running()
    }

    /// What's in the node's /dev/shm.
    pub fn shm(&self) -> Vec<String> {
        self.inner.shm()
    }

    /// The arguments `docker` got for the container `name`'s `run` (kept after it's removed).
    pub fn argv(&self, name: &str) -> Vec<String> {
        let raw = std::fs::read(self.docker_dir().join(format!("{name}.argv"))).unwrap_or_default();
        let mut words: Vec<String> = raw.split(|b| *b == 0).map(|w| String::from_utf8_lossy(w).into_owned()).collect();
        words.pop(); // after the last NUL
        words
    }

    /// The container's log so far.
    pub fn log(&self, name: &str) -> String {
        std::fs::read_to_string(self.docker_dir().join(format!("{name}.log"))).unwrap_or_default()
    }

    /// Explain a failed fixture transition without assuming a missing log means a model failure.
    pub fn diagnostic(&self, name: &str) -> String {
        ["pid", "started", "restarts", "exit", "daemon.log", "log"].iter().map(|suffix| {
            let value = std::fs::read_to_string(self.docker_dir().join(format!("{name}.{suffix}"))).unwrap_or_else(|_| "<absent>".into());
            format!("{suffix}: {value}")
        }).collect::<Vec<_>>().join("\n")
    }

    /// When the container `name`'s process started, by the shim's clock (ms; kept after it's removed).
    pub fn started_ms(&self, name: &str) -> Option<u64> {
        let text = std::fs::read_to_string(self.docker_dir().join(format!("{name}.started"))).ok()?;
        text.split_whitespace().nth(1)?.parse().ok()
    }

    /// When the container `name`'s process ended (ms; kept after it's removed).
    pub fn ended_ms(&self, name: &str) -> Option<u64> {
        std::fs::read_to_string(self.docker_dir().join(format!("{name}.ended"))).ok()?.trim().parse().ok()
    }

    /// How many times the restart policy started the container `name` again (kept after it's removed).
    pub fn restarts(&self, name: &str) -> u64 {
        std::fs::read_to_string(self.docker_dir().join(format!("{name}.restarts"))).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(0)
    }

    /// A rank held by `Knobs::join_held` joins now.
    pub fn let_join(&self) {
        let _ = std::fs::remove_file(self.docker_dir().join("hold-join"));
    }

    /// Every container `docker run` has made here, removed or not, by name.
    pub fn names(&self) -> Vec<String> {
        let dir = std::fs::read_dir(self.docker_dir()).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>()).unwrap_or_default();
        let mut names: Vec<String> = dir.iter().filter_map(|f| f.strip_suffix(".argv").map(String::from)).collect();
        names.sort();
        names
    }

    /// What the process really got, as vllm_standin.py wrote it (`--sb-seen`): {argv, unknown, env, hasKey, values}.
    pub fn seen(&self, name: &str) -> serde_json::Value {
        let text = std::fs::read_to_string(self.docker_dir().join(format!("{name}.seen"))).unwrap_or_default();
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    }

    /// The interface and address of every jumbo ping that reached this node.
    pub fn pings(&self) -> Vec<String> {
        std::fs::read_to_string(self.docker_dir().join("pings")).unwrap_or_default().lines().map(String::from).collect()
    }

    /// How many page-cache drops ran (through sudo, with the right password).
    pub fn drops(&self) -> usize {
        self.inner.drops.load(Ordering::SeqCst)
    }

    /// Kills a container's process outright (an OOM kill, a crash): its /dev/shm segment stays.
    pub fn kill(&self, name: &str) {
        if let Some(pid) = self.inner.pid(name) {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }

    /// Every process the shim started, while it's still a stand-in (a pid isn't reused).
    fn processes(&self) -> Vec<i32> {
        let pids = std::fs::read_to_string(self.docker_dir().join("pids")).unwrap_or_default();
        // The shell registers a daemon before Python starts, and the daemon records itself
        // with each child. Track processes, not registration events, including across restarts.
        let mut pids: Vec<i32> = pids.lines().filter_map(|l| l.trim().parse().ok()).collect();
        pids.sort_unstable();
        pids.dedup();
        pids.into_iter().filter(|p| is_standin(*p, &self.inner.state)).collect()
    }
}

/// `Knobs::join_held`'s file: a rank doesn't join while it's there.
fn args_hold(docker: &Path) {
    std::fs::write(docker.join("hold-join"), "").unwrap();
}

/// A recorded fixture process in this node's private directory, including its launcher.
/// Checked before a signal is sent, so a reused pid outside this fixture is left alone.
fn is_standin(pid: i32, state: &Path) -> bool {
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .map(|o| {
            let command = String::from_utf8_lossy(&o.stdout);
            command.contains(state.to_string_lossy().as_ref())
                && (command.contains("vllm_standin.py") || command.contains(".root/launch.py"))
        })
        .unwrap_or(false)
}

impl Drop for Standin {
    fn drop(&mut self) {
        let pids = self.processes();
        for p in &pids {
            unsafe { libc::kill(*p, libc::SIGTERM) };
        }
        for _ in 0..40 {
            if pids.iter().all(|p| !is_standin(*p, &self.inner.state)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        for p in pids.iter().filter(|p| is_standin(**p, &self.inner.state)) {
            unsafe { libc::kill(*p, libc::SIGKILL) };
        }
        let _ = std::fs::remove_dir_all(self.docker_dir());
        let _ = std::fs::remove_dir_all(self.inner.state.join("shm"));
        let _ = std::fs::remove_file(self.inner.state.join("pid"));
    }
}

/// /proc/meminfo's way of printing a value.
fn meminfo_line(name: &str, bytes: u64) -> String {
    format!("{:<16}{:>8} kB\n", format!("{name}:"), bytes / 1024)
}

/// "Oct 30 12:01", as `ls -l` shows a recent file (UTC: the fake node's zone).
fn ls_time(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (_, mo, d, h, mi, _, _) = crate::demo::ops::civil(secs);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {d:>2} {h:02}:{mi:02}", MONTHS[(mo as usize).saturating_sub(1).min(11)])
}

/// The quoted word right after `after` in `cmd` (sh_quote's form), unquoted.
fn quoted_after(cmd: &str, after: &str) -> Option<String> {
    let rest = cmd.split_once(after)?.1;
    let rest = rest.strip_prefix('\'')?;
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

impl Inner {
    fn docker(&self) -> PathBuf {
        self.state.join("docker")
    }

    fn pid(&self, name: &str) -> Option<i32> {
        std::fs::read_to_string(self.docker().join(format!("{name}.pid"))).ok()?.trim().parse().ok()
    }

    fn running(&self) -> Vec<String> {
        let order = std::fs::read_to_string(self.docker().join("order")).unwrap_or_default();
        order.lines().filter(|n| self.pid(n).is_some_and(|p| unsafe { libc::kill(p, 0) } == 0)).map(String::from).collect()
    }

    /// vLLM servers running (not idle stand-ins): each takes `load_bytes`.
    fn serving(&self) -> u64 {
        self.running().iter().filter(|n| self.docker().join(format!("{n}.vllm")).exists()).count() as u64
    }

    fn shm(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.state.join("shm")).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        names.sort();
        names
    }

    /// (MemTotal, MemFree, MemAvailable, Cached) now.
    fn mem(&self) -> (u64, u64, u64, u64) {
        let k = self.knobs.lock().unwrap().clone();
        let used = k.load_bytes * self.serving();
        let (free, cached) = if self.dropped.load(Ordering::SeqCst) { (k.mem_available.saturating_sub(GIB / 2), 300 << 20) } else { (k.mem_free, k.mem_cached) };
        (k.mem_total, free.saturating_sub(used), k.mem_available.saturating_sub(used), cached)
    }

    /// `ls -la /dev/shm` as GNU ls prints it.
    fn ls_shm(&self) -> String {
        let names = self.shm();
        let mut out = format!("total {}\n", names.len() * 4);
        out.push_str("drwxrwxrwt  2 root   root     60 Oct 29 18:09 .\ndrwxr-xr-x 20 root   root   4660 Oct 29 18:09 ..\n");
        for n in names {
            let meta = std::fs::metadata(self.state.join("shm").join(&n)).ok();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let at = meta.and_then(|m| m.modified().ok()).map(ls_time).unwrap_or_else(|| "Oct 30 12:00".into());
            out.push_str(&format!("-rw-------  1 sparkuser sparkuser {size:>4} {at} {n}\n"));
        }
        out
    }

    /// `ls -ld` of the cache folders, as GNU ls answers with 2>&1: the missing ones first.
    fn ls_caches(&self) -> String {
        let paths = [".cache/vllm", ".cache/flashinfer", ".triton"].map(|p| self.home.join(p));
        let mut out = String::new();
        let mut found: Vec<&PathBuf> = Vec::new();
        for p in &paths {
            if p.is_dir() {
                found.push(p);
            } else {
                out.push_str(&format!("ls: cannot access '{}': No such file or directory\n", p.display()));
            }
        }
        found.sort();
        for p in found {
            let at = std::fs::metadata(p).and_then(|m| m.modified()).map(ls_time).unwrap_or_else(|_| "Oct 30 12:00".into());
            out.push_str(&format!("drwxrwxr-x 2 sparkuser sparkuser 4096 {at} {}\n", p.display()));
        }
        out
    }

    /// The docker shim's own answer, as the node's shell would get it.
    fn shim(&self, args: &[&str]) -> String {
        // The node's own PATH: a test's wrapper around the shim finds what it wraps.
        let path = format!("{}:{}", self.docker_bin.parent().unwrap_or(Path::new("")).display(), std::env::var("PATH").unwrap_or_default());
        let out = std::process::Command::new(&self.docker_bin).args(args).env("SB_FAKE_STATE", &self.state).env("PATH", path).output();
        out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    }

    /// C-P2, every section from the knobs, the stand-ins and the shim's containers.
    fn preflight(&self, cmd: &str) -> String {
        let k = self.knobs.lock().unwrap().clone();
        let (total, free, available, cached) = self.mem();
        let mut out = String::from("--- mem\n");
        out += &meminfo_line("MemTotal", total);
        out += &meminfo_line("MemFree", free);
        out += &meminfo_line("MemAvailable", available);
        out += &meminfo_line("Cached", cached);
        out += "--- earlyoom\n";
        match &k.earlyoom {
            Some(cfg) => out += &format!("active\n{}\n", cfg.trim_end()),
            None => out += "inactive\ncat: /etc/default/earlyoom: No such file or directory\n",
        }
        out += "--- image\n";
        let image = quoted_after(cmd, "docker image inspect -f '{{.Id}}' ").unwrap_or_default();
        match k.images.iter().find(|(r, i)| *r == image || *i == image) {
            Some((_, id)) => out += &format!("{id}\n"),
            None => out += &format!("Error: No such image: {image}\n"),
        }
        out += "--- store\n";
        out += &format!("{}\n", k.store);
        out += "--- model\n";
        if !k.no_model && quoted_after(cmd, "test -f ").is_some_and(|f| Path::new(&f).is_file()) && !quoted_after(cmd, "test ! -e ").is_some_and(|f| Path::new(&f).exists()) {
            out += "present\n";
        }
        out += "--- port\n";
        // 'sport = :8000', or '( sport = :8000 or sport = :29501 )'.
        let filter = quoted_after(cmd, "ss -Hltn ").unwrap_or_default();
        let ports = filter.split(':').skip(1).filter_map(|w| w.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u16>().ok());
        for p in ports.filter(|p| std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], *p).into(), Duration::from_millis(200)).is_ok()) {
            out += &format!("LISTEN 0      2048       127.0.0.1:{p}        0.0.0.0:*\n");
        }
        out += "--- shm\n";
        out += &self.ls_shm();
        out += "--- caches\n";
        out += &self.ls_caches();
        out += "--- containers\n";
        out += &self.shim(&["ps", "-a", "--filter", "label=sparkbox.deployment", "--format", r#"{{.ID}}|{{.Names}}|{{.Label "sparkbox.deployment"}}|{{.Label "sparkbox.instance"}}|{{.Status}}"#]);
        out += "--- ray\n";
        for line in self.shim(&["ps", "-a", "--no-trunc", "--format", "{{.ID}}|{{.Names}}|{{.Status}}|{{.Command}}"]).lines().filter(|l| l.contains("ray start --address")) {
            out += &format!("{line}\n");
        }
        out += "--- ollama\n";
        if let Some(ps) = &k.ollama_ps {
            out += &format!("{ps}\n");
        }
        if let Some(i) = k.preflight_stops_before.and_then(|s| out.find(&format!("--- {s}\n"))) {
            out.truncate(i);
        }
        out
    }

    fn answer(&self, cmd: &str, stdin: &[u8]) -> Option<(String, u32)> {
        if cmd.starts_with("echo '--- mem'; grep -E '^(MemTotal|MemFree|MemAvailable|Cached):' /proc/meminfo;") {
            return Some((self.preflight(cmd), 0));
        }
        match cmd {
            "grep MemAvailable /proc/meminfo" => Some((meminfo_line("MemAvailable", self.mem().2), 0)),
            "ls -la /dev/shm" => Some((self.ls_shm(), 0)),
            "sudo -k -S -p '' sync; echo 3 > /proc/sys/vm/drop_caches" => {
                // The node asks the script twice: to decide (no input yet), then once sudo took
                // the password, with it. Only the second one dropped anything.
                if !stdin.is_empty() {
                    self.drops.fetch_add(1, Ordering::SeqCst);
                    self.dropped.store(true, Ordering::SeqCst);
                }
                Some((String::new(), 0))
            }
            _ => None,
        }
    }
}

// ---- Helpers for tests that load on a fake cell2

/// A model folder vLLM would load, in the node's home.
pub fn gemma_folder(home: &Path) -> String {
    let dir = home.join("models/google/gemma-3-27b-it");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), r#"{"architectures": ["Gemma3ForConditionalGeneration"]}"#).unwrap();
    dir.display().to_string()
}

/// The Session B load on a fake cell2, at a free port on loopback.
pub fn gemma(home: &Path, id: &str, require_key: bool) -> DeploySpec {
    DeploySpec {
        id: id.into(),
        served_name: "gemma-3-27b-it".into(),
        model_path: gemma_folder(home),
        nodes: vec!["cell2".into()],
        max_model_len: Some(32768),
        gpu_memory_utilization: 0.75,
        port: free_port(),
        host: "127.0.0.1".into(),
        image_ref: GLM53_IMAGE.into(),
        image_id: test_image_id(),
        require_key,
        ..Default::default()
    }
}

/// What a rank's `docker run` needs on its node, as a start puts it there: launch.py and the
/// launch files, through the real sparkjob.py.
pub async fn launch_files(live: &Live, node: &str, spec: &DeploySpec, rank: usize, nets: &[plan::NodeNet], key: Option<&str>) {
    live.ensure_file(node, &plan::launcher_name(), plan::LAUNCH).await.unwrap();
    let args = format!("deploy-files {} {}", spec.id, plan::role_of(spec, rank));
    let payload = plan::launch_payload(spec, rank, nets, key).to_string();
    live.sparkjob(node, &args, Some(payload.as_bytes()), Duration::from_secs(30)).await.unwrap();
}

/// curl on the node: the status of `path` (000 when nothing answers).
pub async fn status(live: &Live, port: u16, path: &str) -> String {
    let cmd = format!("curl -s -o /dev/null -m 3 -w '%{{http_code}}' http://127.0.0.1:{port}{path} || true");
    live.run("cell2", &cmd, None, Duration::from_secs(10)).await.unwrap().stdout.trim().to_string()
}

pub async fn until_healthy(live: &Live, port: u16) {
    for _ in 0..100 {
        if status(live, port, "/health").await == "200" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the stand-in on {port} never answered /health");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_node::temp;

    async fn sh(live: &Live, cmd: &str) -> crate::live::ssh::Output {
        live.run("cell2", cmd, None, Duration::from_secs(60)).await.unwrap()
    }

    /// `spec`'s one rank, started the way a load starts it.
    async fn start(live: &Live, spec: &DeploySpec, key: Option<&str>) -> crate::live::ssh::Output {
        launch_files(live, "cell2", spec, 0, &[], key).await;
        sh(live, &plan::rank_command(spec, 0)).await
    }

    #[tokio::test]
    async fn docker_answers_as_docker_29_and_term_cleans_up_where_kill_does_not() {
        let home = temp("vllm-shim");
        let (fake, port, s) = node(&home, Knobs::default()).await;
        let dir = temp("vllm-shim-app");
        let (live, _) = fake_node::connect(&dir, &[("cell2", port)]).await;

        // The image by name, and one this node doesn't have.
        let out = sh(&live, &format!("docker image inspect -f '{{{{.Id}}}}' '{GLM53_IMAGE}'")).await;
        assert_eq!((out.stdout.trim(), out.code), (test_image_id().as_str(), Some(0)));
        let out = sh(&live, "docker image inspect -f '{{.Id}}' 'nvcr.io/nvidia/vllm:26.05-py3'").await;
        assert_eq!((out.stdout.as_str(), out.stderr.trim(), out.code), ("", "Error: No such image: nvcr.io/nvidia/vllm:26.05-py3", Some(1)));

        // C-L2 without a key: launch.py supervises the stand-in, serving on the port its argv names.
        let spec = gemma(&home, "t1", false);
        let run = plan::rank_command(&spec, 0);
        let out = start(&live, &spec, None).await;
        assert_eq!(out.code, Some(0), "{}", out.stderr);
        let id = out.stdout.trim().to_string();
        assert!(id.len() == 64 && id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        assert!(fake.state_dir.join("docker/sparkbox-t1.started").exists(), "`run -d` returned once it ran in a session of its own");
        let again = sh(&live, &run).await;
        assert_eq!(again.code, Some(125));
        assert!(again.stderr.contains("Conflict. The container name \"/sparkbox-t1\" is already in use by container"), "{}", again.stderr);
        until_healthy(&live, spec.port).await;
        assert_eq!(s.running(), ["sparkbox-t1"]);
        let argv = s.argv("sparkbox-t1");
        assert_eq!(&argv[..3], ["run", "-d", "--name"]);
        assert!(argv.windows(2).any(|w| w == ["--entrypoint", "python3"]) && argv.contains(&test_image_id()), "{argv:?}");
        assert_eq!(argv.last().unwrap(), "/run/sparkbox/launch.py", "the container's own path, nothing after it: {argv:?}");
        let seen = s.seen("sparkbox-t1");
        let words: Vec<&str> = seen["argv"].as_array().unwrap().iter().filter_map(|w| w.as_str()).collect();
        assert!(words.windows(2).any(|w| w == ["--port", spec.port.to_string().as_str()]) && words[..2] == ["serve", spec.model_path.as_str()], "each word intact, through launch.py: {words:?}");
        assert_eq!((seen["hasKey"].as_bool(), seen["values"]["HF_HUB_OFFLINE"].as_str()), (Some(false), Some("1")), "{seen}");
        assert_eq!(seen["values"].get("SB_FAKE_STATE"), None, "only the container's environment: {seen}");

        // ps, inspect and logs as SparkBox reads them.
        let short = &id[..12];
        let me = crate::identity::instance_name();
        assert_eq!(sh(&live, &format!("docker ps -aq --filter label=sparkbox.instance={me} --filter label=sparkbox.deployment=t1")).await.stdout, format!("{short}\n"));
        assert_eq!(sh(&live, "docker ps -aq --filter label=sparkbox.deployment=other").await.stdout, "");
        assert_eq!(sh(&live, &plan::load_containers_command("t1")).await.stdout, format!("{short} {me}\n"));
        let probe = sh(&live, &plan::probe_command("sparkbox-t1", "127.0.0.1", spec.port)).await.stdout;
        assert!(probe.starts_with("running 0 0\n200\n") && probe.contains("Application startup complete."), "{probe}");
        assert_eq!(plan::stage_from_logs(&probe, false), plan::Stage::Ready);
        let listed = sh(&live, r#"docker ps -a --filter label=sparkbox.deployment --format '{{.ID}}|{{.Names}}|{{.Label "sparkbox.deployment"}}|{{.Label "sparkbox.instance"}}|{{.Status}}'"#).await.stdout;
        assert_eq!(listed, format!("{short}|sparkbox-t1|t1|{me}|Up 2 minutes\n"));
        assert_eq!(s.shm().len(), 1, "vLLM's segment in /dev/shm while it runs");

        // rm refuses a running container; stop TERMs it, and it cleans up after itself.
        let out = sh(&live, "docker rm sparkbox-t1").await;
        assert_eq!(out.code, Some(1));
        assert_eq!(out.stderr.trim(), "Error response from daemon: cannot remove container \"/sparkbox-t1\": container is running: stop the container before removing or force remove");
        assert_eq!(sh(&live, &format!("docker stop -t 30 {short}")).await.stdout, format!("{short}\n"));
        assert_eq!(sh(&live, "docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' sparkbox-t1").await.stdout, "exited 0\n");
        assert!(s.shm().is_empty(), "TERM leaves nothing in /dev/shm: {:?}", s.shm());
        assert!(s.log("sparkbox-t1").contains("Application shutdown complete."));
        assert_eq!(sh(&live, "docker rm sparkbox-t1").await.stdout, "sparkbox-t1\n");
        let out = sh(&live, "docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' sparkbox-t1").await;
        assert_eq!((out.stderr.trim(), out.code), ("Error: No such object: sparkbox-t1", Some(1)));
        assert_eq!(status(&live, spec.port, "/health").await, "000", "the port is closed");

        // rm -f KILLs it: the segment stays behind.
        assert_eq!(sh(&live, &run).await.code, Some(0), "the name is free again");
        until_healthy(&live, spec.port).await;
        assert_eq!(sh(&live, "docker rm -f sparkbox-t1").await.code, Some(0));
        assert!(s.running().is_empty());
        assert_eq!(s.shm().len(), 1, "a KILL leaves vLLM's segment in /dev/shm");
        assert_eq!(sh(&live, "docker rm -f sparkbox-gone").await.code, Some(0), "rm -f of nothing is fine");

        // Killed outright (an OOM kill): Docker says exited 137, and its segment stays too.
        assert_eq!(sh(&live, &run).await.code, Some(0));
        until_healthy(&live, spec.port).await;
        s.kill("sparkbox-t1");
        for _ in 0..50 {
            if s.running().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(sh(&live, "docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' sparkbox-t1").await.stdout, "exited 137\n");
        assert_eq!(s.shm().len(), 2, "{:?}", s.shm());
        live.stop();
    }

    /// `--restart unless-stopped`, as real Docker 29 did on this Mac: every look reads running, the
    /// exit code reads 0, and only RestartCount rises. A stop ends it, and it stays ended.
    #[tokio::test]
    async fn the_restart_policy_is_honoured_until_a_stop() {
        let home = temp("vllm-policy");
        let (_node, port, s) = node(&home, Knobs::default()).await;
        let (live, _) = fake_node::connect(&temp("vllm-policy-app"), &[("cell2", port)]).await;
        let spec = DeploySpec { keep_running: true, ..gemma(&home, "p1", false) };
        let out = start(&live, &spec, None).await;
        assert_eq!(out.code, Some(0), "{}", out.stderr);
        assert!(s.argv("sparkbox-p1").windows(2).any(|w| w == ["--restart", "unless-stopped"]));
        let first = s.started_ms("sparkbox-p1").unwrap();
        // Crash only after each process has reached the server. A 200 ms self-destruct plus
        // a four-second overall wait was also timing two Python interpreter startups.
        for count in 1..=2 {
            until_healthy(&live, spec.port).await;
            let pid = s.inner.pid("sparkbox-p1").unwrap();
            s.kill("sparkbox-p1");
            fake_node::wait_for("a new container after the crash", || {
                (s.restarts("sparkbox-p1") == count && s.inner.pid("sparkbox-p1").is_some_and(|p| p != pid)).then_some(())
            }, || s.diagnostic("sparkbox-p1")).await;
        }
        until_healthy(&live, spec.port).await;
        assert_eq!(s.restarts("sparkbox-p1"), 2);
        assert!(s.started_ms("sparkbox-p1").unwrap() > first);
        let state = sh(&live, "docker inspect -f '{{.State.Status}} {{.State.ExitCode}} {{.RestartCount}}' sparkbox-p1").await.stdout;
        let words: Vec<&str> = state.split_whitespace().collect();
        assert_eq!(words, ["running", "0", "2"], "{state}");
        assert_eq!(s.log("sparkbox-p1").matches("vLLM API server version").count(), 3, "initial run plus two restarts, in one log: {}", s.log("sparkbox-p1"));

        assert_eq!(sh(&live, "docker stop -t 30 sparkbox-p1").await.code, Some(0));
        let ended = s.restarts("sparkbox-p1");
        fake_node::wait_for("the stopped container and its daemon to exit", || s.processes().is_empty().then_some(()), || s.diagnostic("sparkbox-p1")).await;
        assert_eq!(s.restarts("sparkbox-p1"), ended, "stopped by hand, it isn't started again");
        assert!(s.running().is_empty());
        assert_eq!(sh(&live, "docker rm sparkbox-p1").await.code, Some(0));
        assert!(s.processes().is_empty(), "the daemon's part ended with it");
        live.stop();
    }

    #[tokio::test]
    async fn docker_run_waits_for_actual_startup_even_after_five_seconds() {
        let home = temp("vllm-start-held");
        let (_node, port, s) = node(&home, Knobs::default()).await;
        let (live, _) = fake_node::connect(&temp("vllm-start-held-app"), &[("cell2", port)]).await;
        let spec = gemma(&home, "start-held", false);
        launch_files(&live, "cell2", &spec, 0, &[], None).await;
        let gate = s.docker_dir().join("hold-start");
        std::fs::write(&gate, "").unwrap();
        let l2 = live.clone();
        let command = plan::rank_command(&spec, 0);
        let run = tokio::spawn(async move { l2.run("cell2", &command, None, Duration::from_secs(60)).await });
        fake_node::wait_for("the daemon's startup gate", || s.docker_dir().join("sparkbox-start-held.start-held").exists().then_some(()), || s.diagnostic("sparkbox-start-held")).await;
        // Specifically cross the old five-second fallback; it must not invent success.
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert!(!run.is_finished(), "docker run acknowledged a container that hasn't started");
        assert!(s.running().is_empty() && s.started_ms("sparkbox-start-held").is_none());
        std::fs::remove_file(gate).unwrap();
        let out = run.await.unwrap().unwrap();
        assert_eq!(out.code, Some(0), "{}", out.stderr);
        assert!(s.started_ms("sparkbox-start-held").is_some());
        until_healthy(&live, spec.port).await;
        assert_eq!(sh(&live, "docker stop -t 30 sparkbox-start-held").await.code, Some(0));
        live.stop();
    }

    #[tokio::test]
    async fn docker_run_reports_daemon_failure_before_startup() {
        let home = temp("vllm-start-error");
        let (node, port, s) = node(&home, Knobs::default()).await;
        let (live, _) = fake_node::connect(&temp("vllm-start-error-app"), &[("cell2", port)]).await;
        let spec = gemma(&home, "start-error", false);
        launch_files(&live, "cell2", &spec, 0, &[], None).await;
        // Files are staged; fail the interpreter that would launch the container daemon.
        node.bin("python3", "exit 7\n");
        let out = sh(&live, &plan::rank_command(&spec, 0)).await;
        assert_eq!(out.code, Some(125), "{out:?}");
        assert!(out.stdout.is_empty(), "a failed daemon must not return a container id");
        assert!(out.stderr.contains("exited before startup (status 7)"), "{}", out.stderr);
        assert!(s.started_ms("sparkbox-start-error").is_none() && s.running().is_empty());
        live.stop();
    }

    /// A start that fails leaves the container Created, with nothing in its log, as Docker Desktop did.
    #[tokio::test]
    async fn a_failed_start_leaves_a_created_container() {
        let home = temp("vllm-created");
        let said = "Error response from daemon: could not select device driver \"\" with capabilities: [gpu]";
        let (_node, port, s) = node(&home, Knobs { run_fails: Some(said.into()), ..Knobs::default() }).await;
        let (live, _) = fake_node::connect(&temp("vllm-created-app"), &[("cell2", port)]).await;
        let spec = gemma(&home, "c1", false);
        let out = start(&live, &spec, None).await;
        assert_eq!((out.code, out.stdout.as_str()), (Some(125), ""));
        assert_eq!(out.stderr, format!("docker: {said}\n\nRun 'docker run --help' for more information\n"));
        assert!(s.running().is_empty());
        assert_eq!(sh(&live, "docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' sparkbox-c1").await.stdout, "created 128\n");
        assert_eq!(sh(&live, "docker logs --tail 600 sparkbox-c1").await.stdout, "");
        let listed = sh(&live, r#"docker ps -a --filter label=sparkbox.deployment --format '{{.ID}}|{{.Names}}|{{.Label "sparkbox.deployment"}}|{{.Label "sparkbox.instance"}}|{{.Status}}'"#).await.stdout;
        assert!(listed.ends_with("|Created\n"), "{listed}");
        assert_eq!(sh(&live, &plan::unload_command("c1")).await.stdout, "ok\n");
        assert_eq!(sh(&live, "docker ps -aq --filter label=sparkbox.deployment=c1").await.stdout, "");
        live.stop();
    }

    #[tokio::test]
    async fn the_key_comes_from_the_mounted_file_through_launch_py_and_never_from_argv() {
        let home = temp("vllm-key");
        let (node, port, s) = node(&home, Knobs::default()).await;
        let dir = temp("vllm-key-app");
        let (live, _) = fake_node::connect(&dir, &[("cell2", port)]).await;
        let spec = gemma(&home, "k1", true);
        let out = start(&live, &spec, Some("sbk-test-key")).await;
        assert_eq!(out.code, Some(0), "{}", out.stderr);
        until_healthy(&live, spec.port).await;
        let argv = s.argv("sparkbox-k1");
        assert!(!argv.iter().any(|w| w.contains("sbk-test-key")), "{argv:?}");
        assert_eq!(argv[argv.len() - 2..], ["/run/sparkbox/launch.py", "--key"], "{argv:?}");
        assert!(argv.contains(&format!("{}/.sparkbox/keys/k1:/run/sparkbox/key:ro", node.home.display())), "$HOME expanded: {argv:?}");
        assert_eq!(s.seen("sparkbox-k1")["hasKey"].as_bool(), Some(true), "launch.py put it in vLLM's environment");
        assert_eq!(status(&live, spec.port, "/v1/models").await, "401", "no key, no answer");
        let models = format!("curl -s -m 10 -H @- -w '\\n%{{http_code}}' http://127.0.0.1:{}/v1/models", spec.port);
        let out = live.run("cell2", &models, Some(b"Authorization: Bearer sbk-test-key\n"), Duration::from_secs(10)).await.unwrap();
        assert!(out.stdout.ends_with("\n200") && out.stdout.contains("\"id\": \"gemma-3-27b-it\"") && out.stdout.contains("\"max_model_len\": 32768"), "{}", out.stdout);
        // /health, /metrics and /openapi.json aren't behind the key, as in vLLM.
        assert_eq!(status(&live, spec.port, "/metrics").await, "200");
        assert_eq!(status(&live, spec.port, "/openapi.json").await, "200");
        live.stop();
    }

    /// The launcher's refusals are real here: a key file gone before the run is a folder in the
    /// container (`-v` makes it), and launch.py exits 78 with its one line.
    #[tokio::test]
    async fn a_key_file_that_went_is_a_folder_in_the_container_and_launch_py_refuses_it() {
        let home = temp("vllm-nokey");
        let (node, port, s) = node(&home, Knobs { key_file_goes: true, ..Knobs::default() }).await;
        let (live, _) = fake_node::connect(&temp("vllm-nokey-app"), &[("cell2", port)]).await;
        let spec = gemma(&home, "k2", true);
        let out = start(&live, &spec, Some("sbk-test-key")).await;
        assert_eq!(out.code, Some(0), "docker run -d doesn't wait for what the container does: {}", out.stderr);
        assert!(node.home.join(".sparkbox/keys/k2").is_dir());
        for _ in 0..100 {
            if s.running().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(sh(&live, "docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' sparkbox-k2").await.stdout, format!("exited {}\n", plan::KEY_REFUSED));
        assert_eq!(s.log("sparkbox-k2"), "sparkbox: the API key file is missing or empty, so vLLM was not started.\n");
        live.stop();
    }

    /// Two fake nodes, one load: a worker waits for its head, the head can't turn healthy until
    /// every rank is there, and a rank that goes takes the others with it, exit 1.
    #[tokio::test]
    async fn ranks_on_two_fake_nodes_really_meet_and_a_lost_rank_takes_the_other_down() {
        let meet = temp("vllm-meet");
        std::fs::create_dir_all(&meet).unwrap();
        let knobs = Knobs { rendezvous: Some(meet.clone()), ..Knobs::default() };
        let (h1, h2) = (temp("vllm-meet-h1"), temp("vllm-meet-h2"));
        let (_n1, p1, head) = node(&h1, knobs.clone()).await;
        let (_n2, p2, worker) = node(&h2, knobs).await;
        let (live, _) = fake_node::connect(&temp("vllm-meet-app"), &[("cell1", p1), ("cell2", p2)]).await;
        let spec = DeploySpec { nodes: vec!["cell1".into(), "cell2".into()], tensor_parallel: 2, docker_access: "privileged".into(), ..gemma(&h1, "m2", false) };
        gemma_folder(&h2);
        let spec = DeploySpec { model_path: spec.model_path.clone(), ..spec };
        let net = |id: &str, n: u8| plan::NodeNet { node_id: id.into(), iface: "enp1s0f1np1".into(), ip: format!("192.168.100.{n}"), hcas: vec!["rocep1s0f1".into(), "roceP2p1s0f1".into()] };
        let nets = [net("cell1", 1), net("cell2", 2)];
        // Each rank mounts its node's copy, at the path every rank is told.
        std::fs::create_dir_all(Path::new(&spec.model_path)).unwrap();
        let on = |node: &'static str, rank: usize| {
            let (live, spec, nets) = (live.clone(), spec.clone(), nets.clone());
            async move {
                launch_files(&live, node, &spec, rank, &nets, None).await;
                let out = live.run(node, &plan::rank_command(&spec, rank), None, Duration::from_secs(60)).await.unwrap();
                assert_eq!(out.code, Some(0), "{}", out.stderr);
            }
        };
        on("cell2", 1).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(worker.running(), ["sparkbox-m2-r1"], "it waits for rank 0: {}", worker.log("sparkbox-m2-r1"));
        assert!(!worker.log("sparkbox-m2-r1").contains("Loading model from scratch"), "{}", worker.log("sparkbox-m2-r1"));
        on("cell1", 0).await;
        let asked = format!("curl -s -o /dev/null -m 3 -w '%{{http_code}}' http://127.0.0.1:{}/health || true", spec.port);
        let mut healthy = false;
        for _ in 0..100 {
            if live.run("cell1", &asked, None, Duration::from_secs(10)).await.unwrap().stdout.trim() == "200" {
                healthy = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(healthy, "every rank is here: {}", head.log("sparkbox-m2-r0"));
        let log = head.log("sparkbox-m2-r0");
        assert!(log.contains("mq_connect_ip=192.168.100.1 (local), world_size=2") && log.contains("vLLM is using nccl==2.30.7") && log.contains("NET/IB : Using [0]rocep1s0f1:1/RoCE [1]roceP2p1s0f1:1/RoCE"), "{log}");
        let seen = head.seen("sparkbox-m2-r0");
        assert_eq!(seen["values"]["VLLM_HOST_IP"].as_str(), Some("192.168.100.1"), "{seen}");

        // The worker goes: the head loses it and exits 1, as vLLM's engine does.
        worker.kill("sparkbox-m2-r1");
        for _ in 0..100 {
            if head.running().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(head.running().is_empty(), "{}", head.log("sparkbox-m2-r0"));
        let probe = live.run("cell1", "docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' sparkbox-m2-r0", None, Duration::from_secs(10)).await.unwrap();
        assert_eq!(probe.stdout, "exited 1\n");
        assert!(head.log("sparkbox-m2-r0").contains("Connection to rank 1 was lost"), "{}", head.log("sparkbox-m2-r0"));
        live.stop();
    }

    /// Nothing falls back to a pretend load: several ranks with nowhere to meet is an error.
    #[tokio::test]
    async fn several_ranks_with_no_rendezvous_folder_fail() {
        let home = temp("vllm-alone");
        let (_node, port, s) = node(&home, Knobs::default()).await;
        let (live, _) = fake_node::connect(&temp("vllm-alone-app"), &[("cell2", port)]).await;
        let spec = DeploySpec { nodes: vec!["cell2".into(), "cell3".into()], tensor_parallel: 2, ..gemma(&home, "a2", false) };
        let net = plan::NodeNet { node_id: "cell2".into(), iface: "enp1s0f1np1".into(), ip: "192.168.100.2".into(), hcas: vec!["rocep1s0f1".into()] };
        launch_files(&live, "cell2", &spec, 0, &[net], None).await;
        assert_eq!(sh(&live, &plan::rank_command(&spec, 0)).await.code, Some(0));
        for _ in 0..100 {
            if s.running().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(sh(&live, "docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' sparkbox-a2-r0").await.stdout, "exited 1\n");
        assert!(s.log("sparkbox-a2-r0").contains("no rendezvous folder"), "{}", s.log("sparkbox-a2-r0"));
        live.stop();
    }

    #[tokio::test]
    async fn memory_and_shm_follow_the_stand_in_and_the_drop_needs_sudo() {
        let home = temp("vllm-mem");
        let (_node, port, s) = node(&home, Knobs { mem_free: 41 * GIB, mem_cached: 75 * GIB, ..Knobs::default() }).await;
        let dir = temp("vllm-mem-app");
        let (live, _) = fake_node::connect(&dir, &[("cell2", port)]).await;
        let spec = gemma(&home, "m1", false);
        std::fs::create_dir_all(home.join(".cache/vllm")).unwrap();

        // The whole pre-flight read, as its parser reads it.
        let out = sh(&live, &plan::preflight_script(&spec)).await;
        let p = plan::parse_preflight("cell2", &spec, &out.stdout);
        let m = p.mem.as_ref().expect("meminfo");
        assert_eq!((m.total, m.free, m.available, m.cached), (127_598_748 * 1024, 41 * GIB, 122_601_504 * 1024, 75 * GIB));
        assert!(p.drop_needed, "41 GiB free is short of 0.75 × 121.7 + 4");
        assert_eq!(p.image_id.as_deref(), Some(test_image_id().as_str()));
        assert!(p.model_present && p.port_free, "{p:?}");
        assert!(p.shm.is_empty(), "{:?}", p.shm);
        assert!(out.stdout.contains(&format!("ls: cannot access '{}/.triton': No such file or directory", home.display())), "{}", out.stdout);
        assert_eq!(p.caches.iter().filter(|c| c.exists).count(), 1, "{:?}", p.caches);
        assert_eq!(p.store.as_deref(), Some("overlay2"), "{}", out.stdout);
        assert!(p.leftovers.is_empty() && p.ray_leftovers.is_empty() && p.ollama_resident.is_empty(), "{p:?}");

        // The drop: only through sudo with the password; MemFree rises to about MemAvailable.
        live.sudo("cell2", plan::DROP_CACHES, Duration::from_secs(20)).await.unwrap();
        assert_eq!(s.drops(), 1);
        let p = plan::parse_preflight("cell2", &spec, &sh(&live, &plan::preflight_script(&spec)).await.stdout);
        assert!(!p.drop_needed && p.mem.unwrap().free > 110 * GIB);

        // Loaded: MemAvailable is down by the load, the port is taken, the segment is there.
        let before = sh(&live, "grep MemAvailable /proc/meminfo").await.stdout;
        assert_eq!(before, "MemAvailable:   122601504 kB\n");
        start(&live, &spec, None).await;
        until_healthy(&live, spec.port).await;
        let loaded = sh(&live, "grep MemAvailable /proc/meminfo").await.stdout;
        assert_eq!(loaded, format!("MemAvailable:   {} kB\n", (122_601_504 * 1024 - Knobs::default().load_bytes) / 1024));
        let p = plan::parse_preflight("cell2", &spec, &sh(&live, &plan::preflight_script(&spec)).await.stdout);
        assert!(!p.port_free, "{p:?}");
        assert_eq!((p.leftovers.len(), p.leftovers[0].name.as_str(), p.leftovers[0].deployment.as_str(), p.leftovers[0].ours), (1, "sparkbox-m1", "m1", true));
        let shm = sh(&live, "ls -la /dev/shm").await.stdout;
        assert!(shm.starts_with("total 4\ndrwxrwxrwt") && shm.lines().last().unwrap().ends_with(&s.shm()[0]), "{shm}");

        // Unloaded (C-U): it all comes back.
        assert_eq!(sh(&live, &plan::unload_command("m1")).await.stdout, "ok\n");
        assert_eq!(sh(&live, "grep MemAvailable /proc/meminfo").await.stdout, before);
        assert!(s.shm().is_empty());
        live.stop();
    }

    /// `docker run -d` leaves the container to dockerd: the SSH session that ran it ending at once,
    /// even before `docker run` has returned, doesn't stop it.
    #[tokio::test]
    async fn a_container_outlives_the_session_that_ran_it() {
        for i in 0..8 {
            let home = temp(&format!("vllm-sess{i}"));
            let (node, port, s) = node(&home, Knobs::default()).await;
            let (live, _) = fake_node::connect(&temp(&format!("vllm-sess{i}-app")), &[("cell2", port)]).await;
            let spec = gemma(&home, &format!("s{i}"), false);
            launch_files(&live, "cell2", &spec, 0, &[], None).await;
            let run = plan::rank_command(&spec, 0);
            let l2 = live.clone();
            let h = tokio::spawn(async move { l2.run("cell2", &run, None, Duration::from_secs(60)).await });
            // The connection goes (sshd hangs up on the session: HUP, then TERM) as soon as the
            // container's process exists.
            let name = format!("sparkbox-s{i}");
            fake_node::wait_for("the detached container before dropping SSH", || s.inner.pid(&name).map(|_| ()), || s.diagnostic(&name)).await;
            node.drop_connections().await;
            let _ = h.await;
            tokio::time::sleep(Duration::from_millis(800)).await;
            assert_eq!(s.running(), [name.as_str()], "round {i}: it went with the session");
            assert!(!node.state_dir.join("docker").join(format!("{name}.exit")).exists(), "round {i}: it never exited");
            // A new connection finds it serving.
            live.stop();
            let (again, _) = fake_node::connect(&temp(&format!("vllm-sess{i}-again")), &[("cell2", port)]).await;
            until_healthy(&again, spec.port).await;
            assert_eq!(sh(&again, &format!("docker inspect -f '{{{{.State.Status}}}} {{{{.State.ExitCode}}}}' {name}")).await.stdout, "running 0\n");
            again.stop();
        }
    }

    #[tokio::test]
    async fn dropping_the_stand_in_leaves_no_process_or_state() {
        let home = temp("vllm-drop");
        let (node, port, s) = node(&home, Knobs::default()).await;
        let dir = temp("vllm-drop-app");
        let (live, _) = fake_node::connect(&dir, &[("cell2", port)]).await;
        let spec = gemma(&home, "d1", false);
        start(&live, &spec, None).await;
        until_healthy(&live, spec.port).await;
        // And a container that serves nothing (a Ray worker's).
        sh(&live, &format!("docker run -d --name sparkbox-d1-w --label sparkbox.deployment=d1 --entrypoint bash {} -c 'until ray start --block; do sleep 3; done'", test_image_id())).await;
        assert_eq!(s.running(), ["sparkbox-d1", "sparkbox-d1-w"]);
        let pids = s.processes();
        assert_eq!(pids.len(), 5, "two daemons, the vLLM launcher and engine, and the idle container");
        drop(s);
        assert!(pids.iter().all(|p| !is_standin(*p, &node.state_dir)), "no fixture process outlives the test");
        assert!(!node.state_dir.join("docker").exists() && !node.state_dir.join("shm").exists());
        assert_eq!(status(&live, spec.port, "/health").await, "000");
        live.stop();
    }
}
