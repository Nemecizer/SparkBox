#!/bin/sh
# Removes the staging tier (PLAN P1.S rollback): every sparkbox-staging-* container, the scripts'
# empty lock files in $TMPDIR and the Rust tests' pinned host keys and data folders next to them
# (sparkbox-staging-<test>-<pid>: a Live's known hosts, ssh.log and the evidence copies of the
# container's package lists), and with --image every image staging built: those labelled
# sparkbox.staging=1 (setup.sh labels each build), round 2's builds (labelled
# com.nemecj.sparkbox.staging=image) and the sparkbox-staging:24.04 tag.
# Removes nothing it didn't create — not the base ubuntu:24.04, no other image or container, not
# the nvidia-workbench podman machine (never touched), not Docker Desktop. While check.sh,
# setup.sh or the Rust staging tests hold a lock, it removes nothing and says so. Deleting
# scripts/staging/ by hand finishes the rollback.
#
#   scripts/staging/teardown.sh [--image]
set -eu
unset DOCKER_HOST
DOCKER="docker --context desktop-linux"

# A lock someone holds means a run is using the container: leave everything. (-k: only look.)
tmp=${TMPDIR:-/tmp}
for f in "${tmp%/}"/sparkbox-staging-*.lock; do
  [ -e "$f" ] || continue
  if ! lockf -k -s -t 0 "$f" true; then
    echo "A staging run holds $f (check.sh, setup.sh or the staging tests); nothing removed." >&2
    exit 1
  fi
done

if ! $DOCKER info >/dev/null 2>&1; then
  echo "Docker Desktop isn't running; nothing to remove."
  exit 0
fi

ids=$($DOCKER ps -aq --filter "name=^sparkbox-staging-" 2>/dev/null || true)
if [ -n "$ids" ]; then
  echo "Removing staging containers:"
  $DOCKER ps -a --filter "name=^sparkbox-staging-" --format '  {{.Names}} ({{.Status}})'
  # shellcheck disable=SC2086
  $DOCKER rm -f $ids >/dev/null
else
  echo "No staging containers."
fi

if [ "${1:-}" = "--image" ]; then
  # Old builds too: a rebuild leaves the previous image untagged, still labelled.
  ids=$( {
    $DOCKER image ls -aq --filter label=sparkbox.staging=1
    $DOCKER image ls -aq --filter label=com.nemecj.sparkbox.staging=image
    $DOCKER image ls -aq sparkbox-staging:24.04
  } 2>/dev/null | sort -u)
  for id in $ids; do
    what=$($DOCKER image inspect -f '{{join .RepoTags " "}}' "$id" 2>/dev/null || true)
    if $DOCKER image rm "$id" >/dev/null; then
      echo "Removed image $id${what:+ ($what)}."
    else
      echo "Couldn't remove image $id${what:+ ($what)}; see above." >&2
    fi
  done
  [ -n "$ids" ] || echo "No staging images."
fi
# The lock files, each only while nobody holds it (lockf removes the file once it has the lock),
# and the host keys the Rust tests pinned.
for f in "${tmp%/}"/sparkbox-staging-*.lock; do
  [ -e "$f" ] || continue
  lockf -s -t 0 "$f" true || echo "Kept $f: a staging run took it just now."
done
rm -f "${tmp%/}"/sparkbox-staging-*.known_hosts.json
for d in "${tmp%/}"/sparkbox-staging-*-[0-9]*; do
  [ -d "$d" ] && rm -rf "$d"
done
echo "Done. Delete scripts/staging/ to finish the rollback."
