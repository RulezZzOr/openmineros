#!/bin/sh
# Installs OpenMinerOS into the second NAND firmware slot of an S19 X19
# control board that runs Braiins OS from slot 1, the same way Braiins
# upgrades itself: UBI volumes kernel/rootfs/rootfs_data on mtd "firmware2"
# and the U-Boot variables `firmware` and `upgrade_stage`.
#
# Trial boot: with upgrade_stage=0 U-Boot boots slot 2 once and sets
# upgrade_stage=1; OpenMinerOS does not clear it, so the next reboot or power
# cycle returns to Braiins OS in slot 1 automatically.
#
# Run on the miner:  sh install-slot2.sh /tmp/openmineros-s19-rootfs.squashfs [--reboot]
set -eu
ROOTFS=${1:?rootfs.squashfs}
REBOOT=${2:-}

die() { echo "install-slot2: $*" >&2; exit 1; }

[ "$(fw_printenv -n firmware)" = 1 ] || die "running firmware is not slot 1"
grep -q 'ubi.mtd=7' /proc/cmdline || die "kernel was not booted from mtd7 (firmware1)"
grep -q '"firmware2"' /proc/mtd || die "no firmware2 partition"
[ "$(grep '"firmware2"' /proc/mtd | cut -d: -f1)" = mtd8 ] || die "firmware2 is not mtd8"
[ -s "$ROOTFS" ] || die "rootfs image missing"

# Kernel FIT of the running slot (Braiins 4.4, has UIO/i2c-dev/UBI/squashfs).
KSIZE=$(cat /sys/class/ubi/ubi0_0/data_bytes)
dd if=/dev/ubi0_0 of=/tmp/omo-kernel.fit bs=$KSIZE count=1 2>/dev/null
[ "$(head -c 4 /tmp/omo-kernel.fit | hexdump -e '4/1 "%02x"')" = d00dfeed ] || die "kernel volume is not a FIT image"

/etc/init.d/bosminer stop >/dev/null 2>&1 || true
ubidetach -p /dev/mtd8 >/dev/null 2>&1 || true
# Upgrade in place when the slot already holds OpenMinerOS volumes, so the
# settings in rootfs_data (pools, OC, best share) survive the update.
KEEP=0
if ubiattach -m 8 -d 1 >/dev/null 2>&1; then
	n0=$(cat /sys/class/ubi/ubi1_0/name 2>/dev/null || true); n1=$(cat /sys/class/ubi/ubi1_1/name 2>/dev/null || true); n2=$(cat /sys/class/ubi/ubi1_2/name 2>/dev/null || true)
	if [ "$n0 $n1 $n2" = "kernel rootfs rootfs_data" ]; then KEEP=1; else ubidetach -d 1; fi
fi
if [ $KEEP = 1 ]; then
	echo "upgrading slot 2 in place (settings kept)"
else
	ubiformat /dev/mtd8 -y -q
	ubiattach -m 8 -d 1 >/dev/null
	ubimkvol /dev/ubi1 -n 0 -N kernel -s 4MiB >/dev/null
	ubimkvol /dev/ubi1 -n 1 -N rootfs -s 16MiB >/dev/null
	ubimkvol /dev/ubi1 -n 2 -N rootfs_data -s 8MiB >/dev/null
fi
ubiupdatevol /dev/ubi1_0 /tmp/omo-kernel.fit
ubiupdatevol /dev/ubi1_1 "$ROOTFS"
# Verify what was written.
head -c "$(wc -c < "$ROOTFS")" /dev/ubi1_1 | cmp -s - "$ROOTFS" || die "rootfs verify failed"
head -c "$KSIZE" /dev/ubi1_0 | cmp -s - /tmp/omo-kernel.fit || die "kernel verify failed"
echo "verified: kernel $KSIZE bytes, rootfs $(wc -c < "$ROOTFS") bytes"
ubidetach -d 1

fw_setenv firmware 2
fw_setenv upgrade_stage 0
# Braiins' auto_recovery deletes first_boot with `setenv first_boot && saveenv && reset`.
# In this U-Boot, deleting a variable that does not exist fails, which breaks the
# chain and leaves the board at the U-Boot prompt (seen on .47, 2026-11-22).
fw_setenv first_boot yes
echo "slot 2 written; U-Boot: firmware=$(fw_printenv -n firmware) upgrade_stage=$(fw_printenv -n upgrade_stage) first_boot=$(fw_printenv -n first_boot)"
echo "next boot: OpenMinerOS (trial). Any later reboot returns to Braiins OS."
if [ "$REBOOT" = --reboot ]; then
	sync
	reboot
fi
