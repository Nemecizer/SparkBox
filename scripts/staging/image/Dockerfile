# The staging Spark's image (PLAN P1.S), built by scripts/staging/setup.sh as sparkbox-staging:24.04
# with the label sparkbox.staging=1 (teardown.sh removes every image with it). setup.sh labels the
# image and its container with a hash of this folder too, and refuses a container an older one made.
#
# ubuntu:24.04 for arm64 as pulled on 2026-10-30, pinned so the base doesn't move under the tests.
FROM ubuntu:24.04@sha256:fe81560990fae86dc4743202b2cb4d3df98d0bcf949173cecef15ce1127ef852
ENV DEBIAN_FRONTEND=noninteractive
# What a Spark has that SparkBox's write paths touch. apt-utils only builds the dummy repo's index;
# kmod is modinfo and depmod (the pre-reboot check); binutils' objcopy makes the crafted nvidia.ko;
# iproute2 is ss (a load's pre-flight reads the port with it); iputils-ping is the jumbo-frame check;
# fdisk, file, parted, pciutils, ripgrep, sysstat and tree are what staging_read_tier_options tries on the real tools.
RUN apt-get update && apt-get install -y --no-install-recommends \
      openssh-server sudo python3 systemd systemd-sysv dbus needrestart procps rsync apt-utils kmod binutils iproute2 iputils-ping \
      fdisk file parted pciutils ripgrep sysstat tree \
    && rm -rf /var/lib/apt/lists/*
# The cluster user, as on the Sparks: uid 1000, in sudo and adm, bash, Ubuntu's stock dotfiles.
RUN userdel -r ubuntu && useradd -m -u 1000 -s /bin/bash -G adm,sudo,users,audio,dip,plugdev sparkuser
# There's no Docker inside: /usr/local/bin/docker is a stand-in (docker-standin.py says what it
# answers) that records each call in /var/lib/sparkbox-staging-docker, the cluster user's, as the
# docker group makes Docker the cluster user's on a Spark.
COPY --chmod=0755 docker-standin.py /usr/local/bin/docker
RUN install -d -o sparkuser -g sparkuser -m 0755 /var/lib/sparkbox-staging-docker
# The dummy repo (build-repo.sh says what is in it). libbrotli1 is a dummy too: make sure the real
# one isn't installed, so nothing here depends on it.
COPY build-repo.sh /tmp/build-repo.sh
RUN ! dpkg -s libbrotli1 >/dev/null 2>&1 && sh /tmp/build-repo.sh && rm /tmp/build-repo.sh
COPY <<'EOF' /etc/systemd/system/sparkbox-dummy-svc.service
[Unit]
Description=SparkBox staging: keeps sparkbox-dummy-lib loaded
[Service]
ExecStart=/usr/bin/python3 -c "import ctypes, time; ctypes.CDLL('/usr/lib/sparkbox-dummy-lib/libsparkbox-dummy.so'); time.sleep(10**9)"
[Install]
WantedBy=multi-user.target
EOF
# The repo over HTTP, on the container's own loopback only (not published), so apt's download
# limit (Acquire::http::Dl-Limit) applies as on a node; file: sources ignore it.
COPY <<'EOF' /etc/systemd/system/sparkbox-staging-repo.service
[Unit]
Description=SparkBox staging: the dummy apt repo on http://127.0.0.1:8080/
[Service]
User=nobody
ExecStart=/usr/bin/python3 -m http.server 8080 --bind 127.0.0.1 --directory /srv/sparkbox-repo
[Install]
WantedBy=multi-user.target
EOF
COPY <<'EOF' /etc/apt/sources.list.d/sparkbox-staging.sources
Types: deb
URIs: http://127.0.0.1:8080/
Suites: noble noble-updates noble-security stable
Components: main
Trusted: yes
EOF
# As on a Spark: update-notifier-common's stamp after each successful apt-get update (the update
# check reads its time), and a GRUB_DEFAULT for the pre-reboot check to record. The Docker image's
# docker-clean goes: it deletes every downloaded .deb, which a node keeps
# (APT::Keep-Downloaded-Packages, the rollback copies). Cancel needs apt still running before dpkg:
# with /run/sparkbox-staging-slow-apt present (a test makes it) apt waits 8 s before it calls dpkg.
COPY <<'EOF' /etc/apt/apt.conf.d/15update-stamp
APT::Update::Post-Invoke-Success {"touch /var/lib/apt/periodic/update-success-stamp 2>/dev/null || true";};
EOF
COPY <<'EOF' /etc/apt/apt.conf.d/50sparkbox-staging
DPkg::Pre-Invoke { "if [ -e /run/sparkbox-staging-slow-apt ]; then sleep 8; fi"; };
EOF
# /etc/dgx-release as DGX OS writes it (a Spark's DGX_* lines, without its serial number): the
# build, then a block per OTA (7.5.0, then 7.6.0), for the update check's DGX OS read.
COPY <<'EOF' /etc/dgx-release
DGX_NAME="DGX Spark"
DGX_PRETTY_NAME="NVIDIA DGX Spark"
DGX_SWBUILD_DATE="2025-09-10-13-50-03"
DGX_SWBUILD_VERSION="7.2.3"
DGX_PLATFORM="MS-C931"

DGX_OTA_VERSION="7.5.0"
DGX_OTA_DATE="Sun Apr 19 10:38:20 EDT 2026"

DGX_OTA_VERSION="7.6.0"
DGX_OTA_DATE="Sun Oct  4 11:41:51 EDT 2026"
EOF
COPY <<'EOF' /etc/default/grub
GRUB_DEFAULT=0
GRUB_TIMEOUT_STYLE=hidden
GRUB_TIMEOUT=0
GRUB_CMDLINE_LINUX_DEFAULT=""
EOF
# The reset puts /etc/default/grub back from here (a test removes its GRUB_DEFAULT).
RUN install -d /usr/share/sparkbox-staging && cp /etc/default/grub /usr/share/sparkbox-staging/grub
# apt sees only the dummy repo, so every apt-get update in a test is local and quick. The lists are
# read once here, with the repo served for the moment.
RUN set -eu; \
    mv /etc/apt/sources.list.d/ubuntu.sources /etc/apt/sources.list.d/ubuntu.sources.disabled; \
    rm -f /etc/apt/apt.conf.d/docker-clean; \
    mkdir -p /var/lib/apt/periodic; \
    python3 -m http.server 8080 --bind 127.0.0.1 --directory /srv/sparkbox-repo >/dev/null 2>&1 & pid=$!; \
    i=0; until python3 -c 'import urllib.request; urllib.request.urlopen("http://127.0.0.1:8080/dists/noble/Release", timeout=1)' 2>/dev/null; do \
      i=$((i + 1)); [ "$i" -lt 50 ] || exit 1; sleep 0.2; done; \
    apt-get update; \
    kill "$pid"; \
    systemctl enable sparkbox-dummy-svc.service sparkbox-staging-repo.service
# Host keys are made per container at its first start (setup.sh), not baked into the image.
RUN rm -f /etc/ssh/ssh_host_*
STOPSIGNAL SIGRTMIN+3
