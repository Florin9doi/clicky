#!/bin/bash

dd if=/dev/zero of=ipodhd.img bs=1M count=0 seek=$((90 * 1024 * 1024)) oflag=seek_bytes status=progress

python3 - ipodhd.img <<'PY'
import struct
import sys

f = open(sys.argv[1], "r+b")

# Driver Descriptor Map
d = bytearray(512)
d[0:2] = b'ER'
d[2:4] = struct.pack(">H", 512)
d[4:8] = struct.pack(">I", 90 * 1024 * 1024 // 512)
f.seek(0)
f.write(d)

def entry(start, size, name, typ):
    p = bytearray(512)
    p[0:2] = b'PM'
    p[4:8] = struct.pack(">I", 3)
    p[8:12] = struct.pack(">I", start)
    p[12:16] = struct.pack(">I", size)
    p[16:48] = name.encode().ljust(32, b'\0')
    p[48:80] = typ.encode().ljust(32, b'\0')
    p[88:92] = struct.pack(">I", 0x33)
    return p

f.seek(1 * 512)
f.write(entry(1, 3, "Apple", "Apple_partition_map"))

f.seek(2 * 512)
f.write(entry(4, 61440, "Firmware", "Apple_MDFW"))

f.seek(3 * 512)
f.write(entry(61444, 118784, "iPod", "DOS_FAT_32"))

f.close()
PY

if [ -n "$1" ]; then
    dd if="$1" of=ipodhd.img bs=1M seek=$((4 * 512)) oflag=seek_bytes conv=notrunc status=progress
fi

dd if=/dev/zero of=ipodhd_fat32.img bs=1M count=0 seek=$((118784 * 512)) oflag=seek_bytes status=progress
mkdosfs -F 32 --invariant ipodhd_fat32.img

dd if=ipodhd_fat32.img of=ipodhd.img bs=1M seek=$((61444 * 512)) oflag=seek_bytes conv=notrunc status=progress

rm ipodhd_fat32.img
