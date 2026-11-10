#!/bin/sh
# Contract C-T's proof against real Docker Desktop (P1 round 5, plan item D2): the way sparkjob.py
# hands a download its Hugging Face token keeps the token out of Docker's records, and a process
# in the container can still read it. The stand-in in the staging node can't show this: only real
# Docker writes a container's config.
#
#   scripts/staging/token-proof.sh
#
# Uses only sparkbox-staging-* names, all removed when it ends, however it ends, with the
# throwaway token file. The token is a random fake string, and nothing here sends it anywhere.
# setup.sh's sparkbox-staging-spark1 stays up, as after any staging run: teardown.sh removes it.
#
#   1. A 0400 file holding the fake string, and the container sparkbox-staging-token from the
#      staging image, with --user matching the file's owner and C-T's -e and -v.
#   2. docker inspect doesn't contain the string. As a control, it does for the two forms C-T
#      rules out, -e HF_TOKEN and --env-file, so this check can fail.
#   3. A process in the container reads the file through its mount.
#   4. In the same container, huggingface_hub from pip in a venv: get_token() returns the string
#      through HF_TOKEN_PATH, and nothing without it. Needs the network (pypa.io and PyPI); when
#      that fails, it says so plainly and the script exits 3.
#   5. Real `hf` as a container's PID 1, as a download runs it: what SIGTERM does, with no --init
#      and with --init, and what `docker stop -t 5 <name>` does, with the docker CLI attached and
#      after kill -9 of the CLI (a job that was SIGKILLed). It hangs on a local socket that never
#      answers, so it reaches no network. Needs step 4's hf.
#
# Exit 0: every step ran and passed. 1: a step failed. 2: Docker Desktop isn't available.
# 3: steps 1-3 passed, but step 4 couldn't run.
set -eu
unset DOCKER_HOST
DOCKER="docker --context desktop-linux"
HERE=$(cd "$(dirname "$0")" && pwd)
IMAGE=sparkbox-staging:24.04
NAME=sparkbox-staging-token
CONTROL=sparkbox-staging-token-control
PID1=sparkbox-staging-token-pid1
WITH_HF=sparkbox-staging-token-hf:throwaway
MOUNT=/run/sparkbox/hf_token

tmp=${TMPDIR:-/tmp}
work=$(mktemp -d "${tmp%/}/sparkbox-staging-token-XXXXXXXX")
cleanup() {
  for c in "$PID1" "$NAME" "$CONTROL"; do
    $DOCKER rm -f "$c" >/dev/null 2>&1 || true
  done
  $DOCKER image rm "$WITH_HF" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 1' INT TERM HUP

fail() {
  echo "FAILED: $1" >&2
  exit 1
}

# The staging image, built if it isn't there. setup.sh starts Docker Desktop when it isn't up. It
# also starts the node's sshd, which a staging test may have stopped on purpose, so it waits for
# the container's use lock, as check.sh does. Nothing after it touches that container.
exec 9>>"${tmp%/}/sparkbox-staging-spark1.lock"
lockf -s -t 900 9 || fail "sparkbox-staging-spark1 was still in use after 15 minutes"
status=0
"$HERE/setup.sh" >"$work/setup.log" 2>&1 || status=$?
exec 9>&-
[ "$status" = 2 ] && { echo "Docker Desktop isn't up; start it by hand and re-run. Nothing proved."; exit 2; }
[ "$status" = 0 ] || { tail -5 "$work/setup.log" >&2; fail "setup.sh exited $status"; }

token="hf_sbfake$(LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 32)"
file="$work/hf_token"
(umask 077 && printf '%s' "$token" > "$file")
chmod 0400 "$file"

echo "== 1. The container, with C-T's -e and -v"
$DOCKER run -d --name "$NAME" --label sparkbox.staging=1 --user "$(id -u):$(id -g)" \
  -e "HF_TOKEN_PATH=$MOUNT" -e HF_HOME=/tmp/hf -e HF_HUB_DISABLE_PROGRESS_BARS=1 \
  -v "$file:$MOUNT:ro" --entrypoint sleep "$IMAGE" 900 >/dev/null
echo "   $NAME runs as $(id -u):$(id -g); on this Mac the file is $(stat -f '%Lp' "$file")"

echo "== 2. docker inspect"
inspected=$($DOCKER inspect "$NAME")
case "$inspected" in
  *"$token"*) fail "docker inspect contains the token" ;;
esac
case "$inspected" in
  *"\"HF_TOKEN_PATH=$MOUNT\""*) ;;
  *) fail "docker inspect doesn't show HF_TOKEN_PATH, so this looked at the wrong thing" ;;
esac
case "$inspected" in
  *"$file:$MOUNT:ro"*) ;;
  *) fail "docker inspect doesn't show the read-only mount" ;;
esac
echo "   no token in docker inspect ($(printf '%s' "$inspected" | wc -c | tr -d ' ') bytes read); HF_TOKEN_PATH and the :ro mount are there"
# The control: what C-T replaced. -e HF_TOKEN takes the value from the CLI's environment.
fake1="hf_sbcontrol$(LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 32)"
fake2="hf_sbcontrol$(LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 32)"
(umask 077 && printf 'HF_HUB_TOKEN_TEST=%s\n' "$fake2" > "$work/env")
HF_TOKEN=$fake1 $DOCKER run -d --name "$CONTROL" --label sparkbox.staging=1 -e HF_TOKEN --env-file "$work/env" --entrypoint sleep "$IMAGE" 900 >/dev/null
control=$($DOCKER inspect "$CONTROL")
$DOCKER rm -f "$CONTROL" >/dev/null
case "$control" in
  *"$fake1"*) echo "   control: -e HF_TOKEN does land in docker inspect" ;;
  *) fail "the control's -e HF_TOKEN isn't in docker inspect: this check can't see a leak" ;;
esac
case "$control" in
  *"$fake2"*) echo "   control: --env-file does land in docker inspect" ;;
  *) fail "the control's --env-file value isn't in docker inspect: this check can't see a leak" ;;
esac

echo "== 3. A process in the container reads the file"
read_back=$($DOCKER exec "$NAME" cat "$MOUNT")
[ "$read_back" = "$token" ] || fail "the process in the container read something else"
echo "   uid $($DOCKER exec "$NAME" id -u) read it through the mount, and it matches"
if $DOCKER exec "$NAME" sh -c "echo x >> $MOUNT" 2>/dev/null; then
  fail "the mount is writable from inside the container"
fi
echo "   and can't write to it"

echo "== 4. huggingface_hub's get_token() through HF_TOKEN_PATH"
# The image has no pip, no ensurepip and no CA certificates. get-pip.py carries pip, and pip
# carries its own certificates, so it's fetched here and handed in on stdin.
pip_ok=
if ! curl -fsSL --max-time 60 https://bootstrap.pypa.io/get-pip.py -o "$work/get-pip.py"; then
  echo "   PIP STEP COULD NOT RUN: couldn't fetch get-pip.py from bootstrap.pypa.io"
elif ! $DOCKER exec -i "$NAME" sh -c 'cat > /tmp/get-pip.py' < "$work/get-pip.py"; then
  fail "couldn't copy get-pip.py into the container"
elif ! $DOCKER exec "$NAME" python3 -m venv --without-pip /tmp/hfvenv; then
  fail "couldn't make a venv in the container"
elif ! $DOCKER exec "$NAME" /tmp/hfvenv/bin/python /tmp/get-pip.py --quiet --disable-pip-version-check >"$work/pip.log" 2>&1; then
  echo "   PIP STEP COULD NOT RUN: get-pip.py failed: $(tail -1 "$work/pip.log")"
elif ! $DOCKER exec "$NAME" /tmp/hfvenv/bin/pip install --quiet --disable-pip-version-check huggingface_hub >"$work/pip.log" 2>&1; then
  echo "   PIP STEP COULD NOT RUN: pip install huggingface_hub failed: $(tail -1 "$work/pip.log")"
else
  pip_ok=1
fi

if [ -n "$pip_ok" ]; then
  # Run twice: with C-T's environment it reads the file, and without HF_TOKEN_PATH it finds none.
  # The file's value stays in the container's process; only a yes or no comes back.
  check='
import hashlib, sys
from huggingface_hub import __version__, constants, get_token
token = get_token()
print(__version__, constants.HF_TOKEN_PATH, "none" if token is None else hashlib.sha256(token.encode()).hexdigest())
'
  want=$(printf '%s' "$token" | shasum -a 256 | cut -c1-64)
  set -- $($DOCKER exec "$NAME" /tmp/hfvenv/bin/python -c "$check")
  [ "$#" = 3 ] || fail "get_token()'s check printed $# words"
  version=$1
  [ "$2" = "$MOUNT" ] || fail "huggingface_hub's constants.HF_TOKEN_PATH is $2, not $MOUNT"
  [ "$3" = "$want" ] || fail "get_token() didn't return the string"
  echo "   huggingface_hub $version: constants.HF_TOKEN_PATH is $2, and get_token() returned the string"
  set -- $($DOCKER exec "$NAME" env -u HF_TOKEN_PATH /tmp/hfvenv/bin/python -c "$check")
  [ "$3" = none ] || fail "get_token() found a token with no HF_TOKEN_PATH, so the file wasn't what it read"
  echo "   with no HF_TOKEN_PATH it returns nothing, so the file is where the token came from"

  echo "== 5. Real hf as PID 1: SIGTERM, then docker stop by name"
  # A socket that takes the connection and never answers: hf stays in its first request, on
  # 127.0.0.1 of a network the next containers share. Nothing leaves this Mac.
  hang='
import socket, time
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 8099))
s.listen(16)
held = []
while True:
    conn, _ = s.accept()
    held.append(conn)
    open("/tmp/hf-connected", "w").close()
'
  $DOCKER exec -d "$NAME" python3 -c "$hang"
  $DOCKER commit --change 'LABEL sparkbox.staging=1' "$NAME" "$WITH_HF" >/dev/null
  # hf_up <label> <docker run options>: hf as PID 1, in its first request. Its CLI's pid is $cli.
  hf_up() {
    label=$1
    shift
    $DOCKER exec "$NAME" rm -f /tmp/hf-connected
    $DOCKER run --rm --name "$PID1" --network "container:$NAME" "$@" -e HF_ENDPOINT=http://127.0.0.1:8099 -e HF_HOME=/tmp/hf \
      --entrypoint /tmp/hfvenv/bin/hf "$WITH_HF" download sbstage/never-answers --local-dir /tmp/dest >"$work/pid1.log" 2>&1 &
    cli=$!
    i=0
    until $DOCKER exec "$NAME" test -e /tmp/hf-connected 2>/dev/null; do
      i=$((i + 1))
      [ "$i" -lt 120 ] || fail "hf never reached the local socket ($label): $(tail -2 "$work/pid1.log")"
      sleep 0.5
    done
  }
  # try <label> <docker run options>: the verdict goes in $verdict: "ignored" or "stopped <exit>".
  try() {
    hf_up "$@"
    label=$1
    kill -TERM "$cli"
    sleep 6
    if kill -0 "$cli" 2>/dev/null; then
      running=$($DOCKER ps --filter "name=^$PID1\$" --format '{{.Status}}')
      echo "   $label: 6 s after SIGTERM the docker CLI and the container are still running (${running:-no container})"
      verdict=ignored
      kill -9 "$cli" 2>/dev/null || true
      wait "$cli" 2>/dev/null || true
      sleep 1
      running=$($DOCKER ps --filter "name=^$PID1\$" --format '{{.Status}}')
      if [ -n "$running" ]; then
        echo "   $label: after kill -9 of the CLI the container is still running ($running)"
      else
        echo "   $label: kill -9 of the CLI ended the container too"
      fi
      $DOCKER rm -f "$PID1" >/dev/null 2>&1 || true
    else
      code=0
      wait "$cli" || code=$?
      echo "   $label: SIGTERM stopped it, docker run exited $code"
      verdict="stopped $code"
    fi
  }
  try "no --init"
  plain=$verdict
  try "with --init" --init
  with_init=$verdict
  echo "   hf as PID 1: no --init: $plain; with --init: $with_init"

  # None of this has --init: docker stop's SIGKILL needs nothing inside the container.
  gone() {
    [ -z "$($DOCKER ps -aq --filter "name=^$PID1\$")" ]
  }
  hf_up "docker stop, CLI attached"
  began=$(date +%s)
  $DOCKER stop -t 5 "$PID1" >/dev/null
  took=$(($(date +%s) - began))
  code=0
  wait "$cli" || code=$?
  gone || fail "a container is left after docker stop"
  [ "$code" = 137 ] || fail "docker run exited $code after docker stop, not 137"
  [ "$took" -ge 5 ] || fail "docker stop took $took s: hf didn't ignore SIGTERM this time, so this showed nothing"
  echo "   docker stop -t 5 $PID1: hf ended after $took s, docker run exited $code, and --rm left no container"
  hf_up "docker stop, CLI killed"
  kill -9 "$cli"
  wait "$cli" 2>/dev/null || true
  sleep 1
  gone && fail "the container went with its CLI, so this showed nothing"
  $DOCKER stop -t 5 "$PID1" >/dev/null
  gone || fail "a container is left after docker stop by name"
  echo "   kill -9 of the CLI left the container running; docker stop by name then ended it and --rm left nothing"
fi

echo
echo "Passed: the token isn't in docker inspect, and a process in the container reads the file."
if [ -z "$pip_ok" ]; then
  echo "NOT PROVED: the pip step couldn't run, so get_token() through HF_TOKEN_PATH wasn't checked."
  exit 3
fi
echo "Passed: huggingface_hub's get_token() reads it through HF_TOKEN_PATH."
echo "Passed: docker stop by name ends hf as PID 1, with its CLI attached or killed, with no --init."
