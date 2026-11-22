#!/usr/bin/env sh
# Builds the OpenMinerOS ramdisk image for a board. Runs inside the builder
# container, see image/builder.Dockerfile and `make ramdisk`.
set -eu

board="${1:?board is required}"
model="${2:?model is required}"
version="${3:?version is required}"
dist_dir="${4:-dist}"

case "$board" in
  s19-xil)
    board_family="xilinx"
    install_type="nand"
    rust_target="armv7-unknown-linux-musleabihf"
    ;;
  *)
    echo "ramdisk images are only implemented for s19-xil, got: $board" >&2
    exit 2
    ;;
esac

case "$model" in
  s19|s19-pro|s19j|s19j-pro|s19-xp|t19) ;;
  *)
    echo "unsupported S19-class model: $model" >&2
    exit 2
    ;;
esac

src_dir="$(cd "$(dirname "$0")/.." && pwd)"
source_commit="${SOURCE_COMMIT:-unknown}"
SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-0}"
export SOURCE_DATE_EPOCH
build_time="$(date -u -d "@$SOURCE_DATE_EPOCH" +%Y-%m-%dT%H:%M:%SZ)"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
rootfs="$stage/rootfs"

echo "==> building control plane for $rust_target"
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER=rust-lld \
RUSTFLAGS="--remap-path-prefix=$src_dir=/src --remap-path-prefix=${CARGO_HOME:-/usr/local/cargo}=/cargo" \
  cargo build --manifest-path "$src_dir/Cargo.toml" --locked --release \
    --target "$rust_target" -p openmineros-control-plane -p openmineros-board-xil
control_plane="${CARGO_TARGET_DIR:-$src_dir/target}/$rust_target/release/openmineros-control-plane"

echo "==> assembling root filesystem"
mkdir -p "$rootfs"
cp -R "$src_dir/image/rootfs/." "$rootfs/"
mkdir -p "$rootfs/bin" "$rootfs/sbin" "$rootfs/usr/bin" "$rootfs/usr/sbin" \
  "$rootfs/dev" "$rootfs/proc" "$rootfs/sys" "$rootfs/tmp" "$rootfs/run" \
  "$rootfs/root" "$rootfs/etc/openmineros"

busybox="$(dpkg -L busybox-static:armhf | grep -E '/bin/busybox$' | head -n 1)"
install -m 0755 "$busybox" "$rootfs/bin/busybox"
for applet in $(qemu-arm-static "$busybox" --list-full); do
  mkdir -p "$rootfs/$(dirname "$applet")"
  [ -e "$rootfs/$applet" ] || ln -s /bin/busybox "$rootfs/$applet"
done

install -m 0755 "$control_plane" "$rootfs/usr/bin/openmineros-control-plane"
install -m 0755 "$(dirname "$control_plane")/omo-fpga-probe" "$rootfs/usr/bin/omo-fpga-probe"
install -m 0755 "$(dirname "$control_plane")/omo-fpga-probe" "$stage/omo-fpga-probe"
sed -i "s/@MODEL@/$model/" "$rootfs/etc/inittab"
ln -s /tmp/resolv.conf "$rootfs/etc/resolv.conf"
chmod 0600 "$rootfs/etc/shadow"

cat > "$rootfs/etc/openmineros/fw-info.json" <<EOF_FW
{
  "name": "OpenMinerOS",
  "version": "$version",
  "board": "$board_family",
  "install_type": "$install_type",
  "build_time": "$build_time",
  "source_commit": "$source_commit"
}
EOF_FW

mknod -m 0600 "$rootfs/dev/console" c 5 1
mknod -m 0666 "$rootfs/dev/null" c 1 3

echo "==> packing ramdisk"
find "$rootfs" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
(
  cd "$rootfs"
  find . -mindepth 1 | LC_ALL=C sort \
    | cpio --quiet --reproducible -o -H newc -R 0:0
) | gzip -n -9 > "$stage/ramdisk.cpio.gz"

cat > "$stage/image.its" <<EOF_ITS
/dts-v1/;
/ {
	description = "OpenMinerOS $version $board";
	#address-cells = <1>;
	images {
		ramdisk {
			description = "OpenMinerOS root filesystem";
			data = /incbin/("ramdisk.cpio.gz");
			type = "ramdisk";
			arch = "arm";
			os = "linux";
			compression = "gzip";
			hash-1 {
				algo = "sha256";
			};
		};
	};
	configurations {
		default = "conf";
		conf {
			ramdisk = "ramdisk";
		};
	};
};
EOF_ITS

name="openmineros-${board}-${model}-${version}"
busybox_version="$(dpkg-query -W -f='${Version}' busybox-static:armhf)"
mkdir -p "$dist_dir"
(cd "$stage" && mkimage -f image.its "$name-ramdisk.itb" >/dev/null)
cp "$stage/$name-ramdisk.itb" "$stage/ramdisk.cpio.gz" "$dist_dir/"
cp "$stage/omo-fpga-probe" "$dist_dir/omo-fpga-probe-armv7"
mv "$dist_dir/ramdisk.cpio.gz" "$dist_dir/$name-ramdisk.cpio.gz"

cat > "$dist_dir/$name-ramdisk-manifest.json" <<EOF_MANIFEST
{
  "name": "$name",
  "version": "$version",
  "board": "$board",
  "model": "$model",
  "kind": "ramdisk",
  "source_commit": "$source_commit",
  "source_date_epoch": $SOURCE_DATE_EPOCH,
  "busybox": "$busybox_version",
  "rustc": "$(rustc --version)",
  "hashing": false,
  "warning": "Boots and serves the local API only. It does not drive hashboards, fans or the PSU and has not been booted on real hardware."
}
EOF_MANIFEST

(
  cd "$dist_dir"
  sha256sum "$name-ramdisk.itb" "$name-ramdisk.cpio.gz" "$name-ramdisk-manifest.json" \
    omo-fpga-probe-armv7 \
    > "$name-ramdisk-sha256sum.txt"
  cat "$name-ramdisk-sha256sum.txt"
)
