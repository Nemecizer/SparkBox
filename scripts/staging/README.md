# Staging Ubuntu tier (PLAN P1.S, §6.4)

A "staging Spark": an **Ubuntu 24.04 arm64** container on this Mac, in **Docker Desktop**, that
behaves like a real node in exactly the places `fake_node` can't — real OpenSSH (MaxSessions 10),
real `sudo 1.9.15` with `Defaults use_pty`, systemd as PID 1 (so `systemd-run` is real), real
`apt`/`dpkg` against a dummy repo served over HTTP inside the container, `needrestart`, `kmod`
(modinfo, depmod) and Python 3.12. No GPU, no CX-7 fabric, no Docker-in-Docker. It proves the
command contracts SparkBox's write paths depend on, offline, before any supervised run on the
cluster.

The tier is **required before P6 (staged lockstep) and P7 (agent under systemd)**, not before the
P1 gate. Round 2 stood it up and proved `sudo_exchange` with and without input under `use_pty`, the
`systemd-run` upgrade runner and reattach, holds, needrestart, the channel budget, the detached
upgrade's Cancel and the Python 3.12 run. Round 3 adds the dummy GPU stack and proves, through
SparkBox's own code: the r3 upgrade runner (the package list saved first, `apt-get check` and
needrestart inside the unit, apt's exit code kept, the dist-upgrade form), the reattach with a
download limit, "Pin the GPU stack" and holds, the GPU-stack and 590 guards, the checks before a
restart, and the update check. Its four `staging_session_b_*` tests are the real-Ubuntu half of
Session B steps 19, 21, 22 and 24 (`docs/acceptance/P1-session-B.md` cites them).

**Round 4** adds, through SparkBox's own code on the staging node:
- **shell conformance** (`staging_shell_conformance`, the real proof for T2 blocker 2): a table of
  commands that hide `touch /tmp/sbconf-<n>` (or `rm -rf` of a planted folder) in shell syntax,
  the reads SparkBox sends, and this round's change literals. Each runs through SparkBox's `Live`
  under the real dash 0.5.12 that `/bin/sh` is on the four Sparks and under the real bash 5.2.21,
  and is classified by both of SparkBox's parsers. No row may be a read while a shell ran its
  hidden command;
- **first light's command shapes** under real dash (Session B step 27), today P2 round 1's: the
  pre-flight (C-P2) on real Ubuntu output, the launch files (C-F), the single-node load (C-L2) read by
  a `docker` stand-in that records argv, and the unload (C-U2), plus the page-cache drop (C-D) under
  real `sudo` + `use_pty`;
- **the plan binding** (decision b) on real apt: an upgrade installs exactly the versions its dry
  run showed (C1′ pinned, C2′ checked just before the start), and refuses in plain words when the
  repo moved meanwhile or the plan removes anything;
- **the restart variants**: a held DKMS package, an `rc` libnvidia-compute-580, no GRUB_DEFAULT, and
  plans that would remove docker-ce or the kernel;
- an upgrade that **loses its node** to a stopped sshd is kept as "lost" and picked up again.

**Round 5** adds node jobs, through `sparkjob.py` and `Models` as shipped, on real rsync, real signals
and real Linux modes (`rsync 3.2.7-1ubuntu1.5` here; the Sparks' own rsync version is unconfirmed):
- **`staging_cancel_leaves_no_rsync_password`**: a serve job on a real `rsync --daemon`. Cancel leaves
  no password file, a closed port and no process; a SIGKILLed job's folder goes at the next `list`;
- **`staging_partial_copy_retries`**: a pull stopped partway with SIGKILL and with Cancel reads
  stopped and unverified, a failed check keeps the marker through `--delete`, and a finished pull's
  own record says `checked: "sizes"`;
- **`staging_hf_snapshot_copy`**: a Hugging Face `snapshots/<rev>` folder of links fails with exit 23,
  `Invalid cross-device link`, and nothing arrives, so `Models::share` refuses one (P5.T2 is the real
  fix). A link inside the module copies;
- **`staging_download_token_files`**: the download token's modes (0700, 0400), who can read it, and
  that it's in no argv, environment, process, `docker inspect`, status or log. Cancel ends the
  container found by both labels, `list` stops nothing, and a retry stops a killed job's container
  first, with or without a `list` in between. Nothing in it stops a container by hand;
- **`staging_read_tier_options`**: what the read tier assumes of the real tools (env 9.4, sed 4.9,
  sort 9.4, tar 1.35, mawk). Each row writes a file or runs a program through a long option cut short
  (`sed --in`, `sort --o=`, `tar --use=`), an option at the end of a cluster (`env -iS`) or one a
  denylist never named (`tar --index-file=`, `awk -W exec`). The test looks at what the tool did and
  asserts neither parser calls it a read; the controls use the same tools and change nothing.

The download test's `docker` is the stand-in, so `scripts/staging/token-proof.sh` is its real half,
on **real Docker Desktop** with `sparkbox-staging-*` names: the token isn't in `docker inspect`, the
`-e HF_TOKEN` and `--env-file` controls are, a real `huggingface_hub` reads the file through
`HF_TOKEN_PATH`, and real `hf` as PID 1 ignores SIGTERM and outlives a killed CLI until `docker stop`.
It was run by hand and isn't part of `cargo test`. `sparkjob.py`'s own `stop_download` was also run
by hand on Docker 29.9.2: two `--filter label=` narrow together, a container with one matching label
stays up, and a container whose CLI was killed stops, in about 5 s when its PID 1 ignores SIGTERM.

**P2 round 1** adds a load's new shapes and the orphan copy, through the shipped code:
- **`staging_multi_node_shapes`**: a load on two nodes under real dash and real Python 3.12, with one
  staging node standing for both, through `Live`: C-P2 reading the rendezvous port on real `ss`, C-F's
  files for a worker and a head, each rank's C-L2 as docker's argv word for word, C-J on real ping,
  and C-U2 taking this load's containers and files and nothing else;
- **`staging_orphan_rsync_after_sigkill`**: real rsync, real `kill -9` of the job alone, real /proc.
  Both of a real rsync client's processes outlive their job, `list` and `list --look` say `"orphan":
  true`, say so in the job's message and stop nothing, delete refuses, and a new pull ends them first:
  a watcher reading /proc sees them gone before any new rsync, and never both. A pid used again is left
  alone, the job's own as well as its child's, and holds no folder. A writer with no marker, and another
  user's process, refuse the new pull and are never signalled;
- **`scripts/staging/launch-proof.sh`**, on **real Docker Desktop**: the real half of what the stand-in
  can't show. It runs the shipped `deploy-files` and `launch.py` and C-L2 minus `--gpus` and
  `--privileged`, with a small `vllm` of its own that records its argv, environment and pid and traps
  TERM. PID 1 now supervises `vllm` and reaps its children; `docker inspect` doesn't contain the key, and with `-e` as a control it does;
  the process sees the key; a quoted JSON flag arrives as one element; `docker stop -t 5` delivers TERM
  (an engine ignoring TERM, the control, takes the 5 s and exits 137); a missing key and an empty key
  each exit 78 with the launcher's line and nothing started; and with no `--key` a `VLLM_API_KEY` baked
  into the image doesn't reach it. C-L2's limits read back as set: memlock unlimited, stack 65536 KiB,
  nofile 1048576. It starts by the image's local id, as a load does. Its two `docker run` lines are
  held to Rust's by the plain test `launch_proof_sends_what_rust_writes`, so they can't drift. It was
  run by hand and isn't part of `cargo test`.

**November 6 unload lifecycle regression:** `python3 scripts/staging/unload-proof.py`
uses the existing staging image and network-disabled containers. A unique private Docker
volume represents shared memory across container lifetimes, without mounting host IPC.
The old exec/PID-1 control must leave a real multiprocessing semaphore behind. The shipped
launcher must allow head and worker trackers to remove their own semaphores, preserve a
still-running worker, an unrelated file and the old control's leftover, and keep the fake
key out of Docker records/logs. A forced SIGKILL control must still leave its semaphore.
Every fixture container and the private volume are removed.
No image is pulled or changed by this proof. The regular launcher unit tests also run in
Ubuntu to cover Linux subreaping; macOS alone cannot establish orphan adoption.

None of this is vLLM. No test here runs vLLM, a GPU, RDMA or a rendezvous between two machines, and
nothing here has run on a Spark. The rendezvous tests are a stand-in's, on fake nodes
(`live/fixtures/vllm_standin.py`). Multi-node on the cluster is the owner's acceptance session.

## Prerequisites (what the owner would need)

- **Docker Desktop** (installed, not running by default). The scripts start it with
  `open -ga Docker` and wait up to 3 minutes. **If it shows a first-run or update dialog, or doesn't
  come up, start it once by hand** and re-run — the scripts never force it (they exit 2 and say so).
- **`uv` (optional, not installed)** would let the node scripts' Python tests run under 3.12 on the
  Mac directly. It isn't on this Mac and the scripts install nothing; instead `check.sh` runs that
  3.12 suite **inside the container**, which is the 3.12 run round 1 owed.
- Never the `nvidia-workbench` podman machine — it is NVIDIA AI Workbench's and is never touched.

## Use

```sh
scripts/staging/setup.sh [role]        # build the image (cached) + start sparkbox-staging-<role>
scripts/staging/check.sh               # the node scripts' Python 3.12 unittests, in the container
scripts/staging/teardown.sh [--image]  # remove the container(s) (and every staging image with --image)
scripts/staging/token-proof.sh         # the download token and Cancel against real Docker Desktop
scripts/staging/launch-proof.sh        # the launcher, C-F and C-L2 against real Docker Desktop

cd src-tauri && SPARKBOX_STAGING_REQUIRED=1 cargo test --lib -- --ignored staging   # the Rust staging tests (below)
```

- `setup.sh` builds the image `sparkbox-staging:24.04` from `scripts/staging/image/` (`Dockerfile`,
  and `build-repo.sh`, which makes the dummy packages). The image and the container carry the label
  `sparkbox.staging=1`, which is how `teardown.sh --image` finds every image staging built, and
  `sparkbox.staging.rev`, a hash of `image/`. **A container made from an older `image/` is refused**
  ("was made from an older staging image … Run scripts/staging/teardown.sh --image, then this
  again"), so a test never runs against yesterday's packages. BuildKit makes only the final,
  labelled image: no intermediate or unlabelled image is left behind.
- It publishes the container's sshd on **127.0.0.1 only**, on a port Docker picks
  (`docker port sparkbox-staging-spark1 22`). It makes fresh host keys and a **new random password**
  for `sparkuser` each time, kept only in the container at `/root/.sparkbox-staging-password`. A
  re-run leaves an existing container as it is: a stopped one (after a Docker Desktop restart or a
  reboot) is started again, and either way `setup.sh` returns only once systemd, the dummy repo and
  sshd are up.
- **One at a time.** `setup.sh` holds a lock per role while it works, so several started at once
  (the Rust tests do) can't race to create the container or read it half made. `check.sh` and each
  Rust staging test hold a second lock, on the container itself, so a `check.sh`, the tests and a
  second `cargo test` take turns instead of running apt and dpkg over each other; whoever comes
  second waits and says so (`check.sh` up to 15 minutes; a Rust staging test up to 45, since the
  31 tests of one run queue on the lock one by one: about 6 minutes in all, 340 s and 434 s in P2 round 1's
  full runs, 211 s and 234 s with 24 tests in round 4's two, but one run that day sat 21 minutes behind a test that stalled on
  this Mac before it logged in, and its 17 waiting tests gave up at round 3's 15 minutes). Both locks are `flock`s on empty files in
  `$TMPDIR` (`sparkbox-staging-<role>.setup.lock`, `sparkbox-staging-spark1.lock`), so the kernel
  drops them when a run ends, even a killed one.

### What the staging node has

- `ubuntu:24.04` (arm64, digest-pinned) + `openssh-server`, `sudo`, `systemd`, `needrestart`,
  `python3`, `rsync`, `kmod` and `binutils` (its `objcopy` makes the crafted nvidia.ko), and
  `file`, `pciutils`, `ripgrep`, `sysstat` and `tree`, tools the read tier's review runs for real.
- **A dummy apt repo**, `/srv/sparkbox-repo`, served by `sparkbox-staging-repo.service`
  (`python3 -m http.server` on the container's own `127.0.0.1:8080`, not published), so apt's
  download limit (`Acquire::http::Dl-Limit`) applies as on a node; `file:` sources ignore it. Suites
  `noble` (what's installed), `noble-updates`, `noble-security` (a security fix is in both, as
  Ubuntu publishes it) and `stable` (Chrome's). apt's real sources are disabled, so every
  `apt-get update` in a test is local. Each package is an empty stand-in, installed at the
  version a Spark has, with a newer one in the repo (`image/build-repo.sh` lists them):
  - round 2's `sparkbox-dummy` (a 1.1 whose postinst prints a line a second for 6 s),
    `sparkbox-dummy-held`, and `sparkbox-dummy-lib` + `sparkbox-dummy-svc.service` (needrestart);
  - `sparkbox-dummy-fail` (1.1's postinst fails while `/run/sparkbox-staging-fail-postinst`
    exists, so apt exits 100);
  - `libbrotli1` (a routine fix in noble-security) and `google-chrome-stable` (the new one is
    20 MiB of random bytes, so a 500 KB/s download takes about half a minute);
  - the GPU stack, named as on a Spark: `nvidia-driver-580-open`, `libnvidia-compute-580`
    (580.95.05 → 580.105.08), `linux-image-7.0.0-1019-nvidia` (its fix in noble-security; it ships
    `/boot/vmlinuz-K` and its postinst makes `/boot/initrd.img-K` and the `/boot/vmlinuz` link, as
    the kernel's hooks do), `linux-modules-nvidia-580-open-7.0.0-1019-nvidia` (see below),
    `docker-ce`, `containerd.io`, `nvidia-container-toolkit`, `nvidia-spark-grub-kho` and
    `dgx-spark-ota-update-meta` (7.7.0 depends on `nvidia-driver-590-open`, which
    Conflicts/Replaces `nvidia-driver-580-open`, so an unpinned dist-upgrade would install 590 and
    remove 580); `nvidia-dkms-580` is in the archive, not installed.
- **The NVIDIA module is a crafted file, read by the real modinfo.** The dummy
  `linux-modules-nvidia-580-open-K` ships `/lib/modules/K/kernel/nvidia-580-open/nvidia.ko`: an
  aarch64 ELF that has only a `.modinfo` section (`version=580.95.05`, or 580.105.08 in the newer
  package), made with `objcopy`. Its postinst runs the real `depmod -a K`, and kmod's own
  `modinfo -k K -F version nvidia` reads it. There is **no modinfo stand-in on PATH** (the step 19
  test checks `command -v modinfo` is kmod's). The module can't be loaded; nothing tries.
- As on a Spark: `/etc/dgx-release` (a Spark's DGX_* lines without its serial number: build 7.2.3,
  OTAs 7.5.0 then 7.6.0), `/etc/default/grub` (`GRUB_DEFAULT=0`), and update-notifier-common's
  `15update-stamp` (each successful `apt-get update` touches
  `/var/lib/apt/periodic/update-success-stamp`). The Docker image's `docker-clean` apt hook is
  removed, because it deletes every downloaded .deb and a node keeps them
  (`APT::Keep-Downloaded-Packages`, the rollback copies). `/proc/cmdline` is the Docker VM's.
- With `/run/sparkbox-staging-slow-apt` present (a test makes it) apt waits 8 s before it calls
  dpkg, so Cancel lands while stopping is still safe.
- **Round 4's additions** (all in the labelled build):
  - **A `docker` stand-in** at `/usr/local/bin/docker` (`image/docker-standin.py`; there's no Docker
    inside). It answers `run`, `ps`, `stop`, `rm`, `image inspect` and, since P2 round 1, `info` and
    `ps`'s `{{.Status}}` and `{{.Command}}` with Docker 29's words and exit
    codes: `docker rm` of a running container is refused ("cannot remove container \"<name>\":
    container is running: …", exit 1), an unknown image or container is "Error response from
    daemon: No such image: …" / "No such container: …", a name already taken is docker run's
    "Conflict. …" (exit 125), each checked against Docker Desktop 29.9.2's own answer on 2026-10-30
    with a throwaway `sparkbox-staging-rmprobe` container. It keeps its containers and images in
    `/var/lib/sparkbox-staging-docker` (the cluster user's), and records every call in
    `calls.jsonl` there: its argv exactly as the shell passed it, its environment, and every
    process's command line and readable environment at that moment. It starts nothing: a load's
    `launch.py` and vLLM never run in it. `docker info` with no format answers, as a daemon does; with
    the pre-flight's format it prints what Docker Desktop 29.9.2 prints (`overlayfs
    [["driver-type","io.containerd.snapshotter.v1"]]`, read on 2026-11-01). The four Sparks' store
    differs: a look-only read that day says `overlay2` on all four. Any other format and any other verb
    answer as a CLI with no daemon (exit 1), so the update check still reads "no Docker".
  - `iproute2`, for `ss` (the pre-flight reads the port with it, as on a Spark), and `iputils-ping`,
    for the jumbo-frame check.
  - Dummies only in the archive (`pool/extra`), installed by the test that needs them:
    `sparkbox-bind` (2.0 depends on a new `sparkbox-bind-dep`), `sparkbox-swap` (2.0 conflicts with
    `sparkbox-swap-old`), `sparkbox-rm-docker` and `sparkbox-rm-kernel` (2.0 conflicts with
    docker-ce, or with linux-image-7.0.0-1019-nvidia). `pool/later` holds what a test publishes
    as an archive would, in place of the version noble-updates had: `sparkbox-bind` 3.0 and
    `sparkbox-bind-dep` 1.1. `/usr/local/sbin/sparkbox-staging-index` makes the indexes again; the
    reset puts the repo back from `/srv/sparkbox-repo.orig`.
  - `libnvidia-compute-580` ships a conffile, so `dpkg -r` leaves it `rc` as on a node.
  - `/usr/share/sparkbox-staging/grub`: the reset puts `/etc/default/grub` back from it.

### Shell facts: the staging node and the four Sparks

`staging_shell_conformance` reads the staging node's shell facts with the evidence capture's own
`shell` step (contract C-SH) and checks them. R4-LIVE's look-only capture of the four Sparks
(2026-10-30 12:49 EDT, `~/Library/Caches/sparkbox-rounds/p1/r4/shell-facts.txt`) read:

| | the four Sparks (cell1, cell2, cell3, gx10-0000) | the staging node |
|---|---|---|
| `/bin/sh`, `/usr/bin/sh` | symlinks to `dash`; `readlink -f`: `/usr/bin/dash` | the same |
| `type sh` | `sh is /usr/bin/sh` | the same |
| dash | `0.5.12-6ubuntu5` | `0.5.12-6ubuntu5` |
| bash | `5.2.21-2ubuntu4` | `5.2.21-2ubuntu4` (`GNU bash, version 5.2.21(1)-release`) |
| sparkuser's login shell | `/bin/bash` | `/bin/bash` |

So the staging node parses SparkBox's commands with the same programs, at the same versions, as
the Sparks: `live::wire` sends `env LC_ALL=C sh -c '<command>'`, the login shell (bash) only
unquotes that one argument, and dash 0.5.12 parses the command. The conformance test runs each row
that way and again as `bash -c '<row>'`, for commands that name a shell themselves. (The passwd
line differs only in its comment field: the Sparks' is `sparkuser,,,`, the container's empty.)

## The Rust tests (`src-tauri/src/live/staging_tests.rs`)

The tests sit inside the crate (a `#[cfg(test)]` module of `live`), so they run SparkBox's own
code against the real node, not copies of it. Some send SparkBox's commands themselves over its
SSH client (`ssh::connect_to`, password login, the node's host key pinned in
`$TMPDIR/sparkbox-staging-spark1.known_hosts.json`) and `sudo_exchange`: `root_install_command`,
`root_sparkjob_command`, and from `ops::updates` `detached_command_opts`, `apt_command`,
`simulate_command`, `cancel_command`, `hold_command`, `PRESET_READ` and `STATUS_SCRIPT`, read back
with its own parsers (`parse_status`, `parse_simulation`, `post_checks`, `progress_of`,
`exit_of`) and judged by `gpu_guard`. Others drive SparkBox itself: a `Live` logged in to the node
the way SparkBox logs in to a Spark (its own node id per test, the container's password where the
vault's would be, no `~/.ssh/config`, no agent; its data folder is `$TMPDIR/sparkbox-staging-<test>-<pid>`)
and its `Ops`: `updates_simulate`, `updates_upgrade_opts`, `reattach_upgrades`,
`updates_hold_preset`, `updates_hold`, `prereboot::check` and the update check's poller round
(`UpdateCache::tick` with `CHECK_SCRIPT` and `STAMPS`). Upgrades `Ops` starts log under
`/var/lib/sparkbox-staging-ops/logs` (where `Live::upgrade_log_dir` points in tests; the app uses
`/var/log/sparkbox`, which the tests that build the command themselves use). Change one of those
and the staging tests run the changed one. `sparkjob.py` is the shipped file. The only node
commands the tests build themselves are their own setup, resets and checks.

These `Ops` have no maintenance-window double and the write exit is the permissive one tests have
(`cfg(test)`): the fence classes, gate entries and windows are proven by the fake_node tests and
the Session B rehearsal, not here. The conformance test reads the fence's shape parser
(`fence_class::class_of`) and the classifier (`classify::verdict`) directly, next to what the real
shells did.

The tests are `#[ignore]`d and named `staging_*`, so the normal `cargo test` (and
`scripts/check.sh`) skips them (no Docker needed) and `cargo test --lib -- --ignored staging` runs
them. They ensure the container via `setup.sh` once per run, so the command is self-contained.
Each test that touches packages first puts the node back (every dummy at its installed version,
nothing held, no 590 driver, DKMS package or other archive-only dummy, the initrd there, the repo
and apt's lists as built, `/etc/default/grub` as built, `/proc/sys` read-only, the docker
stand-in empty), checks that it took, and puts it back again when it's done. Each dpkg change the tests make themselves (the reset, and the few
`dpkg -i` a test does by hand) drops apt's package cache after it: apt keeps using its cache while
dpkg's status file has the same size and the same mtime in whole seconds, so a swap of 1.1 for
1.0 within the second left apt planning against 1.1 ("already the newest version (1.1)"), and
the test after it found nothing to upgrade or cancel. The reset's check includes that apt sees the
versions dpkg has.

**When Docker Desktop can't be reached**, each test prints a line on the console and returns:

```
SKIP (blocked-on-owner): Docker Desktop isn't up, so live::staging_tests::staging_holds ran nothing. ...
```

cargo still counts those tests as passed, so **"31 passed" next to SKIP lines means nothing
ran**. To make that a failure instead (for example when a run must prove something), set
`SPARKBOX_STAGING_REQUIRED=1`.

What they prove, against the real node:

| Test | Contract |
|---|---|
| `staging_sudo_exchange_without_input` | `sudo_exchange` gives the password to real `sudo 1.9.15` + `use_pty` and the command runs as root; the password isn't in the output |
| `staging_sudo_exchange_with_input` | 8 KiB of every byte value survives sudo's stdin intact; a wrong password comes back as SparkBox's "password was rejected" error, and the command didn't run |
| `staging_root_helper_install_and_run` | with no root copy, `root_sparkjob_command` exits `ROOT_COPY_STALE` (97) without running; `root_install_command` writes sparkjob.py root-owned 0644 (folder 0755); the checked copy runs `df`; bytes that don't match the checksum replace nothing; a tampered copy exits 97 |
| `staging_systemd_run_upgrade_and_reattach` | `detached_command` starts the upgrade as a transient unit; it's seen active while running and gone (`LoadState=not-found`) after, with apt's `SPARKBOX-EXIT 0` in the log; `progress_of` reads the real apt log as finishing, 1 package |
| `staging_upgrade_cancel_before_install` | while apt waits before dpkg, `progress_of` says Cancel is safe and `cancel_command` (exit 0) TERMs apt-get under the unit's MainPID; the log's exit is non-zero; nothing is installed |
| `staging_reset_puts_apt_back_too` | right after sparkbox-dummy-lib 1.1 is put in with `dpkg -i` and read by `apt-get check`, the reset (its dpkg puts 1.0 back, the status file as long as before) leaves apt seeing 1.0: SparkBox's dry run of the library plans `1.0 → 1.1` in each of 3 rounds. Each round gives dpkg's status file the time it had when apt read it, the state in which a kept cache would fool apt: landing in the same second by luck failed under load, twenty rounds without one |
| `staging_holds` | `hold_command` shows in `STATUS_SCRIPT` (`parse_status`: held); the dry run keeps it back and upgrades the rest; `apt_command` naming it is refused (exit 100); the unhold lets it through |
| `staging_shipped_upgrade_and_needrestart` | C1 as built by `detached_command_opts`, through `sudo_exchange`: a `/var/log/sparkbox` left 0700 is made 0755 and `dpkg-before-<id>.txt` (root 0644) holds the package list from before apt ran, read back by `sparkuser`; the log has `SPARKBOX-AFTER` … `SPARKBOX-AFTER-END` right before `SPARKBOX-EXIT 0`; `post_checks` reads `apt-get check` (clean) and needrestart's list with `sparkbox-dummy-svc.service`; the service's pid is unchanged |
| `staging_r3_runner_keeps_apts_exit_code` | a failing postinst: `SPARKBOX-EXIT 100` (apt's code), the after-section still written; as root `dpkg --audit` names the package; as the cluster user it exits 2 with no list (finding 4) |
| `staging_upgrade_everything_pinned_with_the_stack_held` | "Upgrade everything" through `Ops` with the stack held: the C2 dry run of everything (`apt-get -s … -y dist-upgrade`, as `sparkuser`) passes the guard, keeps the stack back and plans the six routine upgrades (nothing new); Upgrade bound to that plan runs C2′ as the cluster user and one root command, C1′ with each of the six pinned at the version shown (no dist-upgrade runs any more); they are installed at those versions, and the stack and the 580 driver stay where they were |
| `staging_session_b_step21_upgrade_one` | Session B step 21 through `Ops`, bound to its plan: the dry run of libbrotli1 (C2, as the cluster user) shows libbrotli1 `1.1.0-2build2 → 1.1.0-2ubuntu0.24.04.1` and C1′'s apt-get as the command; Upgrade sends that plan back; C2′ (exactly the pinned command with `-s`) runs as the cluster user before the one sudo command, C1′ byte for byte (`--no-remove install --only-upgrade 'libbrotli1=1.1.0-2ubuntu0.24.04.1'`); the dpkg-before list is on the node and its copy in the evidence folder; `apt-get check` clean, needrestart read; `dpkg --audit` ran exactly once, as the cluster user, exit 0; the new .deb is kept in apt's cache |
| `staging_session_b_step22_reattach` | Session B step 22: the dry run with the download limit, then `updates_upgrade_opts` with `dl_limit_kbps: Some(500)` and that plan (the start's sudo row is C1′ with `-o Acquire::http::Dl-Limit=500 -o Acquire::https::Dl-Limit=500` right after `--force-confold`, and `google-chrome-stable=142.0.7444.59-1` pinned); SparkBox's Live stops mid-download; the partial download keeps growing and the unit stays active; a new Live and `Ops` on the same data folder run `reattach_upgrades`, and the picked-up task follows it to the end (Chrome at 142, exit 0; apt's own count: "Fetched 21.0 MB in 33s" to "in 34s") |
| `staging_session_b_step24_holds` | Session B step 24: `PRESET_READ` as the cluster user, `preset_packages` gives exactly the 9 stack names; "Pin the GPU stack" (`updates_hold_preset`) holds them in one sudo row (`hold_command`); showhold lists them; the dry run of everything then has none of them (kept back) and isn't refused; Hold and Release of google-chrome-stable show in showhold and the status; Release the GPU-stack pin unholds them all |
| `staging_gpu_stack_and_590_guards` | apt lists the kernel's fix from `noble-updates, noble-security` (security and GPU-stack) and libbrotli1's; `security_only` picks only libbrotli1; the unpinned dry run of everything removes `nvidia-driver-580-open` and installs `nvidia-driver-590-open`, and `check_plan` refuses it (the 590 rule first; the same plan without its 590 line is refused for the 580 removal, and without both for the stack); SparkBox's own dry run of everything is refused before the guard, the stack not being pinned ("Upgrade everything needs the GPU stack pinned first: containerd.io, dgx-spark-ota-update-meta, docker-ce, … nvidia-spark-grub-kho aren't held on staging-guards", all 9); Upgrade without a plan is refused ("Run the dry run first: …"); Upgrade everything bound to the refused plan stops at C2′ (apt can't keep 580 under `--no-remove`): "This plan removes nvidia-driver-580-open (a 580 package)", with no root command, no unit, no log; a selection naming the kernel is refused at once; Upgrade security only, bound to libbrotli1's plan, installs it (pinned) and leaves the kernel, with one sudo row among the rows it added |
| `staging_session_b_step19_prereboot` | Session B step 19's checks (C6) over a Live: kernel 7.0.0-1019-nvidia, its initrd, module 580.95.05 (the crafted nvidia.ko, real modinfo) matching libnvidia-compute-580, no DKMS; kho, GRUB_DEFAULT and the modules package recorded only; both reads as the cluster user. Then each refusal on the real node: the initrd removed, the userspace upgraded without its module (`would boot driver 580.95.05 against userspace 580.105.08`), `nvidia-dkms-580` installed; and module and userspace moved together pass again |
| `staging_prereboot_variants` | Step 19's variants (C6′) over a Live: a held `nvidia-dkms-580` (`hi`) blocks with the DKMS words; libnvidia-compute-580 removed with its config kept (`rc 580.95.05-0ubuntu1`) blocks ("libnvidia-compute-580 is removed (only its config is left): …"); `/etc/default/grub` without GRUB_DEFAULT is an ok step B with "not set" as information, and that check's step B activity row (the one new row, newest first) exits 0 (C6′'s `; true`: without it the test fails). Plans that would remove docker-ce (`sparkbox-rm-docker` 2.0) or `linux-image-7.0.0-1019-nvidia` (`sparkbox-rm-kernel` 2.0): the dry run is refused with "GPU-stack packages move only in the cluster flow: this plan removes …", and so is Upgrade bound to it, at C2′ (the guard reads apt's list of what it would remove); nothing runs as root, and both packages stay |
| `staging_plan_binding` | Decision (b) through `Ops`: sparkbox-bind 1.0 → 2.0 plans a new dependency (sparkbox-bind-dep 1.0); C1′ pins only `sparkbox-bind=2.0`, C2′ (as the cluster user, just before the start) shows the same plan, and exactly 2.0 and the dependency's 1.0 are installed. sparkbox-bind 3.0 published in place of 2.0 between the dry run and Upgrade (then `apt-get update`): refused, "apt's plan for staging-bind changed since the dry run you saw (apt-get -s: E: Version '2.0' for 'sparkbox-bind' was not found). Look at the new dry run and start again."; the dependency's 1.1 published instead: refused, "… sparkbox-bind-dep now 1.1 (was 1.0). …"; each time no root command, no unit, no log, nothing installed. sparkbox-swap 2.0 (would remove sparkbox-swap-old): the dry run and Upgrade are refused ("This plan removes sparkbox-swap-old. SparkBox's one-node upgrades only upgrade; …"); C1′ forced through as root stops on `--no-remove`: `SPARKBOX-EXIT 100`, "E: Packages need to be removed but remove is disabled.", nothing changed |
| `staging_upgrade_lost_and_picked_up_again` | R4-UPD's follower against a real sshd that goes away: Chrome's upgrade at 500 KB/s, bound to its plan; while apt downloads, sshd stops and every session is cut; after the injected 3 s wait the task ends with "Lost staging-lost while it was upgrading" and the warning naming the log; `upgrades.json` keeps the record with phase `lost`; the unit stays active. sshd back: the same `Ops` follows it again ("… (picked up again)") to "The upgrade on staging-lost finished", Chrome is at 142, and the record leaves `upgrades.json` |
| `staging_session_b_step27_first_light_shapes` | Step 27 under real dash, through `Live::run`: C-P2 (`preflight_script`) on real Ubuntu output, read by `parse_preflight`: MemTotal as /proc/meminfo has it, earlyoom `inactive` with "No such file or directory" for its config (not installed), the image's local id and Docker's store from the docker stand-in, the model folder present, `ss` empty for port 8000, the real /dev/shm (empty, then listing a file put there), the cache folders missing and then the cluster user's, no leftovers and then the load as one; C-F (`deploy-files`, the shipped sparkjob.py on Python 3.12) with a key holding quotes, a backslash, `$(id)` and a backtick: folder 700, `argv.json` and `env.json` 600 and equal to `rank_argv` and `rank_env`, key 400 holding exactly the key, the three cache folders the cluster user's, and only `{"dir": …}` printed; C-L2 (`rank_command`), one command with nothing on stdin: docker's argv is the contract's vector exactly (`$HOME` expanded to /home/sparkuser), it ran as uid 1000, and the key is in none of argv, docker's environment, any process's command line or readable environment, the activity log or ssh.log; C-U2 (`unload_command`): `ps -aq` with the instance and deployment labels, `stop -t 30 <id>`, then `rm <id>`, never `rm -f`; the container, the key file and the launch files are gone |
| `staging_multi_node_shapes` | A two-node load under real dash and Python 3.12, one staging node standing for both, through `Live`: C-P2 with `( sport = :8000 or sport = :29501 )` on real `ss`, whose real listener on 29501 makes the port not free; a key on a worker and an argv that isn't `vllm serve` refused by `deploy-files`, with nothing written; each rank's `argv.json` and `env.json` its own (`VLLM_HOST_IP` the rank's fabric address); docker's argv word for word for the worker and then the head (`--privileged` on both, no key mount on the worker, the mount and `--key` together on the head); the key in no call or activity row; C-P2 during the load lists both ranks, and real grep finds a stand-in container's `ray start --address`; C-J on real ping (`-M do -s 8972`): `lo` to 127.0.0.1 ok, `lo` to a TEST-NET address failed, a missing interface failed, read by `jumbo_failures`; C-U2 takes both ranks and their files and leaves another load's container. Every link here carries jumbo frames, so a small MTU's refusal isn't shown |
| `staging_orphan_rsync_after_sigkill` | Real rsync 3.2.7, real `kill -9`, real /proc. A pull stands still inside a file; `childStart` and `pidStart` equal `/proc/<pid>/stat`'s field 22, read with `cut`; `kill -9` of the job alone leaves both of rsync's processes running; `list --look` and `list` say `state: failed, orphan: true` and "Its copy is still running: a new copy stops it first.", `--look` writes nothing, neither stops anything, and neither `orphan` nor those words are kept in the status. The folder reads stopped, and `delete` refuses with "An earlier copy may still be writing to <dest>." and stops nothing. A new pull: a watcher reading /proc logs exactly `gone`, then `new`, and never `beside`; the new pull finishes, its bytes match, and the folder reads verified. A live `sleep` at the recorded pid with another start time isn't an orphan and is left alone, and the pull goes on. A live group leader at a dead job's own pid with another start time holds no folder (the pull goes on) and Cancel doesn't signal it. With the marker removed, a real orphan refuses the new pull with "An earlier copy may still be writing to <dest>." and is left running. A process of uid 65534 with this folder as its last word refuses the new pull, whether or not a record names it, and it's never signalled |
| `staging_session_b_step27_drop_caches` | C-D (`plan::DROP_CACHES`) through `Live::sudo` on real sudo + `use_pty`, and `Models::deploy` reading C-P2 and `check quick` as a load does: with /proc/sys read-only (as Docker leaves it) sudo runs it and it fails with "sh: 1: cannot create /proc/sys/vm/drop_caches: Read-only file system"; `Models::deploy` with the owner's consent (`drop_caches`, share 0.95, which a Docker VM never fits) says "<node>: couldn't drop the page cache: sh: 1: cannot create … Read-only file system" and starts nothing. With /proc/sys made writable for the container (`mount -o remount,bind,rw`, the test's own setup): a real drop, Cached falls (Docker Desktop's VM's page cache: harmless, the kernel refills it), and deploy reads the node again and refuses with "… free after dropping the page cache; vLLM at share 0.95 needs … Lower the share or free memory."; four sudo rows, each the command as written; the password is in neither the activity log nor ssh.log; docker `run` is never called |
| `staging_shell_conformance` | T2 blocker 2's real proof. 115 rows since P2 round 1 (C-L2 for one node, a head and a worker, C-F, C-U2, C-P2 for two nodes and C-J are rows): 91 hide a command in shell syntax, 82 of them `touch /tmp/sbconf-<n>` (a substitution, variable, glob or `${…}` as the command word, a `~` or `/tmp` path named cat, `$((a[$(…)]))` and bash's other array-subscript re-evaluations (`$((x))`, `$[x]`, `[[ -eq ]]`, `test -v`, `printf -v`, `read`, `declare -i`, `let`, and `${a[x]}` after `${x:=…}`), brace expansion, `$'\x74ouch'`, `$"…"`, heredocs (one ending early with `<<-`), a here-string, `<(…)`, `>(…)`, an alias across a newline, `hash -p`, `trap`, `eval`, `export -f`, `ENV=`, `${IFS}` and an IFS assignment, NBSP, U+2028, CR and VT before a `#`, a comment ending in a backslash, `<>`, `>|`, `&>`, `\|&`, bash's `>&word`, `( )`, `{ ;}`, the reserved words `if for while case ! time coproc function select`, `;&` and `;;&`, wrappers whose options the parsers must read as the program does (`watch` re-joining its words, and with a quoted `$(…)`; `watch -d`, `taskset -c`, `chrt -i`, `command -p` and bash's `exec -c` are flags, `xargs -i` and `--replace` take an optional value, bash's `exec -a` takes a name: read the other way, the next word, `cat`, looks like the command), seven of SparkBox's own commands (C-L2 twice, C-U2, C-P2, C-SH, C6′, C-X) with a hidden command added, bash evaluating a value read in (`[[ $y -eq 0 ]]`, `${x[y]}`, `${!y}`, `${y@P}`, `test -v "$y"`) and a quote spanning two heredocs) and 9 delete a folder of files planted before each run (the wrappers' forms again as `rm cat -rf` or `rm -rf`; the row ran when the folder is gone); 14 are reads (R4-SAFE's seven positive controls, C5's check and stamps, C6′, C-SH, C-P2 for one node and two, C-J); 10 are change literals, not run (C-L2's three, C-F's three, C-U2, C-S and C-D keep their class; C-X is recognised by its exact text, `upgrade_cancel_unit`, and no other row passes as C-X). Every hidden row ran under at least one shell. Each runs through `Live::run` under dash and as `bash -c` and is classified by `classify::verdict` and `fence_class::class_of` (and the `bash -c` form by both). Asserts: no row either parser calls a read ran its hidden command in either shell; each hidden row ran in at least one shell and is `other` to the shape parser and not a read to the classifier; the reads stay reads; the literals keep their class. It prints the whole table, and checks the shell facts above. The 21 rows added from the round 4 acceptance tester's forms (the `${a[x]}`, `>&word`, wrapper and `rm` rows) were reads to both parsers at 7ed18f0, and the shells ran them; with R4-SAFE's round 4 fixes all 101 rows pass (finding 9) |
| `staging_update_check_on_real_ubuntu` | the update check's poller round (`UpdateCache::tick`) over a Live: `CHECK_SCRIPT` parsed by `parse_check`: firmware unknown ("fwupdmgr isn't installed"), no snaps, no Ollama, no Docker; DGX OS 7.2.3 / OTA 7.6.0 (Oct 4) with 7.7.0 pending; the stamps equal `stat`'s times; 2 security fixes, waiting since that check. A round with nothing moved reads only `STAMPS`; a dpkg change moves dpkg's stamp and the next round checks in full (libbrotli1's fix gone, the kernel's still waiting since the first check); every read as the cluster user |
| `staging_channel_budget_maxsessions` | the 11th session channel on one connection is refused (MaxSessions 10) with `ConnectFailed`; a closed one's place is given back |
| `staging_updates_status_and_refresh` | "Check all nodes"' refresh (`update_check::REFRESH`, sudo) reads the repo over HTTP, and `STATUS_SCRIPT` against real apt; `parse_status` finds the pending dummy package (1.0 → 1.1) and the last refresh time |

**What this does not prove.** A container has no kernel of its own: needrestart prints no kernel
lines there (`NEEDRESTART-KSTA/KCUR/KEXP`; only the service list is proven), nothing reboots, and the
crafted module is only read, never loaded. fwupdmgr, snap, Ollama and Docker are absent here, so
only their "absent" reading is proven. The repo is unsigned (`Trusted: yes`) and has no phased
updates. The owner-present `sudo -k -S true` on cell2 stays the real `use_pty` proof, and every
command here still has to be run on the cluster in the acceptance sessions.

Round 4's limits, named. `docker` is a stand-in: first light's tests prove what dash hands to
docker (argv, environment, what any process could see) and that the unload stops before it
removes, not what Docker 29 or vLLM then do: no container, GPU, port 8000 listener or /dev/shm
segment of vLLM's exists here (the /dev/shm file the test lists is its own). `launch-proof.sh` shows
what real Docker does with C-L2, with a stand-in for vLLM; `--gpus` and `--privileged` are untested on
any Docker here. The page-cache drop
writes the Docker Desktop VM's `drop_caches` only after the test itself makes /proc/sys writable for
the container; the Sparks' /proc/sys is writable by root anyway. The conformance table proves the
rows in it; a hidden form not in it is unproven. The round 4 acceptance tester tried 78 more forms
on a scratch copy: the 12 that were reads and ran are in the table now, with 9 `rm` variants of the
wrapper ones; of the rest, 54 were already `other` and ran, and neither shell ran the other 12.
Seven of those are curl's (`-o`, `-D`, `--stderr`, `--trace-ascii`, `-K`), left out: the staging
node has no curl, since curl's libcurl needs the real libbrotli1, which the dummy repo stands in
for. The table checks SparkBox's parsers, not the exit's own verdict (`exit::verdict_of`, which
uses them).

## Findings the tier surfaced (for the owner / other packages)

1. **MaxSessions refusal reason code (resolved).** Staging showed that real OpenSSH 9.6p1, what
   the Sparks run, refuses the 11th session channel with OPEN_FAILURE reason
   `SSH_OPEN_CONNECT_FAILED` (russh `ConnectFailed`), not `AdministrativelyProhibited`. fake_node
   and `live/ssh.rs::open()` were updated to match in the round 2 integration: fake_node now
   refuses with `ConnectFailed`, and `open()` backs off on either reason.

2. **The agent leaked `NOTIFY_SOCKET` to child systemd tools (agent package, P7): fixed at the round 2
   integration.** Under real systemd, `agent_test.RunAsAService` failed: the agent spawns `journalctl`,
   `systemctl` and `timedatectl`, and each (linked against libsystemd) sent `EXIT_STATUS=0` to the
   inherited `NOTIFY_SOCKET` on exit, so the test's first datagrams were those, not the agent's
   `READY=1`. The Mac suite can never see this (those tools don't exist there). On a real node a
   `Type=notify` unit's default `NotifyAccess=main` refuses children's notifications, so the agent
   worked, but PID 1 logs a warning for each one ("Got notification message from PID …, but reception
   only permitted for main PID …"), every cycle. `sparkbox_agent.py run` now takes `NOTIFY_SOCKET` out of
   its environment when it starts and keeps it for its own `sd_notify` (`keep_notify_socket`;
   `agent_test.Watchdog.test_run_keeps_notify_socket_from_the_children_it_starts`), and `RunAsAService`
   passes in the container. `check.sh` no longer has an exemption: any failure fails the run.

3. **SparkBox's upgrade let needrestart restart services (P1.24's environment, added at the round 2
   integration).** The detached upgrade set no needrestart environment, so on stock Ubuntu 24.04
   (needrestart 3.6-7ubuntu4.5 in the container) apt's hook restarted, in the middle of the upgrade, each
   service still running a replaced library, except the ones needrestart's default config leaves alone
   (dbus, docker, networking, display managers and a few more; containerd is not on that list).
   `apt_command` sets `NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1`. Since round 3 the unit also lists
   what's left (`needrestart -b -r l` after apt), and `staging_shipped_upgrade_and_needrestart`
   checks both: the service is listed and keeps its pid.

4. **`dpkg --audit` as the cluster user can't report a problem (round 3; resolved at the round 3
   integration).** On Ubuntu 24.04 (dpkg 1.22.6), `dpkg --audit` run as `sparkuser` exits 0 with no
   output when the package database is clean, but once it finds a problem (a half-configured
   package) it checks dpkg's lock file, which is root's (0640), and fails: exit 2, `dpkg: error:
   unable to check lock file for dpkg database directory /var/lib/dpkg: Permission denied`, and no
   list. As root it lists the package. The Updates task's check after an upgrade runs `dpkg --audit`
   without sudo. Until the integration it read only the output, so in that state it would have said
   "No broken or half-installed packages". Now a non-zero exit with nothing listed fails the task
   as "The upgrade finished, but SparkBox couldn't check the package database on <node>. Run sudo
   dpkg --audit there." (`ops::updates` test `an_audit_that_couldnt_read_the_database_is_no_all_clear`,
   against fake_node, whose `dpkg --audit` answers the cluster user as dpkg 1.22 does here).
   `staging_r3_runner_keeps_apts_exit_code` shows the dpkg behaviour that made it necessary, on the
   real node. (A failing package usually fails apt first, with exit 100, so the task fails anyway;
   the audit is the check for what apt leaves behind.)

5. **apt's download limit runs a little over its number.** With `Dl-Limit=500` the 20 MiB package
   came down at 611–632 kB/s by apt's own count (33–34 s, four runs) over the container's HTTP: limited, about
   a fifth over 500 KiB/s. Session B step 22's "tens of seconds" holds; a real repo is slower still.

6. **Six hidden commands were reads to the p1-r3 parsers (round 4; for R4-SAFE, T2 blocker 2).**
   The conformance table run against HEAD (`p1-r3`, 8ebe8d9) with only this test added: both
   `classify::verdict` and `fence_class::class_of` called these reads, and real dash and real bash
   both ran the hidden `touch`: `~/sbconf-bin-7/cat /etc/hostname` and `/tmp/sbconf-bin-8/cat
   /etc/hostname` (a file named cat outside the system folders), and `uptime<X>#;touch
   /tmp/sbconf-N` with `<X>` a no-break space (U+00A0), U+2028, CR or VT: SparkBox's parsers took
   `<X>` for a space and `#;touch …` for a comment; the shells, in the C locale, take `uptime<X>#`
   for one word (command not found) and run `touch`. Every other hidden row was already a change.
   With R4-SAFE's round 4 parsers (the same table against the round 4 tree, 2026-10-30 14:25 EDT)
   none of them is a read: the `~` path and the four whitespace rows are destructive, the `/tmp`
   path a change, and all six are `other` to the shape parser. Both tables are kept with the
   round's records (`conformance-head.txt`, `conformance-final.txt`).

7. **Round 3's check before a restart read a removed libnvidia-compute-580 as installed (round 4;
   fixed by C6′).** On real dpkg 1.22.6 the round 3 compute line, `dpkg-query -W
   -f='${Version}\n' libnvidia-compute-580`, prints `580.95.05-0ubuntu1` for a package removed
   with its config kept (`rc`), the same as for an installed one; C6′'s
   `${db:Status-Abbrev}${Version}` prints `rc 580.95.05-0ubuntu1`, which now blocks
   (`staging_prereboot_variants`).

8. **A GPU-stack removal was refused in two sets of words (round 4; for R4-UPD; resolved in the
   round).** The dry run of an upgrade that would remove docker-ce (C2) is refused by the GPU-stack
   guard ("GPU-stack packages move only in the cluster flow: this plan removes docker-ce"). Upgrade
   bound to that plan runs C2′, where `--no-remove` makes apt stop before it prints its plan lines,
   so the removal is only in apt's "will be REMOVED" list. Mid-round, that list reached only the
   removal rule ("This plan removes docker-ce. SparkBox's one-node upgrades only upgrade; …") and
   the guard never saw it; `staging_prereboot_variants` showed it on real apt. R4-UPD now runs the
   guard on that list first, and the test checks that both refusals use the guard's words (and the
   580 rule's for Upgrade everything: "This plan removes nvidia-driver-580-open (a 580 package)").
   Both always refused before anything ran as root.

9. **Twenty-one hidden forms were reads to the round 4 parsers (round 4, from the acceptance
   tester's forms; for R4-SAFE, T2 blocker 2).** The round 4 acceptance tester found twelve forms
   both `classify::verdict` and `fence_class::class_of` call reads while real dash or bash runs
   the hidden command: a wrapper option read the wrong way, so the word after it (`cat`) looks
   like the command (`watch -d`, `taskset -c`, `chrt -i`, `command -p` and bash's `exec -c` are
   flags; `xargs -i` and `--replace` take an optional value; bash's `exec -a` takes a name),
   `watch` joining its words into one `sh -c` (`watch -n 1 uptime ';' touch …`, and a quoted
   `$(…)`), bash's `>&word` (stdout and stderr to a file), and `${a[x]}` re-evaluating what
   `${x:=…}` assigned. With nine `rm` variants (`taskset -c 0 rm cat -rf <folder>` and the like,
   which delete a planted folder of files) they are rows of `staging_shell_conformance`. On
   2026-10-30 at 15:56 EDT (`ops/classify.rs` and `live/fence_class.rs` as at 7ed18f0) all 21
   were reads to both parsers and ran, 15 under both shells and 6 under bash only; the other 80
   rows were as before, and the test failed. With R4-SAFE's round 4 fixes in the working tree
   (16:05 EDT, before the round's integration) none of them is a read: the `${a[x]}` row is
   destructive to the classifier, the other 20 are changes, and all 21 are `other` to the shape
   parser; all 101 rows pass, and so does the whole staging suite (24 tests). Both tables are kept
   with the round's records (`~/Library/Caches/sparkbox-rounds/p1/r4/stage-fix/run-1.txt`,
   `run-full-3.txt`). The integrator added R4-SAFE's six fix-phase rows (102–107: bash evaluating a
   value read in, and a quote spanning two heredocs); on the final tree (2026-10-30, after the
   round's end switches) all 107 rows pass and so does the whole staging suite, 24 of 24
   (`~/Library/Caches/sparkbox-rounds/p1/r4/final/staging.txt`).

10. **Docker Desktop stalled starting containers (P2 round 1; cause not found).** On 2026-11-01 from
    about 12:25 to 12:36 EST, every new `docker run` on this Mac sat in `Created` for up to 10 minutes
    while `docker ps`, `docker exec` and the running containers answered normally and the VM stood
    idle: `launch-proof.sh`, another builder's `setup.sh` and a bare `docker run ubuntu true` alike.
    They all went on by themselves and the next `launch-proof.sh` took 8 s. A script here that seems
    to hang at `docker run` may be this; `docker ps -a` shows `Created`.

## Security

The container is throwaway and local. Its sshd is bound to `127.0.0.1` only; the dummy repo's HTTP
server listens on the container's own loopback and isn't published. The password is random per
container and lives only inside it; the tests read it over a `docker exec` pipe (never an
argument, never logged) and send it to sudo on stdin, the same way SparkBox does. Nothing here
touches the cluster, the Keychain, the release app, or `~/Library/Application Support`.

## Rollback

`scripts/staging/teardown.sh` removes every `sparkbox-staging-*` container, the empty lock
files in `$TMPDIR`, the tests' pinned host keys next to them
(`sparkbox-staging-spark1.known_hosts.json`) and their data folders
(`sparkbox-staging-<test>-<pid>`: a Live's known hosts, `ssh.log` and the evidence copies of the
container's package lists). With `--image` it removes every image staging built: those labelled
`sparkbox.staging=1`, round 2's (labelled `com.nemecj.sparkbox.staging=image`) and the
`sparkbox-staging:24.04` tag, old untagged builds included. While a `check.sh`, `setup.sh` or
staging test holds a lock it removes nothing and says so. Then delete `scripts/staging/` and
`src-tauri/src/live/staging_tests.rs` with its `mod staging_tests;` line in `live/mod.rs`.
Round 4's tests leave nothing on this Mac beyond those data folders: what they change in the
container (the archive-only dummies, a published repo, /etc/default/grub, a writable /proc/sys, the
docker stand-in's state) the reset puts back, and teardown removes the container anyway. It
removes nothing it didn't create — not the base `ubuntu:24.04`, no other image or container, not
Docker Desktop, not the `nvidia-workbench` machine. Round 2's unlabelled `sbproto:1` image is
neither labelled nor named as staging's, so teardown leaves it to the owner (it wasn't on this Mac
when round 3 or round 4 started).
