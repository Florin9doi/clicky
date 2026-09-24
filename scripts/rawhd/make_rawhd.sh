# makes a 90Mb drive, and optionally copies a firmware binary into the firmware
# partition (if the firmware file is specified as the first arg)

dd if=/dev/zero of=ipodhd.img bs=1M count=0 seek=$((90 * 1024 * 1024)) oflag=seek_bytes status=progress
sfdisk ipodhd.img << EOM
label: dos
label-id: 0x04206969
device: ipodhd.img
unit: sectors

ipodhd.img1 : start=          63, size=       61440, type=0, bootable
ipodhd.img2 : start=       61503, size=      118784, type=b
EOM

if [ -n "$1" ]; then
    dd if=$1 of=ipodhd.img bs=1M seek=$((63 * 512)) oflag=seek_bytes conv=notrunc status=progress
fi

dd if=/dev/zero of=ipodhd_fat32.img bs=1M count=0 seek=$((118784 * 512)) oflag=seek_bytes status=progress
mkdosfs -F 32 --invariant ipodhd_fat32.img
dd if=ipodhd_fat32.img of=ipodhd.img bs=1M seek=$((61503 * 512)) oflag=seek_bytes conv=notrunc status=progress

# cleanup
rm ipodhd_fat32.img
