#!/bin/sh
# Contracts C-A, C-F and C-L2's proof against real Docker Desktop (P2 round 1): a load started the way
# SparkBox starts one has no shell in its path, and its API key stays out of Docker's records. The
# stand-in in the staging node can't show this: it runs nothing, and only real Docker writes a
# container's config, runs an entrypoint and delivers docker stop's TERM.
#
#   scripts/staging/launch-proof.sh
#
# It runs the shipped sparkjob.py deploy-files and the shipped launch.py, and C-L2 as
# plan::rank_command_for writes it, minus --gpus all and --privileged: Docker Desktop has no GPU. The
# two `docker run` lines below are compared with Rust's by the `launch_proof_sends_what_rust_writes`
# test, so they can't drift. vLLM isn't here: `vllm` in the image is a small Python program that
# records its argv, its environment and its pid, and traps TERM. No real model, no network, no GPU.
#
# Uses only sparkbox-staging-* names, all removed when it ends, however it ends, with the throwaway
# image, home folder and key. HOME is that folder, so nothing touches the real ~/.sparkbox. The key is
# a random fake string, and nothing here sends it anywhere. setup.sh's sparkbox-staging-spark1 stays
# up, as after any staging run: teardown.sh removes it.
#
#   1. deploy-files, on real files: its modes, and that it prints nothing of the payload.
#   2. A one-node load's C-L2 with --key, by image id. The process sees the key and C-L2's limits.
#   3. PID 1 supervises vllm as its child; Docker still records launch.py as the command,
#      which is why discovery reads labels.
#   4. docker inspect doesn't contain the key. As a control, it does with -e, so this check can fail.
#   5. A quoted JSON flag arrives as one element.
#   6. docker stop -t 5 forwards TERM. As a control, an engine ignoring TERM takes 5 s, then 137.
#   7. A missing key and an empty key each exit 78, with launch.py's line and nothing started.
#   8. No --key leaves no VLLM_API_KEY, even with one baked into the image. As a control, the image
#      really has one.
#
# Exit 0: every step ran and passed. 1: a step failed. 2: Docker Desktop isn't available.
set -eu
unset DOCKER_HOST
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
SPARKJOB=$REPO/src-tauri/src/live/sparkjob.py
LAUNCH=$REPO/src-tauri/src/live/launch.py
BASE=sparkbox-staging:24.04
PROOF=sparkbox-staging-launch:throwaway
LOAD=staging-launch
SINGLE=sparkbox-staging-launch
WORKER=sparkbox-staging-launch-r1
CONTROL=sparkbox-staging-launch-control
BAKED=sbk-baked-into-the-image

docker() { command docker --context desktop-linux "$@"; }

tmp=${TMPDIR:-/tmp}
work=$(mktemp -d "${tmp%/}/sparkbox-staging-launch-XXXXXXXX")
cleanup() {
  for c in "$SINGLE" "$WORKER" "$CONTROL"; do
    docker rm -f "$c" >/dev/null 2>&1 || true
  done
  docker image rm "$PROOF" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 1' INT TERM HUP

fail() {
  echo "FAILED: $1" >&2
  exit 1
}

# The staging image, built if it isn't there. setup.sh starts Docker Desktop when it isn't up. It also
# starts the node's sshd, which a staging test may have stopped on purpose, so it waits for the
# container's use lock, as check.sh does. Nothing after it touches that container.
exec 9>>"${tmp%/}/sparkbox-staging-spark1.lock"
lockf -s -t 900 9 || fail "sparkbox-staging-spark1 was still in use after 15 minutes"
status=0
"$HERE/setup.sh" >"$work/setup.log" 2>&1 || status=$?
exec 9>&-
[ "$status" = 2 ] && { echo "Docker Desktop isn't up; start it by hand and re-run. Nothing proved."; exit 2; }
[ "$status" = 0 ] || { tail -5 "$work/setup.log" >&2; fail "setup.sh exited $status"; }

# The image: staging's Python 3.12 and the stand-in vllm, on the PATH launch.py searches. Each run
# writes to the cache folder C-L2 mounts, so there's no mount here C-L2 doesn't have.
mkdir "$work/image"
cat > "$work/image/vllm" <<'PROGRAM'
#!/usr/bin/python3
import hashlib, json, os, signal, sys, time
run = os.environ["VLLM_PROOF_RUN"]
folder = "/root/.cache/vllm"
key = os.environ.get("VLLM_API_KEY")
record = {"pid": os.getpid(), "parent": os.getppid(), "argv": sys.argv[1:], "env": {k: v for k, v in os.environ.items() if k != "VLLM_API_KEY"},
          "key": hashlib.sha256(key.encode()).hexdigest() if key is not None else None, "ulimits": os.popen("ulimit -l; ulimit -s; ulimit -n").read().split()}
def termed(*_):
    open(os.path.join(folder, run + ".term"), "w").close()
    sys.exit(0)
if "VLLM_PROOF_NO_HANDLER" not in os.environ:
    signal.signal(signal.SIGTERM, termed)
else:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
with open(os.path.join(folder, run + ".tmp"), "w") as f:
    json.dump(record, f)
os.replace(os.path.join(folder, run + ".tmp"), os.path.join(folder, run + ".json"))
while True:
    time.sleep(1)
PROGRAM
cat > "$work/image/Dockerfile" <<DOCKERFILE
FROM $BASE
COPY --chmod=0755 vllm /usr/local/bin/vllm
ENV VLLM_API_KEY=$BAKED
LABEL sparkbox.staging=1
STOPSIGNAL SIGTERM
DOCKERFILE
docker build -q --platform linux/arm64 -t "$PROOF" "$work/image" >/dev/null
IMAGE_ID=$(docker image inspect -f '{{.Id}}' "$PROOF")
case "$IMAGE_ID" in
  sha256:*) [ "${#IMAGE_ID}" = 71 ] || fail "the image's id isn't sha256:<64 hex>: $IMAGE_ID" ;;
  *) fail "the image's id isn't sha256:<64 hex>: $IMAGE_ID" ;;
esac

# Everything below reads $HOME the way C-L2 does on a node, and it's a folder of this run's.
export DOCKER_CONFIG="${DOCKER_CONFIG:-$HOME/.docker}"
export HOME="$work/home"
mkdir -p "$HOME/.sparkbox"
LAUNCH_NAME="launch-$(shasum -a 256 "$LAUNCH" | awk '{print $1}').py"
cp "$LAUNCH" "$HOME/.sparkbox/$LAUNCH_NAME"
MODEL="$work/models/org/sbproof"
mkdir -p "$MODEL"
printf '{"architectures": ["Sbproof"]}' > "$MODEL/config.json"
key="sbk-$(LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 40)"
key_file="$HOME/.sparkbox/keys/$LOAD"
deploy_dir="$HOME/.sparkbox/deploy/$LOAD"
# The JSON flag a shell would have broken: quotes, spaces and a dollar.
FLAG='{"method": "mtp", "num_speculative_tokens": 4, "note": "two words $HOME"}'

# deploy <role> <run> <key or nothing> [a variable to set]: C-F through the shipped file.
deploy() {
  role=$1 run=$2 extra=${4:-}
  PROOF_KEY=$3 python3 - "$role" "$run" "$extra" "$MODEL" "$FLAG" <<'PAYLOAD' | python3 -I "$SPARKJOB" deploy-files "$LOAD" "$role" >"$work/deploy.out" 2>"$work/deploy.err" || fail "deploy-files: $(tail -1 "$work/deploy.err")"
import json, os, sys
role, run, extra, model, flag = sys.argv[1:]
argv = ["vllm", "serve", model, "--served-model-name", "sbproof", "--gpu-memory-utilization", "0.75", "--speculative-config", flag]
argv += ["--headless"] if role == "worker" else ["--host", "0.0.0.0", "--port", "18999"]
env = {"HF_HUB_OFFLINE": "1", "VLLM_ENGINE_READY_TIMEOUT_S": "3600", "VLLM_PROOF_RUN": run}
if extra:
    env[extra] = "1"
print(json.dumps({"argv": argv, "env": env, "key": os.environ["PROOF_KEY"] or None}))
PAYLOAD
}

# The two rank commands, each a line plan::rank_command_for writes, with $MODEL and $IMAGE_ID for the
# spec's folder and image. Don't change one without Rust: `launch_proof_sends_what_rust_writes`.
single_rank() {
  docker run -d --name sparkbox-staging-launch --label sparkbox.instance=dev --label sparkbox.deployment=staging-launch --label sparkbox.group=staging-launch --label sparkbox.role=single --label sparkbox.rank=0 --label sparkbox.model=sbproof --label sparkbox.recipe=generic --label sparkbox.image=sparkbox-staging-launch:throwaway --label sparkbox.path=$MODEL --label sparkbox.parallel=1x1 --label sparkbox.gpu-share=0.75 --label sparkbox.port=18999 --label sparkbox.listen=0.0.0.0 --label sparkbox.maxlen=32768 --network host --ipc host --ulimit memlock=-1 --ulimit stack=67108864 --ulimit nofile=1048576:1048576 --restart no -v $MODEL:$MODEL:ro -v "$HOME/.cache/vllm:/root/.cache/vllm" -v "$HOME/.cache/flashinfer:/root/.cache/flashinfer" -v "$HOME/.triton:/root/.triton" -v "$HOME/.sparkbox/deploy/staging-launch:/run/sparkbox/deploy:ro" -v "$HOME/.sparkbox/$LAUNCH_NAME:/run/sparkbox/launch.py:ro" -v "$HOME/.sparkbox/keys/staging-launch:/run/sparkbox/key:ro" --entrypoint python3 $IMAGE_ID /run/sparkbox/launch.py --key
}
worker_rank() {
  docker run -d --name sparkbox-staging-launch-r1 --label sparkbox.instance=dev --label sparkbox.deployment=staging-launch --label sparkbox.group=staging-launch --label sparkbox.role=worker --label sparkbox.rank=1 --label sparkbox.model=sbproof --label sparkbox.recipe=generic --label sparkbox.image=sparkbox-staging-launch:throwaway --label sparkbox.path=$MODEL --label sparkbox.parallel=2x1 --label sparkbox.gpu-share=0.75 --label sparkbox.maxlen=32768 --network host --ipc host --ulimit memlock=-1 --ulimit stack=67108864 --ulimit nofile=1048576:1048576 --restart no -v $MODEL:$MODEL:ro -v "$HOME/.cache/vllm:/root/.cache/vllm" -v "$HOME/.cache/flashinfer:/root/.cache/flashinfer" -v "$HOME/.triton:/root/.triton" -v "$HOME/.sparkbox/deploy/staging-launch:/run/sparkbox/deploy:ro" -v "$HOME/.sparkbox/$LAUNCH_NAME:/run/sparkbox/launch.py:ro" --entrypoint python3 $IMAGE_ID /run/sparkbox/launch.py
}

# record <run> <python expression over r>: a fact from what the stand-in vllm wrote.
record() {
  python3 -c "import json, sys; r = json.load(open(sys.argv[1])); print($2)" "$HOME/.cache/vllm/$1.json"
}
# wait_record <run> <container>
wait_record() {
  i=0
  until [ -e "$HOME/.cache/vllm/$1.json" ]; do
    i=$((i + 1))
    [ "$i" -lt 100 ] || fail "vllm never ran in $2: exit $(docker inspect -f '{{.State.ExitCode}}' "$2"): $(docker logs "$2" 2>&1 | tail -2)"
    sleep 0.1
  done
}
# exited <container>: waits until it has stopped, and prints its exit code.
exited() {
  i=0
  until [ "$(docker inspect -f '{{.State.Status}}' "$1")" = exited ]; do
    i=$((i + 1))
    [ "$i" -lt 100 ] || fail "$1 didn't exit by itself"
    sleep 0.1
  done
  docker inspect -f '{{.State.ExitCode}}' "$1"
}

echo "== 1. deploy-files, on real files (the shipped sparkjob.py)"
deploy single first "$key"
[ ! -s "$work/deploy.err" ] || fail "deploy-files: $(cat "$work/deploy.err")"
[ "$(cat "$work/deploy.out")" = "{\"dir\": \"$deploy_dir\"}" ] || fail "deploy-files printed something else: $(cat "$work/deploy.out")"
modes="$(stat -f '%Lp' "$deploy_dir") $(stat -f '%Lp' "$deploy_dir/argv.json") $(stat -f '%Lp' "$deploy_dir/env.json") $(stat -f '%Lp' "$key_file")"
[ "$modes" = "700 600 600 400" ] || fail "the modes are $modes"
[ "$(cat "$key_file")" = "$key" ] || fail "the key file doesn't hold exactly the key"
for cache in .cache/vllm .cache/flashinfer .triton; do
  [ -d "$HOME/$cache" ] || fail "deploy-files didn't make ~/$cache"
done
if grep -q "$key" "$deploy_dir/argv.json" "$deploy_dir/env.json" "$work/deploy.out"; then
  fail "the key is in a launch file or in what deploy-files printed"
fi
echo "   folder 700, argv.json and env.json 600, key 400; the caches are there; it printed only the folder"

echo "== 2. A one-node load's C-L2, minus --gpus, by image id"
single_rank >/dev/null
wait_record first "$SINGLE"
want=$(printf '%s' "$key" | shasum -a 256 | cut -c1-64)
[ "$(record first 'r["key"]')" = "$want" ] || fail "the process didn't get the key"
echo "   $IMAGE_ID"
echo "   vllm's VLLM_API_KEY hashes to the key's SHA-256: the process sees it"
ulimits=$(record first '" ".join(r["ulimits"])')
[ "$ulimits" = "unlimited 65536 1048576" ] || fail "the ulimits are $ulimits"
echo "   its limits: memlock $(echo "$ulimits" | cut -d' ' -f1), stack $(echo "$ulimits" | cut -d' ' -f2) KiB, nofile $(echo "$ulimits" | cut -d' ' -f3)"

echo "== 3. PID 1 supervises vllm and reaps its children"
[ "$(record first 'r["pid"]')" -gt 1 ] || fail "vllm replaced its supervisor"
[ "$(record first 'r["parent"]')" = 1 ] || fail "vllm is not the supervisor's child"
cmdline=$(docker exec "$SINGLE" sh -c 'tr "\0" " " < /proc/1/cmdline')
case "$cmdline" in
  *launch.py*) ;;
  *) fail "PID 1 is not the launcher: $cmdline" ;;
esac
echo "   /proc/1/cmdline: $cmdline"
recorded=$(docker inspect -f '{{.Path}} {{join .Args " "}}' "$SINGLE")
[ "$recorded" = "python3 /run/sparkbox/launch.py --key" ] || fail "docker records $recorded"
echo "   Docker's record still says: $recorded (so discovery reads the labels)"

echo "== 4. docker inspect"
inspected=$(docker inspect "$SINGLE")
case "$inspected" in
  *"$key"*) fail "docker inspect contains the key" ;;
esac
case "$inspected" in
  *"$HOME/.sparkbox/keys/$LOAD"*) ;;
  *) fail "docker inspect doesn't show the key's mount, so this looked at the wrong thing" ;;
esac
echo "   no key in docker inspect ($(printf '%s' "$inspected" | wc -c | tr -d ' ') bytes read); its mount is there"
docker run -d --name "$CONTROL" --label sparkbox.staging=1 -e "VLLM_API_KEY=$key" --entrypoint sleep "$IMAGE_ID" 30 >/dev/null
control=$(docker inspect "$CONTROL")
docker rm -f "$CONTROL" >/dev/null
case "$control" in
  *"$key"*) echo "   control: with -e the same key does land in docker inspect" ;;
  *) fail "the control's -e isn't in docker inspect: this check can't see a leak" ;;
esac

echo "== 5. A quoted JSON flag arrives as one element"
got=$(record first 'r["argv"][r["argv"].index("--speculative-config") + 1]')
[ "$got" = "$FLAG" ] || fail "the flag arrived as: $got"
echo "   --speculative-config's value: $got"

echo "== 6. docker stop -t 5 delivers TERM"
began=$(date +%s)
docker stop -t 5 "$SINGLE" >/dev/null
took=$(($(date +%s) - began))
[ -e "$HOME/.cache/vllm/first.term" ] || fail "vllm never saw TERM"
[ "$(docker inspect -f '{{.State.ExitCode}}' "$SINGLE")" = 0 ] || fail "vllm didn't end by its own handler"
[ "$took" -lt 5 ] || fail "docker stop took $took s, so it wasn't TERM that ended it"
echo "   vllm's handler ran, it exited 0, and docker stop took $took s"
docker rm "$SINGLE" >/dev/null
deploy single noterm "$key" VLLM_PROOF_NO_HANDLER
single_rank >/dev/null
wait_record noterm "$SINGLE"
began=$(date +%s)
docker stop -t 5 "$SINGLE" >/dev/null
took=$(($(date +%s) - began))
code=$(docker inspect -f '{{.State.ExitCode}}' "$SINGLE")
[ "$took" -ge 5 ] && [ "$code" = 137 ] || fail "the control: an engine ignoring TERM took $took s and exited $code: this check can't tell"
echo "   control: engine ignores TERM, took $took s and exited $code"
docker rm "$SINGLE" >/dev/null

echo "== 7. A missing key and an empty key"
missing_line='sparkbox: the API key file is missing or empty, so vLLM was not started.'
refused() {
  label=$1
  single_rank >/dev/null
  code=$(exited "$SINGLE")
  [ "$code" = 78 ] || fail "$label: exit $code, not 78: $(docker logs "$SINGLE" 2>&1 | tail -2)"
  line=$(docker logs "$SINGLE" 2>&1)
  [ "$line" = "$missing_line" ] || fail "$label: the log says: $line"
  [ ! -e "$HOME/.cache/vllm/$2.json" ] || fail "$label: vllm started"
  case "$(docker inspect "$SINGLE")" in
    *"$key"*) fail "$label: the key is in docker inspect" ;;
  esac
  docker rm "$SINGLE" >/dev/null
  echo "   $label: exit 78, the launcher's line, vllm never started"
}
deploy single missing "$key"
rm -f "$key_file"
refused "a missing key" missing
deploy single empty "$key"
chmod 0600 "$key_file"
: > "$key_file"
refused "an empty key" empty

echo "== 8. No --key leaves no VLLM_API_KEY"
deploy worker worker ""
[ ! -e "$key_file" ] || fail "a worker's deploy-files left a key file"
worker_rank >/dev/null
wait_record worker "$WORKER"
[ "$(record worker 'r["key"]')" = None ] || fail "vllm has a VLLM_API_KEY with no --key"
case "$(record worker 'r["argv"]')" in
  *--headless*) ;;
  *) fail "the worker's argv has no --headless" ;;
esac
baked=$(docker exec "$WORKER" printenv VLLM_API_KEY)
[ "$baked" = "$BAKED" ] || fail "the control: the image has no VLLM_API_KEY: $baked"
echo "   the worker's vllm has none, though the image sets one (control: docker exec still sees it there)"
docker stop -t 5 "$WORKER" >/dev/null
docker rm "$WORKER" >/dev/null

echo
echo "Passed: no shell in the path, the key in no Docker record, one word for a JSON flag, TERM reaches vllm."
