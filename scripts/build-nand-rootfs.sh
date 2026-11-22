#!/bin/sh
# Builds the OpenMinerOS root filesystem (squashfs) for a NAND firmware slot
# of an S19 X19 control board. Runs inside the builder container.
#
#   build-nand-rootfs.sh <out-dir> <pool host:port|-> <user|-> <root-password>
#
# Pass "-" for pool and user to build a public image without a pool: the
# miner then waits until pools are set in the web UI.
#
# The kernel is not built here: on a Braiins OS unit the installer reuses the
# kernel FIT of the running slot (see scripts/install-slot2.sh).
set -eu
out=${1:?out dir}
pool=${2:?pool}
user=${3:?user}
rootpw=${4:?root password}
target=armv7-unknown-linux-musleabihf
src="$(cd "$(dirname "$0")/.." && pwd)"
bin="${CARGO_TARGET_DIR:-$src/target}/$target/release"

command -v mksquashfs >/dev/null || { apt-get update -qq && apt-get install -y -qq squashfs-tools >/dev/null; }

CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER=rust-lld \
  cargo build --manifest-path "$src/Cargo.toml" --release --target "$target" \
    -p openmineros-control-plane -p openmineros-board-xil

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
r="$stage/rootfs"
mkdir -p "$r"
cp -R "$src/image/rootfs/." "$r/"
rm -f "$r/init" "$r/usr/sbin/omo-sdlog"
mkdir -p "$r/bin" "$r/sbin" "$r/usr/bin" "$r/usr/sbin" "$r/dev" "$r/proc" "$r/sys" \
  "$r/tmp" "$r/run" "$r/root" "$r/etc/openmineros" "$r/var" "$r/data"

busybox="$(dpkg -L busybox-static:armhf | grep -E '/bin/busybox$' | head -n 1)"
install -m 0755 "$busybox" "$r/bin/busybox"
for applet in $(qemu-arm-static "$busybox" --list-full); do
  mkdir -p "$r/$(dirname "$applet")"
  [ -e "$r/$applet" ] || ln -s /bin/busybox "$r/$applet"
done
for b in omo-minerd omo-chain-probe omo-bootenv; do install -m 0755 "$bin/$b" "$r/usr/bin/$b"; done
install -m 0755 "$bin/openmineros-control-plane" "$r/usr/bin/openmineros-control-plane"

salt="$(head -c 12 /dev/urandom | base64 | tr -dc 'a-zA-Z0-9' | head -c 12)"
hash="$(perl -e 'print crypt($ARGV[0], "\$6\$" . $ARGV[1] . "\$")' "$rootpw" "$salt")"
case "$hash" in '$6$'*) ;; *) echo "password hashing failed" >&2; exit 1 ;; esac
echo "root:$hash:19000:0:99999:7:::" > "$r/etc/shadow"
chmod 0600 "$r/etc/shadow"
echo omo-s19 > "$r/etc/hostname"
ln -sf /tmp/resolv.conf "$r/etc/resolv.conf"
ln -sf /tmp "$r/var/log"

printf 'root:%s\n' "$(printf '%s' "$rootpw" | sha256sum | cut -d' ' -f1)" > "$r/etc/openmineros/web-auth"
chmod 0600 "$r/etc/openmineros/web-auth"
if [ "$pool" = - ] || [ "$user" = - ]; then pools='[]'; else pools="[ { \"url\": \"$pool\", \"user\": \"$user\", \"password\": \"x\" } ]"; fi
cat > "$r/etc/openmineros/default-config.json" <<EOF
{
  "pools": $pools,
  "freq_mhz": 109.0,
  "voltage": 11.96,
  "fan": { "mode": "auto", "target_temp": 50.0, "min_percent": 30, "manual_percent": 60 },
  "max_pcb_temp": 80.0
}
EOF

cat > "$r/etc/init.d/rcS" <<'EOF'
#!/bin/sh
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t tmpfs -o mode=1777 tmpfs /tmp
mount -t tmpfs -o mode=0755,nosuid,nodev tmpfs /run
if ! grep -q '^devtmpfs /dev ' /proc/mounts; then
	mount -t tmpfs -o mode=0755 tmpfs /dev
	echo /sbin/mdev > /proc/sys/kernel/hotplug
	mdev -s
fi
mkdir -p /dev/pts /dev/shm
mount -t devpts devpts /dev/pts
hostname -F /etc/hostname
ip link set lo up
ip link set eth0 up
udhcpc -i eth0 -b -t 20 -s /usr/share/udhcpc/default.script -x "hostname:$(hostname)"
mount -t ubifs ubi0:rootfs_data /data 2>/dev/null || { mount -t tmpfs -o mode=0755 tmpfs /data; echo "rcS: no UBI rootfs_data (SD boot?), /data is tmpfs" >> /tmp/boot.log; }
mkdir -p /data/omo
[ -s /data/omo/config.json ] || cp /etc/openmineros/default-config.json /data/omo/config.json
# A password set in the web UI also applies to telnet.
[ -s /data/omo/shadow ] && mount --bind /data/omo/shadow /etc/shadow
echo "rcS done $(date)" >> /tmp/boot.log
EOF

cat > "$r/usr/sbin/omo-minerd-run" <<'EOF'
#!/bin/sh
# Keeps omo-minerd running. It exits after a configuration change (SIGTERM
# from the API) or on an error; errors back off for a minute.
for i in $(seq 1 60); do ip -4 addr show eth0 | grep -q inet && break; sleep 2; done
while true; do
	echo "== start $(date)" >> /tmp/omo-minerd.log
	/usr/bin/omo-minerd --config /data/omo/config.json >> /tmp/omo-minerd.json 2>> /tmp/omo-minerd.log
	rc=$?
	echo "== exit $rc $(date)" >> /tmp/omo-minerd.log
	tail -n 2000 /tmp/omo-minerd.log > /tmp/omo-minerd.log.1 && mv /tmp/omo-minerd.log.1 /tmp/omo-minerd.log
	: > /tmp/omo-minerd.json
	if [ $rc -eq 0 ]; then sleep 5; else sleep 60; fi
done
EOF
chmod 0755 "$r/usr/sbin/omo-minerd-run" "$r/etc/init.d/rcS"

cat > "$r/etc/inittab" <<'EOF'
::sysinit:/etc/init.d/rcS
::once:/usr/sbin/omo-minerd-run
::respawn:/usr/bin/openmineros-control-plane --fw-info /etc/openmineros/fw-info.json --model s19 --listen 0.0.0.0:80
::respawn:/usr/sbin/telnetd -F -l /bin/login
::respawn:/sbin/watchdog -F -T 60 -t 10 /dev/watchdog
::shutdown:/bin/sh -c 'kill -9 $(pidof watchdog) 2>/dev/null; true'
::shutdown:/usr/bin/killall -TERM omo-minerd-run
::shutdown:/usr/bin/killall -TERM omo-minerd
::shutdown:/bin/sleep 8
::shutdown:/bin/umount -a -r
::restart:/sbin/init
EOF

cat > "$r/etc/openmineros/fw-info.json" <<EOF
{ "name": "OpenMinerOS", "version": "0.1.0", "board": "xilinx", "install_type": "nand", "source_commit": "${SOURCE_COMMIT:-unknown}" }
EOF

mknod -m 0600 "$r/dev/console" c 5 1
mknod -m 0666 "$r/dev/null" c 1 3

mkdir -p "$out"
mksquashfs "$r" "$out/openmineros-s19-rootfs.squashfs" -comp xz -all-root -noappend -quiet
ls -la "$out/openmineros-s19-rootfs.squashfs"
sha256sum "$out/openmineros-s19-rootfs.squashfs"
