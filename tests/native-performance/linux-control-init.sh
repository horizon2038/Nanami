#!/bin/busybox sh
# Test-only PID 1. The benchmark and all of its libraries come from the same
# read-only AHCI/ext2 image used by Alter, not this bootstrap initramfs.
export PATH=/bin:/sbin:/usr/bin:/usr/sbin
exec </dev/console >/dev/console 2>&1

fail() {
    echo "LINUX-CONTROL-FAIL stage=$1"
    /bin/busybox poweroff -f
    while :; do /bin/busybox sleep 1; done
}

mount -t proc proc /proc || fail proc
mount -t sysfs sysfs /sys || fail sysfs
# Tiny Core does not require CONFIG_DEVTMPFS. Make only the bootstrap nodes
# inside the VM, using the detected sysfs device number for its one partition.
mount -t tmpfs tmpfs /dev || fail dev
mknod /dev/console c 5 1 || fail console
mknod /dev/null c 1 3 || fail null
modprobe ahci || fail ahci
modprobe ext4 || fail filesystem
attempt=0
while [ ! -e /sys/class/block/sda2/dev ]; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 200 ]; then
        fail disk
    fi
    /bin/busybox sleep 0.05
done
IFS=: read -r major minor </sys/class/block/sda2/dev
mknod /dev/sda2 b "$major" "$minor" || fail partition
mkdir -p /mnt/nanami
# The official kernel provides ext2 compatibility through its ext4 module.
mount -t ext4 -o ro,noload /dev/sda2 /mnt/nanami || fail mount
root=/mnt/nanami/alter/linux/tinycore/rootfs
echo 'LINUX-CONTROL-ENV-BEGIN'
uname -a
cat /sys/devices/system/clocksource/clocksource0/current_clocksource
cat /proc/mounts
chroot "$root" /bin/busybox ls -l /bin/nanami-process-performance /bin/busybox \
    /lib/ld-linux-x86-64.so.2 /lib/libc.so.6 /lib/libm.so.6 /usr/lib/libcrypt.so.1 || fail identity
echo 'LINUX-CONTROL-ENV-END'
# The Alter profile has already run BusyBox repeatedly before its measurement.
# Warm the same executable/libraries and allow Linux's boot workers to settle.
chroot "$root" /bin/busybox true || fail warmup
sleep 2
echo 'LINUX-CONTROL-BEGIN'
chroot "$root" /bin/nanami-process-performance || fail benchmark
echo 'LINUX-CONTROL-PASS'
poweroff -f
while :; do sleep 1; done
