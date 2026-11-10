#!/bin/sh
# Builds the staging node's dummy apt repo into the image (setup.sh runs it in the Dockerfile).
# Each package is an empty stand-in: the name and version a Spark has, installed, and a newer
# version in the repo, so SparkBox's update paths meet real apt, dpkg and kmod with the names they
# look for. None carries real software.
#
#   /srv/sparkbox-repo/pool/<pool>/<name>_<version>_all.deb
#   /srv/sparkbox-repo/dists/<suite>/main/binary-arm64/Packages   (suites below)
#
# Suites, served over HTTP on 127.0.0.1:8080 inside the container (sparkbox-staging-repo.service):
#   noble           every installed version (pool/noble, installed at build and by the tests' reset)
#                   and nvidia-dkms-580 (pool/extra, in the archive but not installed)
#   noble-updates   the newer versions (pool/noble-updates) and the security ones (pool/noble-security),
#                   as Ubuntu publishes a security fix in both pockets
#   noble-security  the security fixes: libbrotli1 and the kernel
#   stable          google-chrome-stable's newer version, as Google's own repo has it
#
# The packages:
#   sparkbox-dummy        1.1's postinst prints a line a second for 6 s (follow, reattach, Cancel)
#   sparkbox-dummy-held   for apt-mark hold
#   sparkbox-dummy-lib    a library sparkbox-dummy-svc.service keeps loaded (needrestart)
#   sparkbox-dummy-fail   1.1's postinst fails while /run/sparkbox-staging-fail-postinst exists (apt exits 100)
#   libbrotli1            a routine security fix (Session B step 21), noble-security
#   google-chrome-stable  142's package is 20 MiB of random bytes, so a download limited to
#                         500 KB/s takes about 40 s (Session B step 22, the reattach)
#   the GPU stack         nvidia-driver-580-open, libnvidia-compute-580 (580.95.05 -> 580.105.08),
#                         linux-image-7.0.0-1019-nvidia (the newer one in noble-security; it ships
#                         /boot/vmlinuz-K and its postinst makes /boot/initrd.img-K and the
#                         /boot/vmlinuz link, as the kernel's hooks do), linux-modules-nvidia-580-open-K
#                         (a crafted nvidia.ko: an ELF whose .modinfo says version=<driver version>,
#                         indexed with depmod, so real modinfo reads it; it can't be loaded), docker-ce,
#                         containerd.io, nvidia-container-toolkit, nvidia-spark-grub-kho (installed only),
#                         dgx-spark-ota-update-meta (7.6.0 depends on nvidia-driver-580-open; 7.7.0 on
#                         nvidia-driver-590-open, so a dist-upgrade would install 590 and remove 580)
#   nvidia-driver-590-open  Conflicts/Replaces nvidia-driver-580-open, not installed
#   nvidia-dkms-580       not installed (the pre-reboot check's dkms variant installs it)
#   libnvidia-compute-580 ships a conffile, so `dpkg -r` leaves it `rc` (the pre-reboot rc variant)
#
# Round 4's, in the archive (pool/extra) but not installed, so nothing else sees them until a test
# installs their 1.0 with dpkg:
#   sparkbox-bind         2.0 depends on sparkbox-bind-dep, new (the plan binding: a new dependency)
#   sparkbox-swap         2.0 conflicts with sparkbox-swap-old, which it would remove (a plan with a removal)
#   sparkbox-rm-docker    2.0 conflicts with docker-ce (a GPU-stack removal)
#   sparkbox-rm-kernel    2.0 conflicts with linux-image-7.0.0-1019-nvidia (a GPU-stack removal)
# and, unpublished in pool/later (a test publishes one, as a repo would, and the reset puts the
# repo back from /srv/sparkbox-repo.orig): sparkbox-bind 3.0 and sparkbox-bind-dep 1.1, each in place
# of the version noble-updates has (an Ubuntu pocket lists one version of a package).
#
# The indexes are made by /usr/local/sbin/sparkbox-staging-index (written here), which a test runs
# again after it publishes.
set -eu
repo=/srv/sparkbox-repo
K=7.0.0-1019-nvidia
work=$(mktemp -d)
mkdir -p "$repo/pool"
fields="" post="" zip=""

# deb <pool> <name> <version>: builds the package into pool/<pool>/ from the files already under
# $work/<name>_<version>/ (if any), with the control lines in $fields and a postinst that runs
# $post on configure. $fields, $post and $zip (dpkg-deb's compressor) are cleared after.
deb() {
  d="$work/$2_$3"
  mkdir -p "$d/DEBIAN" "$repo/pool/$1"
  {
    printf 'Package: %s\nVersion: %s\nArchitecture: all\nMaintainer: SparkBox staging <staging@localhost>\n' "$2" "$3"
    [ -z "$fields" ] || printf '%s\n' "$fields"
    printf 'Description: SparkBox staging stand-in for %s (no real software)\n' "$2"
  } > "$d/DEBIAN/control"
  if [ -n "$post" ]; then
    printf '#!/bin/sh\nset -e\nif [ "$1" = configure ]; then\n%s\nfi\n' "$post" > "$d/DEBIAN/postinst"
    chmod 0755 "$d/DEBIAN/postinst"
  fi
  # The file name has no epoch (docker-ce's "5:").
  dpkg-deb "-Z${zip:-xz}" --build --root-owner-group "$d" "$repo/pool/$1/$2_${3#*:}_all.deb" >/dev/null
  fields="" post="" zip=""
}

# kernel <version> <pool>: linux-image-K, with what its postinst hooks leave in /boot.
kernel() {
  d="$work/linux-image-${K}_$1"
  mkdir -p "$d/boot" "$d/lib/modules/$K"
  printf 'Linux version %s (SparkBox staging stand-in, not a kernel) #%s\n' "$K" "$1" > "$d/boot/vmlinuz-$K"
  : > "$d/lib/modules/$K/modules.order"
  : > "$d/lib/modules/$K/modules.builtin"
  : > "$d/lib/modules/$K/modules.builtin.modinfo"
  post="printf 'SparkBox staging stand-in initrd for $K\n' > /boot/initrd.img-$K
ln -sfn vmlinuz-$K /boot/vmlinuz
ln -sfn initrd.img-$K /boot/initrd.img
depmod -a $K"
  deb "$2" "linux-image-$K" "$1"
}

# modules <version> <driver version> <pool>: linux-modules-nvidia-580-open-K with an nvidia.ko
# whose .modinfo carries the driver version (objcopy turns the strings into an aarch64 ELF).
modules() {
  d="$work/linux-modules-nvidia-580-open-${K}_$1"
  mkdir -p "$d/lib/modules/$K/kernel/nvidia-580-open"
  printf 'version=%s\0license=Dual MIT/GPL\0description=SparkBox staging stand-in for the NVIDIA module (only a .modinfo, not loadable)\0name=nvidia\0vermagic=%s SMP preempt mod_unload modversions aarch64\0' "$2" "$K" > "$work/modinfo.bin"
  (cd "$work" && objcopy -I binary -O elf64-littleaarch64 -B aarch64 --rename-section .data=.modinfo,alloc,load,readonly,data,contents modinfo.bin "$d/lib/modules/$K/kernel/nvidia-580-open/nvidia.ko")
  post="depmod -a $K"
  deb "$3" "linux-modules-nvidia-580-open-$K" "$1"
}

# What the round 2 tests use.
for v in 1.0 1.1; do
  pool=noble
  [ "$v" = 1.0 ] || pool=noble-updates
  deb "$pool" sparkbox-dummy-held "$v"
  mkdir -p "$work/sparkbox-dummy-lib_$v/usr/lib/sparkbox-dummy-lib"
  cp "$(readlink -f /usr/lib/aarch64-linux-gnu/libz.so.1)" "$work/sparkbox-dummy-lib_$v/usr/lib/sparkbox-dummy-lib/libsparkbox-dummy.so"
  deb "$pool" sparkbox-dummy-lib "$v"
done
deb noble sparkbox-dummy 1.0
post='for i in 1 2 3 4 5 6; do echo "sparkbox-dummy: configuring, step $i of 6"; sleep 1; done'
deb noble-updates sparkbox-dummy 1.1
deb noble sparkbox-dummy-fail 1.0
post='if [ -e /run/sparkbox-staging-fail-postinst ]; then echo "sparkbox-dummy-fail: failing on purpose (/run/sparkbox-staging-fail-postinst)" >&2; exit 1; fi'
deb noble-updates sparkbox-dummy-fail 1.1

# Routine fixes (Session B steps 21 and 22).
deb noble libbrotli1 1.1.0-2build2
deb noble-security libbrotli1 1.1.0-2ubuntu0.24.04.1
deb noble google-chrome-stable 141.0.7390.65-1
mkdir -p "$work/google-chrome-stable_142.0.7444.59-1/opt/google/chrome"
head -c 20971520 /dev/urandom > "$work/google-chrome-stable_142.0.7444.59-1/opt/google/chrome/sparkbox-staging-payload.bin"
zip=none
deb stable google-chrome-stable 142.0.7444.59-1

# The GPU stack, as a Spark names it.
deb noble nvidia-driver-580-open 580.95.05-0ubuntu1
deb noble-updates nvidia-driver-580-open 580.105.08-0ubuntu1
for v in noble:580.95.05-0ubuntu1 noble-updates:580.105.08-0ubuntu1; do
  d="$work/libnvidia-compute-580_${v#*:}"
  mkdir -p "$d/etc/sparkbox-staging" "$d/DEBIAN"
  printf '# SparkBox staging: a conffile, so a removed package keeps its config (dpkg state rc)\n' > "$d/etc/sparkbox-staging/libnvidia-compute-580.conf"
  echo /etc/sparkbox-staging/libnvidia-compute-580.conf > "$d/DEBIAN/conffiles"
  deb "${v%%:*}" libnvidia-compute-580 "${v#*:}"
done
kernel 7.0.0-1019.19 noble
kernel 7.0.0-1019.20 noble-security
modules 7.0.0-1019.19 580.95.05 noble
modules 7.0.0-1019.20 580.105.08 noble-updates
deb noble docker-ce 5:29.0.0-1~ubuntu.24.04~noble
deb noble-updates docker-ce 5:29.0.1-1~ubuntu.24.04~noble
deb noble containerd.io 1.7.28-1~ubuntu.24.04~noble
deb noble-updates containerd.io 1.7.29-1~ubuntu.24.04~noble
deb noble nvidia-container-toolkit 1.18.0-1
deb noble-updates nvidia-container-toolkit 1.18.1-1
deb noble nvidia-spark-grub-kho 1.0
fields='Depends: nvidia-driver-580-open'
deb noble dgx-spark-ota-update-meta 7.6.0
fields='Depends: nvidia-driver-590-open'
deb noble-updates dgx-spark-ota-update-meta 7.7.0
fields='Conflicts: nvidia-driver-580-open
Replaces: nvidia-driver-580-open'
deb noble-updates nvidia-driver-590-open 590.44.01-0ubuntu1
deb extra nvidia-dkms-580 580.95.05-0ubuntu1

# Round 4: the plan binding and the removals (see the top).
deb extra sparkbox-bind 1.0
fields='Depends: sparkbox-bind-dep (>= 1.0)'
deb noble-updates sparkbox-bind 2.0
deb noble-updates sparkbox-bind-dep 1.0
fields='Depends: sparkbox-bind-dep (>= 1.0)'
deb later sparkbox-bind 3.0
deb later sparkbox-bind-dep 1.1
deb extra sparkbox-swap 1.0
deb extra sparkbox-swap-old 1.0
fields='Conflicts: sparkbox-swap-old'
deb noble-updates sparkbox-swap 2.0
deb extra sparkbox-rm-docker 1.0
fields='Conflicts: docker-ce'
deb noble-updates sparkbox-rm-docker 2.0
deb extra sparkbox-rm-kernel 1.0
fields="Conflicts: linux-image-$K"
deb noble-updates sparkbox-rm-kernel 2.0

# The suites' Packages (all their pools) and Release, from what is in the pools now.
cat > /usr/local/sbin/sparkbox-staging-index <<'EOF'
#!/bin/sh
# Makes the staging repo's indexes from its pools (scripts/staging/image/build-repo.sh): a test
# runs it again after it moves a package into or out of a pool, then apt-get update.
set -eu
repo=/srv/sparkbox-repo
tmp=$(mktemp)
# index <suite> <codename> <pool>...
index() {
  s=$1 c=$2
  shift 2
  dir="$repo/dists/$s/main/binary-arm64"
  mkdir -p "$dir"
  (cd "$repo" && for p in "$@"; do apt-ftparchive packages "pool/$p"; done) > "$dir/Packages"
  rm -f "$dir/Packages.gz"
  gzip -9kn "$dir/Packages"
  (cd "$repo" && apt-ftparchive -o APT::FTPArchive::Release::Origin="SparkBox staging" -o APT::FTPArchive::Release::Label="SparkBox staging" \
    -o APT::FTPArchive::Release::Suite="$s" -o APT::FTPArchive::Release::Codename="$c" \
    -o APT::FTPArchive::Release::Architectures=arm64 -o APT::FTPArchive::Release::Components=main release "dists/$s") > "$tmp"
  mv "$tmp" "$repo/dists/$s/Release"
}
index noble noble noble extra
index noble-updates noble noble-updates noble-security
index noble-security noble noble-security
index stable stable stable
chmod -R a+rX "$repo"
EOF
chmod 0755 /usr/local/sbin/sparkbox-staging-index
/usr/local/sbin/sparkbox-staging-index

# Installed at the Spark's versions; the tests' reset puts these back.
ls "$repo"/pool/noble/*.deb > "$repo/installed.list"
# shellcheck disable=SC2046
dpkg -i $(cat "$repo/installed.list") >/dev/null
rm -rf "$work"
# What the reset puts back after a test published something.
cp -a "$repo" "$repo.orig"
