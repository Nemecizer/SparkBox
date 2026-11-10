#!/bin/sh
# The staging Ubuntu tier (PLAN P1.S): a "staging Spark", an Ubuntu 24.04 arm64 container in
# Docker Desktop with what SparkBox's write paths meet on a real node and fake_node can't give:
# stock sshd (MaxSessions 10), sudo 1.9.15 with `Defaults use_pty`, systemd as PID 1 (so
# systemd-run is real), apt and dpkg with a dummy .deb repo served over HTTP inside it, needrestart,
# kmod, Python 3.12 and the user sparkuser (uid 1000, in sudo) with a password. No GPU, no fabric, no
# Docker inside.
#
#   scripts/staging/setup.sh [role]     role: a-z, 0-9 and -, default spark1
#
# Builds the image sparkbox-staging:24.04 from scripts/staging/image/ (cached after the first time;
# what is in it: image/Dockerfile, image/build-repo.sh and the docker stand-in image/docker-standin.py)
# and starts the container sparkbox-staging-<role>, labelled com.nemecj.sparkbox.staging=<role>.
# Image and container carry sparkbox.staging=1 (teardown.sh removes what has it) and
# sparkbox.staging.rev, a hash of image/: a container an older image/ made is refused (run
# teardown.sh --image first). Its sshd is published on
# 127.0.0.1 only, on a port Docker picks: `docker port sparkbox-staging-<role> 22`. Each start gets
# its own host keys and a new random password for sparkuser, kept in the container only
# (/root/.sparkbox-staging-password, read with docker exec). Run again: an existing container is
# left as it is (started again if it was stopped), and this returns once its systemd and sshd are
# up, with the dummy repo answering. Several at once (the Rust tests start them in parallel) take
# turns on a lock; the later ones find the container ready. Rollback: scripts/staging/teardown.sh.
#
# Docker Desktop only (context desktop-linux). Never podman: its nvidia-workbench machine belongs
# to NVIDIA AI Workbench. When Docker Desktop isn't running, this starts it (open -ga Docker) and
# waits up to 3 minutes; if it doesn't come up (a first-run dialog, an update), start it once by
# hand and run this again. Exit 2: Docker Desktop isn't available.
set -eu
unset DOCKER_HOST
DOCKER="docker --context desktop-linux"
IMAGE=sparkbox-staging:24.04
HERE=$(cd "$(dirname "$0")" && pwd)
# What the image is made from (the base is pinned by digest in the Dockerfile).
rev=$(cat "$HERE/image/Dockerfile" "$HERE/image/build-repo.sh" "$HERE/image/docker-standin.py" | shasum -a 256 | cut -c1-12)

role="${1:-spark1}"
case "$role" in
  *[!a-z0-9-]* | "" | -*) echo "The role may only have a-z, 0-9 and - (got '$role')." >&2; exit 1 ;;
esac
[ "${#role}" -le 20 ] || { echo "The role is longer than 20 characters." >&2; exit 1; }
name="sparkbox-staging-$role"

# One setup.sh per role at a time, so two can't both create the container or read it half made.
# The lock is flock (lockf), so the kernel drops it when this script ends, however it ends.
tmp=${TMPDIR:-/tmp}
exec 8>>"${tmp%/}/$name.setup.lock"
lockf -s -t 900 8 || { echo "Another setup.sh for $role held its lock for 15 minutes." >&2; exit 1; }

docker_up() {
  [ "$($DOCKER info --format '{{.OperatingSystem}}' 2>/dev/null)" = "Docker Desktop" ]
}
if ! docker_up; then
  echo "Starting Docker Desktop…"
  open -ga Docker
  i=0
  while ! docker_up; do
    i=$((i + 5))
    if [ "$i" -ge 180 ]; then
      echo "Docker Desktop didn't come up in 3 minutes. Start it once by hand (it may be showing" >&2
      echo "a first-run or update dialog), then run this again." >&2
      exit 2
    fi
    sleep 5
  done
fi

# systemd as PID 1 has finished starting: running, or degraded (a unit a container can't run).
wait_systemd() {
  i=0
  until state=$($DOCKER exec "$name" systemctl is-system-running 2>/dev/null || true) && { [ "$state" = running ] || [ "$state" = degraded ]; }; do
    i=$((i + 1))
    if [ "$i" -ge 60 ]; then
      echo "systemd in $name didn't finish starting (state: ${state:-none}):" >&2
      $DOCKER logs --tail 20 "$name" >&2
      exit 1
    fi
    sleep 1
  done
}

# The dummy repo answers on the container's own loopback (sparkbox-staging-repo.service).
wait_repo() {
  i=0
  until $DOCKER exec "$name" python3 -c 'import urllib.request; urllib.request.urlopen("http://127.0.0.1:8080/dists/noble/Release", timeout=2)' 2>/dev/null; do
    i=$((i + 1))
    [ "$i" -lt 30 ] || { echo "The dummy repo in $name isn't answering on 127.0.0.1:8080 (systemctl status sparkbox-staging-repo)." >&2; exit 1; }
    sleep 1
  done
}

# sshd answers with its banner on the published port (sets addr and banner).
wait_sshd() {
  addr=$($DOCKER port "$name" 22/tcp)
  port=${addr##*:}
  i=0
  until banner=$(nc -w 2 127.0.0.1 "$port" </dev/null 2>/dev/null | head -1) && [ -n "$banner" ]; do
    i=$((i + 1))
    [ "$i" -lt 30 ] || { echo "sshd in $name isn't answering on $addr." >&2; exit 1; }
    sleep 1
  done
}

if [ -n "$($DOCKER ps -aq --filter "name=^$name\$")" ]; then
  made=$($DOCKER inspect -f '{{index .Config.Labels "sparkbox.staging.rev"}}' "$name")
  if [ "$made" != "$rev" ]; then
    echo "$name was made from an older staging image (${made:-no rev label}; image/ is now $rev)." >&2
    echo "Run scripts/staging/teardown.sh --image, then this again." >&2
    exit 1
  fi
  if [ "$($DOCKER inspect -f '{{.State.Running}}' "$name")" != true ]; then
    echo "Starting $name again…"
    $DOCKER start "$name" >/dev/null
  fi
  wait_systemd
  # The password is the last thing a first run writes: without it the container is half made.
  if ! $DOCKER exec "$name" test -s /root/.sparkbox-staging-password; then
    echo "$name is only partly set up (an earlier setup.sh stopped part-way). Run" >&2
    echo "scripts/staging/teardown.sh, then this again." >&2
    exit 1
  fi
  wait_repo
  # A test that takes sshd away (the lost-upgrade test) starts it again when it ends; if it was
  # killed first, this does (nothing when sshd's socket is already listening).
  $DOCKER exec "$name" systemctl start ssh.socket
  wait_sshd
  echo "$name is already set up: sshd on $addr"
  exit 0
fi

echo "Building $IMAGE (cached after the first time)…"
# BuildKit makes only the final image, labelled: no intermediate or unlabelled image is left behind.
$DOCKER build --platform linux/arm64 -t "$IMAGE" --label sparkbox.staging=1 --label "sparkbox.staging.rev=$rev" "$HERE/image"

echo "Starting ${name}…"
# systemd as PID 1 without --privileged: SYS_ADMIN, seccomp and AppArmor unconfined, and the
# container's own cgroup namespace mounted writable (Docker mounts it read-only).
$DOCKER run -d -t --name "$name" --hostname "$name" \
  --label com.nemecj.sparkbox.staging="$role" --label sparkbox.staging=1 --label "sparkbox.staging.rev=$rev" \
  --platform linux/arm64 \
  --cgroupns=private --cap-add SYS_ADMIN --security-opt seccomp=unconfined --security-opt apparmor=unconfined \
  --tmpfs /run:exec --tmpfs /run/lock \
  -p 127.0.0.1::22 \
  --entrypoint /bin/sh "$IMAGE" -c 'mount -o remount,rw /sys/fs/cgroup && exec /sbin/init' >/dev/null

wait_systemd
$DOCKER exec "$name" ssh-keygen -A >/dev/null

# A new random password with a space, quotes, a backslash, a dollar and a non-ASCII letter: it
# reaches sshd and sudo as bytes, never through a shell. It goes in on stdin, never as an argument.
pw="sb-$(LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 16) '\"\\\$é"
printf 'sparkuser:%s\n' "$pw" | $DOCKER exec -i "$name" chpasswd
printf '%s' "$pw" | $DOCKER exec -i "$name" sh -c 'umask 077 && cat > /root/.sparkbox-staging-password'
pw=""

wait_repo
wait_sshd
echo "$name is up: sshd on $addr ($(echo "$banner" | tr -d '\r'))"
$DOCKER exec "$name" sh -c 'for k in /etc/ssh/ssh_host_*_key.pub; do ssh-keygen -lf "$k"; done'
