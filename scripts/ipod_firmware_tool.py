#!/usr/bin/env python3
"""
ipod_firmware_tool.py

A pure-Python, cross-platform, dependency-free tool for reading Apple
click-wheel iPod "Firmware" files: it parses the firmware directory,
lists every image (OSOS, RSRC, AUPD, HIBE, OSBK), and extracts them -
transparently decrypting AUPD (which is RC4-scrambled) along the way.

No external libraries, no compiled helpers - just the standard library.
Works anywhere Python 3 runs.

Format reference (reverse-engineered from the A1099 / iPod 4th-gen
bootloader, and cross-checked against Rockbox's ipodpatcher source:
https://github.com/Rockbox/rockbox/tree/master/utils/ipodpatcher):

  Firmware partition header (at the very start of the firmware
  partition / "Firmware" file):
    +0x100   4 bytes   magic, on-disk as b"]ih[" (logical "[hi]")
    +0x104   4 bytes   directory pointer (add 0x200 to get diroffset)
    +0x10a   2 bytes   directory format version

  Directory (at diroffset), a sequence of entries, each:
    +0x00    4 bytes   marker, b"!ATA" or b"DNAN"
    +0x04    4 bytes   tag (4 ASCII chars stored reversed on disk, e.g.
                       the logical tag "aupd" is stored as b"dpua")
    +0x08    4 bytes   id
    +0x0c    4 bytes   devOffset  (image offset, relative to fwoffset)
    +0x10    4 bytes   len        (image length in bytes)
    +0x14    4 bytes   addr
    +0x18    4 bytes   entryOffset
    +0x1c    4 bytes   chksum     (simple additive checksum of the
                                   *decrypted* image bytes)
    +0x20    4 bytes   vers
    +0x24    4 bytes   loadAddr
  (36 bytes of fields, preceded by the 4-byte marker = 40 bytes/entry)

  fwoffset = partition_start                      if nimages > 1 and
                                                      version == 2
           = partition_start + sector_size (512)  otherwise

  Each image's raw bytes live at file offset: fwoffset + devOffset

  AUPD is additionally RC4-encrypted, but ONLY on directory version 3.
  On version 2, AUPD is stored as plain, unobfuscated bytes - identical
  in principle to OSOS/RSRC/etc - and needs no key or decryption at all.
  When encrypted (version 3), the 4-byte RC4 key is derived from the
  512-byte "security block" sector immediately preceding the AUPD image
  (at fwoffset + devOffset - 512), using a key-schedule algorithm
  originally documented by BadBlocks/Kingstone at
  http://ipodlinux.org/Flash_Decryption and implemented in Rockbox's
  ipodpatcher_aupd.c (GetSecurityBlockKey/testMarker), reimplemented
  here in pure Python (see AupdKeyDeriver below).

Usage (flags can be combined freely; steps always run in this order:
list, extract-*, build-flash, build-hdd). Output goes to the current
directory and existing files are never overwritten without --force:

    # List the images in a Firmware file
    python3 ipod_firmware_tool.py -i Firmware-5.4.2.1 -l

    # Rebuild the NOR flash image (the model selects the gfCS/SCfg
    # device-info struct that gets inserted)
    python3 ipod_firmware_tool.py -i Firmware-5.4.2.1 -m 4g -f 4g_flash.bin

    # Build a 90 MiB HDD image (pure Python): MBR needs v2/v3 firmware,
    # APM works for v0/v2/v3; the AUPD id inside the image is patched to 1
    python3 ipod_firmware_tool.py -i Firmware-5.4.2.1 -d mbr 4g_hdd.bin

    # Less frequent: --extract-all, --extract-raw IMAGE FILE,
    # --extract-decoded IMAGE FILE, --extract-flash-parts, --hdd-size MiB

As a library:
    from ipod_firmware_tool import FirmwareImage
    fw = FirmwareImage.load("Firmware-5.4.2.1")
    for img in fw.images:
        print(img.name, img.length, img.checksum_ok)
        data = img.decoded_data()   # transparently RC4-decrypts AUPD
"""

from __future__ import annotations

import argparse
import os
import struct
import sys
from dataclasses import dataclass, field
from typing import Optional


# --------------------------------------------------------------------------
# Constants describing the on-disk format
# --------------------------------------------------------------------------

SECTOR_SIZE = 512
DIR_ENTRY_MARKER_SIZE = 4
DIR_ENTRY_FIELDS_SIZE = 36  # everything after the marker
DIR_ENTRY_SIZE = DIR_ENTRY_MARKER_SIZE + DIR_ENTRY_FIELDS_SIZE

HEADER_MAGIC = b"]ih["          # on-disk bytes; logical value is "[hi]"
HEADER_MAGIC_OFFSET = 0x100
DIR_POINTER_OFFSET = 0x104
DIR_POINTER_ADDEND = 0x200
DIR_VERSION_OFFSET = 0x10a

VALID_ENTRY_MARKERS = (b"!ATA", b"DNAN")

# Logical tag -> on-disk bytes is the ASCII reversed, e.g. "aupd" -> b"dpua"
TAG_NAMES = {
    b"soso": "OSOS",
    b"crsr": "RSRC",
    b"dpua": "AUPD",
    b"ebih": "HIBE",
    b"kbso": "OSBK",
}

AUPD_RC4_CONSTANT = 0x54c3a298
AUPD_KEY_OFFSETS = (0x5, 0x25, 0x6f, 0x69, 0x15, 0x4d, 0x40, 0x34)

# "FwUp"/"flsh" flash-part header, as found inside a decoded AUPD image.
# Stored reversed on disk (same FourCC convention as the directory tags):
#   "FwUp" -> b"pUwF", "flsh" -> b"hslf"
FWUP_MAGIC = b"pUwF"
FLSH_MAGIC = b"hslf"
FWUP_HEADER_LENS = (0x18, 0x1c)  # the two observed header sizes
FLASH_PAD_PATTERN = b"\xFF\xFF"  # filler for uncovered regions in a combined image

GFCS_REGION_SIZE = 0x1000  # size of the region a gfCS struct is checked/inserted into


def _build_gfcs_struct(elements: list[tuple[str, str]]) -> bytes:
    """Builds a "gfCS"/"SCfg" device-info struct from a list of
    (name_hex, data_hex) element tuples (space-separated hex strings,
    matching how this data is naturally transcribed from a hex dump),
    computing the header/length fields automatically.

    Layout (all multi-byte fields little-endian):
        @0x00  magic "gfCS" (4 bytes)
        @0x04  struct_len (4 bytes)
        @0x08  fixed bytes (12 bytes): 00 20 00 00 01 00 01 00 00 00 00 00
        @0x14  element_count (4 bytes)
        @0x18  element_count * 20-byte elements, each:
                 +0x00  name (4 bytes)
                 +0x04  data (16 bytes)
    """
    fixed = bytes.fromhex("002000000100010000000000")  # 12 bytes
    assert len(fixed) == 12

    body = bytearray()
    for name_hex, data_hex in elements:
        name = bytes.fromhex(name_hex.replace(" ", ""))
        data = bytes.fromhex(data_hex.replace(" ", ""))
        assert len(name) == 4, f"element name must be 4 bytes, got {name!r}"
        assert len(data) == 16, f"element data must be 16 bytes, got {len(data)} for {name!r}"
        body += name
        body += data

    header_len = 0x18
    struct_len = header_len + len(body)

    out = bytearray()
    out += b"gfCS"
    out += struct.pack("<I", struct_len)
    out += fixed
    out += struct.pack("<I", len(elements))
    out += body
    assert len(out) == struct_len
    return bytes(out)


# --------------------------------------------------------------------------
# "gfCS"/"SCfg" device-info structs, hardcoded per iPod model,
# captured verbatim from real devices. Each element's comment gives its
# logical (un-reversed) 4-char tag, e.g. "mNrS" on disk is logical "SrNm".
# --------------------------------------------------------------------------

GFCS_STRUCTS: dict[str, bytes] = {
    # Replace with proper dumps when/if aval    # !!! DO NOT RE-ORDER THE ELEMENTS !!! #
    "1g": _build_gfcs_struct([ # copy-paste from 3g + unk elem removed + HwVr fixed
        ("6D 4E 72 53", "32 58 35 31 36 30 32 52 50 51 35 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "72 15 4C 00 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "0B 35 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "00 00 00 00 01 A1 FE FF 00 00 00 00 00 00 00 00"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 00 00 01 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        # Add DrmV? It is present on the latest version.
    ]),
    "2g": _build_gfcs_struct([ # copy-paste from 3g + unk elem removed + HwVr fixed
        ("6D 4E 72 53", "32 58 35 31 36 30 32 52 50 51 35 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "72 15 4C 00 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "7A 36 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "00 00 00 00 01 A1 FE FF 00 00 00 00 00 00 00 00"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 00 00 02 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        # Add DrmV? It is present on the latest version.
    ]),
    "3g": _build_gfcs_struct([
        ("6D 4E 72 53", "32 58 35 31 36 30 32 52 50 51 35 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "72 15 4C 00 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "0A 43 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "00 00 00 00 01 A1 FE FF 00 00 00 00 00 00 00 00"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 01 00 03 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 00 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "50 39 32 34 34 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
        ("31 4F 77 48", "0A 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00"),  # 1OwH/HwO1
        ("74 6E 6F 43", "00 00 04 00 05 00 00 00 00 00 00 00 00 00 00 00"),  # tnoC/Cont
        # Add DrmV? It is present on the latest version. Is this an older dump?
    ]),
    "4g_mono": _build_gfcs_struct([ # copy-paste from 4g color + HwVr fixed
        ("6D 4E 72 53", "4A 51 35 33 37 41 30 4E 54 44 53 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "00 00 00 01 36 4F 79 14 00 27 0A 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "5A 53 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 14 00 05 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 00 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "4D 39 32 38 32 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
        ("74 6E 6F 43", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # tnoC/Cont
        ("56 6D 72 44", "00 00 00 00 06 00 00 00 00 00 00 00 00 00 00 00"),  # VmrD/DrmV
    ]),
    "4g_photo": _build_gfcs_struct([ # copy-paste from 4g color + HwVr fixed
        ("6D 4E 72 53", "4A 51 35 33 37 41 30 4E 54 44 53 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "00 00 00 01 36 4F 79 14 00 27 0A 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "2A 64 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 00 00 06 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 00 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "4D 39 35 38 35 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
        ("74 6E 6F 43", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # tnoC/Cont
        ("56 6D 72 44", "00 00 00 00 06 00 00 00 00 00 00 00 00 00 00 00"),  # VmrD/DrmV
    ]),
    "4g_color": _build_gfcs_struct([
        ("6D 4E 72 53", "4A 51 35 33 37 41 30 4E 54 44 53 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "00 00 00 01 36 4F 79 14 00 27 0A 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "4A 76 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 04 00 06 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 00 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "4D 41 30 37 39 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
        ("74 6E 6F 43", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # tnoC/Cont
        ("56 6D 72 44", "00 00 00 00 06 00 00 00 00 00 00 00 00 00 00 00"),  # VmrD/DrmV
    ]),
    "5g": _build_gfcs_struct([
        ("6D 4E 72 53", "34 4A 36 30 38 32 59 37 54 58 4B 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "00 00 00 01 26 E7 EF 14 00 27 0A 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "3A 76 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("72 56 77 48", "00 00 00 00 05 00 0B 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 02 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "4D 41 31 34 36 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
        ("56 6D 72 44", "00 00 00 00 06 00 00 00 00 00 00 00 00 00 00 00"),  # VmrD/DrmV
    ]),
    "mini1g": _build_gfcs_struct([ # copy-paste from 4g color + HwVr fixed
        ("6D 4E 72 53", "4A 51 35 33 37 41 30 4E 54 44 53 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "00 00 00 01 36 4F 79 14 00 27 0A 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "6A 62 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 13 00 04 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 00 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "4D 39 31 36 30 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
        ("74 6E 6F 43", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # tnoC/Cont
        ("56 6D 72 44", "00 00 00 00 06 00 00 00 00 00 00 00 00 00 00 00"),  # VmrD/DrmV
    ]),
    "mini2g": _build_gfcs_struct([ # copy-paste from 4g color + HwVr fixed
        ("6D 4E 72 53", "4A 51 35 33 37 41 30 4E 54 44 53 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "00 00 00 01 36 4F 79 14 00 27 0A 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "4A 80 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 02 00 07 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 00 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "4D 39 38 30 30 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
        ("74 6E 6F 43", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # tnoC/Cont
        ("56 6D 72 44", "00 00 00 00 06 00 00 00 00 00 00 00 00 00 00 00"),  # VmrD/DrmV
    ]),
    "nano1g": _build_gfcs_struct([ # copy-paste from 5g + HwVr fixed
        ("6D 4E 72 53", "34 4A 36 30 38 32 59 37 54 58 4B 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "00 00 00 01 26 E7 EF 14 00 27 0A 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "6A 85 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("72 56 77 48", "00 00 00 00 06 00 0c 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 02 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "4D 41 30 30 34 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
        ("56 6D 72 44", "00 00 00 00 06 00 00 00 00 00 00 00 00 00 00 00"),  # VmrD/DrmV
    ]),
}

# --------------------------------------------------------------------------
# Model -> (gfCS insertion offset, gfCS struct to use) lookup.
# `gfcs` is None where we don't yet have a captured struct for that exact
# model - insertion is simply skipped (with a note) for those
# until real data is available to add here.
# --------------------------------------------------------------------------

@dataclass
class ModelInfo:
    offset: int
    gfcs: Optional[str]  # key into GFCS_STRUCTS, or None if not yet known


IPOD_MODELS: dict[str, ModelInfo] = {
    "1g":       ModelInfo(0x2000, "1g"),
    "2g":       ModelInfo(0x2000, "2g"),
    "3g":       ModelInfo(0x2000, "3g"),
    "4g_mono":  ModelInfo(0x2000, "4g_mono"),
    "4g_photo": ModelInfo(0x2000, "4g_photo"),
    "4g_color": ModelInfo(0x2000, "4g_color"),
    "5g":       ModelInfo(0x4000, "5g"),
    "mini1g":   ModelInfo(0x2000, "mini1g"),
    "mini2g":   ModelInfo(0x2000, "mini2g"),
    "nano1g":   ModelInfo(0x4000, "nano1g"),
}

# A few convenience aliases so common ways of typing a model still work.
_MODEL_ALIASES = {
    "1": "1g", "gen1": "1g",
    "2": "2g", "gen2": "2g",
    "3": "3g", "gen3": "3g",
    "4": "4g_mono", "gen4": "4g_mono", "4g": "4g_mono", "4gmono": "4g_mono", "4g_gray": "4g_mono", "4g_grayscale": "4g_mono",
    "4gphoto": "4g_photo", "photo": "4g_photo",
    "4gcolor": "4g_color", "color": "4g_color",
    "5": "5g", "gen5": "5g", "video": "5g",
    "mini": "mini1g",
    "mini1": "mini1g", "mini_1g": "mini1g",
    "mini2": "mini2g", "mini_2g": "mini2g",
    "nano": "nano1g", "nano1": "nano1g", "nano_1g": "nano1g",
}


def resolve_model(name: str) -> ModelInfo:
    """Looks up a model string, case/whitespace/punctuation
    insensitively, with a helpful error listing valid names if it
    doesn't match anything known."""
    key = name.strip().lower().replace(" ", "_").replace("-", "_")
    key = _MODEL_ALIASES.get(key, key)
    if key not in IPOD_MODELS:
        valid = ", ".join(sorted(set(IPOD_MODELS) | set(_MODEL_ALIASES)))
        raise ValueError(f"unknown iPod model {name!r}. Valid options: {valid}")
    return IPOD_MODELS[key]


# --------------------------------------------------------------------------
# Small helpers
# --------------------------------------------------------------------------

def _to_i32(x: int) -> int:
    """Wrap an integer to signed 32-bit, matching C 'int' semantics."""
    x &= 0xFFFFFFFF
    return x - 0x1_0000_0000 if x & 0x8000_0000 else x


def _to_u32(x: int) -> int:
    return x & 0xFFFFFFFF


def additive_checksum(data: bytes) -> int:
    """The simple checksum scheme used for every image: sum all bytes,
    keep a 32-bit running total (no seed)."""
    total = 0
    for b in data:
        total = (total + b) & 0xFFFFFFFF
    return total


def rc4(key: bytes, data: bytes) -> bytes:
    """Standard RC4 stream cipher (key-scheduling + pseudo-random
    generation), used as-is (no relation to modern secure ciphers - this
    is only here because it's what Apple happened to use)."""
    s = list(range(256))
    j = 0
    key_len = len(key)
    for i in range(256):
        j = (j + s[i] + key[i % key_len]) & 0xFF
        s[i], s[j] = s[j], s[i]

    out = bytearray(len(data))
    i = j = 0
    for n, byte in enumerate(data):
        i = (i + 1) & 0xFF
        j = (j + s[i]) & 0xFF
        s[i], s[j] = s[j], s[i]
        out[n] = byte ^ s[(s[i] + s[j]) & 0xFF]
    return bytes(out)


# --------------------------------------------------------------------------
# AUPD key derivation
# --------------------------------------------------------------------------

class AupdKeyDeriver:
    """Derives the 4-byte RC4 key for an AUPD image from its preceding
    512-byte security block.

    This is a line-for-line port of GetSecurityBlockKey/testMarker from
    Rockbox's ipodpatcher_aupd.c (itself crediting BadBlocks/Kingstone,
    http://ipodlinux.org/Flash_Decryption), translated to Python's
    arbitrary-precision integers with explicit 32-bit wrapping at each
    step to reproduce C's 'int' overflow behaviour exactly.
    """

    @staticmethod
    def _test_marker(marker: int) -> bool:
        marker = _to_i32(marker)
        b = marker & 0xFF
        mask = _to_i32(b | (b << 8) | (b << 16) | (b << 24))
        decrypt = _to_i32(marker ^ mask)

        temp1 = _to_i32(_to_u32(decrypt) >> 24)
        temp2 = _to_i32(decrypt << 8)
        if temp1 == 0:
            return False

        temp2 = _to_i32(_to_u32(temp2) >> 24)
        decrypt = _to_i32(decrypt << 16)
        decrypt = _to_i32(_to_u32(decrypt) >> 24)

        if temp1 < temp2 < decrypt:
            temp1 &= 0xF
            temp2 &= 0xF
            decrypt &= 0xF
            if temp1 > temp2 > decrypt != 0:
                return True
        return False

    @classmethod
    def derive_keys(cls, security_block: bytes) -> list[bytes]:
        """Returns every 4-byte key candidate found (normally exactly
        one for a valid security block)."""
        if len(security_block) < 512:
            raise ValueError("security block must be at least 512 bytes")

        def u32le(pos: int) -> int:
            return struct.unpack_from("<I", security_block, pos)[0]

        keys = []
        for c, off in enumerate(AUPD_KEY_OFFSETS):
            marker = u32le(off * 4)
            if not cls._test_marker(marker):
                continue

            next_off = AUPD_KEY_OFFSETS[c + 1] if c < 7 else AUPD_KEY_OFFSETS[0]
            pos = (next_off * 4) + 4

            key = 0
            for _ in range(2):
                word = _to_i32(u32le(pos))
                key = _to_i32(_to_i32(marker) ^ word ^ _to_i32(AUPD_RC4_CONSTANT))
                pos += 4

            r1 = 0x6F
            count = 2
            while count < 128:
                r2 = _to_i32(u32le(count * 4))
                r12 = _to_i32(u32le((count * 4) + 4))
                r14 = _to_i32(r2 | (_to_u32(r12) >> 16))
                r2 = _to_i32(r2 & 0xFFFF)
                r2 = _to_i32(r2 | r12)
                r1 = _to_i32(r1 ^ r14)
                r1 = _to_i32(r1 + r2)
                count += 2

            key = _to_i32(key ^ r1)
            key_u32 = _to_u32(key)
            keys.append(bytes([
                key_u32 & 0xFF,
                (key_u32 >> 8) & 0xFF,
                (key_u32 >> 16) & 0xFF,
                (key_u32 >> 24) & 0xFF,
            ]))
        return keys


# --------------------------------------------------------------------------
# Firmware directory parsing
# --------------------------------------------------------------------------

@dataclass
class FirmwareImageEntry:
    name: str
    tag_raw: bytes
    dev_offset: int
    length: int
    checksum: int
    id_: int = 0
    addr: int = 0
    entry_offset: int = 0
    vers: int = 0
    load_addr: int = 0

    # populated once the parent FirmwareImage is known:
    _fw: Optional["FirmwareImage"] = field(default=None, repr=False, compare=False)

    def raw_data(self) -> bytes:
        """Bytes exactly as stored on disk (still RC4-encrypted for AUPD)."""
        start = self._fw.fwoffset + self.dev_offset
        return self._fw.buf[start:start + self.length]

    def decoded_data(self) -> bytes:
        """Bytes after any necessary decryption.

        AUPD is RC4-obfuscated on firmware directory version 3, but
        stored as plain, unobfuscated bytes on version 2 - identical to
        every other image type. Everything else is returned as-is
        regardless of version.
        """
        data = self.raw_data()
        if self.name == "AUPD" and self._fw.version == 3:
            key = self._fw.get_aupd_key(self)
            data = rc4(key, data)
        return data

    def checksum_ok(self) -> bool:
        return additive_checksum(self.decoded_data()) == self.checksum


@dataclass
class FirmwareImage:
    """Represents a parsed iPod Firmware file. The firmware header must
    sit at the very start of the file (header magic at +0x100); anything
    else is rejected as invalid."""
    buf: bytes
    fwoffset: int
    version: int
    images: list[FirmwareImageEntry]

    @classmethod
    def load(cls, path: str) -> "FirmwareImage":
        with open(path, "rb") as f:
            buf = f.read()
        return cls.from_bytes(buf)

    @classmethod
    def from_bytes(cls, buf: bytes) -> "FirmwareImage":
        if len(buf) < 0x110:
            raise ValueError("file too small to contain a firmware header")

        magic = buf[HEADER_MAGIC_OFFSET: HEADER_MAGIC_OFFSET + 4]
        if magic != HEADER_MAGIC:
            raise ValueError(
                f"not a valid Firmware file: expected header magic {HEADER_MAGIC!r} "
                f"at offset 0x{HEADER_MAGIC_OFFSET:x}, got {magic!r}. Note a raw "
                "bootloader/NOR ROM dump normally does NOT contain this header; "
                "it lives in the firmware partition on disk instead."
            )

        dir_ptr = struct.unpack_from("<I", buf, DIR_POINTER_OFFSET)[0]
        diroffset = dir_ptr + DIR_POINTER_ADDEND
        version = struct.unpack_from("<H", buf, DIR_VERSION_OFFSET)[0]

        firmware_offset = 0
        if version == 0:
            diroffset = 0x4000
            firmware_offset = dir_ptr

        images: list[FirmwareImageEntry] = []
        pos = diroffset
        while pos + DIR_ENTRY_SIZE <= len(buf) and len(images) < 10:
            marker = buf[pos:pos + DIR_ENTRY_MARKER_SIZE]
            if marker not in VALID_ENTRY_MARKERS:
                break

            fields_off = pos + DIR_ENTRY_MARKER_SIZE
            tag_raw = buf[fields_off:fields_off + 4]
            if tag_raw not in TAG_NAMES:
                break

            id_        = struct.unpack_from("<I", buf, fields_off + 0x04)[0]
            dev_offset = (struct.unpack_from("<I", buf, fields_off + 0x08)[0]) - firmware_offset
            length     = struct.unpack_from("<I", buf, fields_off + 0x0c)[0]
            addr       = struct.unpack_from("<I", buf, fields_off + 0x10)[0]
            entry_off  = struct.unpack_from("<I", buf, fields_off + 0x14)[0]
            checksum   = struct.unpack_from("<I", buf, fields_off + 0x18)[0]
            vers       = struct.unpack_from("<I", buf, fields_off + 0x1c)[0]
            load_addr  = struct.unpack_from("<I", buf, fields_off + 0x20)[0]

            if length == 0 or length > len(buf) or dev_offset > len(buf):
                break

            images.append(FirmwareImageEntry(
                name=TAG_NAMES[tag_raw],
                tag_raw=tag_raw,
                dev_offset=dev_offset,
                length=length,
                checksum=checksum,
                id_=id_,
                addr=addr,
                entry_offset=entry_off,
                vers=vers,
                load_addr=load_addr,
            ))
            pos += DIR_ENTRY_SIZE

        if not images:
            raise ValueError("firmware header found, but no valid directory entries after it")

        if len(images) > 1 and version == 3:
            fwoffset = SECTOR_SIZE
        else:
            fwoffset = 0

        fw = cls(buf=buf, fwoffset=fwoffset, version=version, images=images)
        for img in images:
            img._fw = fw
        return fw

    def find(self, name: str) -> Optional[FirmwareImageEntry]:
        name = name.upper()
        return next((i for i in self.images if i.name == name), None)

    def get_aupd_key(self, aupd_entry: FirmwareImageEntry) -> bytes:
        """Derives (and caches) the RC4 key for an AUPD entry from its
        preceding 512-byte security block."""
        sec_start = self.fwoffset + aupd_entry.dev_offset - SECTOR_SIZE
        if sec_start < 0 or sec_start + SECTOR_SIZE > len(self.buf):
            raise ValueError("security block location is out of range for this file")
        security_block = self.buf[sec_start:sec_start + SECTOR_SIZE]

        keys = AupdKeyDeriver.derive_keys(security_block)
        if len(keys) != 1:
            raise ValueError(
                f"expected exactly 1 key candidate in the security block, "
                f"found {len(keys)} - can't reliably decrypt AUPD"
            )
        return keys[0]


# --------------------------------------------------------------------------
# "FwUp"/"flsh" flash-part parsing (inside a decoded AUPD image)
# --------------------------------------------------------------------------

@dataclass
class FwUpPart:
    """One flash-programming chunk found inside a decoded AUPD image:
    a small header ('FwUp' ... 'flsh') followed immediately by
    `payload_len` bytes to be written at `dest_offset` in the target
    flash chip.

        @0x00  FwUp magic (4 bytes)
        @0x04  header_len   (4 bytes LE) - 0x18 or 0x1c
        @0x08  flsh magic   (4 bytes)
        @0x0c  payload_len  (4 bytes LE)
        @0x10  dest_offset  (4 bytes LE)
        @0x14  reserved1    (4 bytes, seen as zero - ignored)
        @0x18  reserved2    (4 bytes, only present if header_len == 0x1c;
                             seen as zero - ignored)

    The payload itself starts right after the header, i.e. at
    `header_offset + header_len`, and is `payload_len` bytes long.
    """
    header_offset: int      # offset of this header within the AUPD image
    header_len: int
    payload_len: int
    dest_offset: int
    reserved1: int
    reserved2: int

    _aupd_data: bytes = field(default=b"", repr=False, compare=False)

    @property
    def payload_offset(self) -> int:
        return self.header_offset + self.header_len

    @property
    def last_addr(self) -> int:
        """Last byte address covered by this part (inclusive)."""
        return self.dest_offset + self.payload_len - 1

    def payload(self) -> bytes:
        start = self.payload_offset
        return self._aupd_data[start:start + self.payload_len]

    def suggested_filename(self, stem: str, addr_width: int = 5) -> str:
        return (f"{stem}_{self.dest_offset:0{addr_width}x}-"
                f"{self.last_addr:0{addr_width}x}.bin")


def find_fwup_parts(aupd_data: bytes) -> list[FwUpPart]:
    """Scans a decoded AUPD image for every 'FwUp'...'flsh' header and
    returns the parsed parts, in the order they appear."""
    parts: list[FwUpPart] = []
    pos = 0
    limit = len(aupd_data) - 12
    while pos <= limit:
        if aupd_data[pos:pos + 4] != FWUP_MAGIC:
            pos += 1
            continue

        header_len = struct.unpack_from("<I", aupd_data, pos + 4)[0]
        if header_len not in FWUP_HEADER_LENS:
            pos += 1
            continue

        if aupd_data[pos + 8:pos + 12] != FLSH_MAGIC:
            pos += 1
            continue

        if pos + header_len > len(aupd_data):
            pos += 1
            continue

        payload_len = struct.unpack_from("<I", aupd_data, pos + 0xc)[0]
        dest_offset = struct.unpack_from("<I", aupd_data, pos + 0x10)[0]
        reserved1   = struct.unpack_from("<I", aupd_data, pos + 0x14)[0]
        reserved2 = 0
        if header_len >= 0x1c:
            reserved2 = struct.unpack_from("<I", aupd_data, pos + 0x18)[0]

        parts.append(FwUpPart(
            header_offset=pos,
            header_len=header_len,
            payload_len=payload_len,
            dest_offset=dest_offset,
            reserved1=reserved1,
            reserved2=reserved2,
            _aupd_data=aupd_data,
        ))

        # Headers observed so far don't overlap; skip past this whole
        # part (header + payload) before continuing the scan so we don't
        # get spurious re-matches inside payload data.
        pos += header_len + payload_len

    return parts


# --------------------------------------------------------------------------
# Fallback for the earliest firmware files: no "FwUp"/"flsh" headers at all,
# but the decoded AUPD still contains raw NOR data at fixed offsets. A
# hardcoded table, keyed by the AUPD's directory checksum, tells us exactly
# where to cut it and where each chunk belongs in the flash.
# --------------------------------------------------------------------------

@dataclass
class FallbackChunk:
    aupd_offset: int   # where the chunk starts inside the decoded AUPD image
    length: int        # chunk length in bytes
    dest_offset: int   # address the chunk occupies in the rebuilt flash image


@dataclass
class FallbackEntry:
    version: str                 # human-readable firmware version
    checksum: int                # AUPD directory checksum (the lookup key)
    chunks: list[FallbackChunk]


FALLBACK_TABLE: list[FallbackEntry] = [
    FallbackEntry("1g v1.0.0", 0x0e30b4ca, [FallbackChunk(0x7EC4,  0x2000, 0x0000),
                                            FallbackChunk(0x9EC4, 0xFC000, 0x4000)]),
    FallbackEntry("1g v1.0.2", 0x0e2fc2f8, [FallbackChunk(0x7EFC,  0x2000, 0x0000),
                                            FallbackChunk(0x9EFC, 0xFC000, 0x4000)]),
    FallbackEntry("1g v1.0.4", 0x0e98f686, [FallbackChunk(0x7EFC,  0x2000, 0x0000),
                                            FallbackChunk(0x9EFC, 0xFC000, 0x4000)]),
    FallbackEntry("1g v1.1.0", 0x0e2be30d, [FallbackChunk(0x82D0,  0x2000, 0x0000),
                                            FallbackChunk(0xA2D0, 0xFC000, 0x4000)]),
]

FALLBACK_BY_CHECKSUM: dict[int, FallbackEntry] = {e.checksum: e for e in FALLBACK_TABLE}
assert len(FALLBACK_BY_CHECKSUM) == len(FALLBACK_TABLE), "duplicate checksum in FALLBACK_TABLE"


def fallback_parts(aupd_data: bytes, aupd_checksum: int) -> tuple[list[FwUpPart], str]:
    """Looks up `aupd_checksum` in FALLBACK_TABLE and returns the chunks as
    FwUpPart objects (zero-length header, so payload() slices straight out
    of the AUPD) plus a human-readable note. Returns ([], note) if the
    checksum is unknown or a chunk doesn't fit inside the AUPD."""
    entry = FALLBACK_BY_CHECKSUM.get(aupd_checksum)
    if entry is None:
        return [], (f"AUPD checksum 0x{aupd_checksum:08x} is not in the fallback "
                    f"table - add it to FALLBACK_TABLE if this is an early firmware")
    for c in entry.chunks:
        if c.aupd_offset + c.length > len(aupd_data):
            return [], (f"fallback entry {entry.version}: chunk @0x{c.aupd_offset:x} "
                        f"len=0x{c.length:x} exceeds AUPD size {len(aupd_data):,} - skipped")
    parts = [FwUpPart(header_offset=c.aupd_offset, header_len=0,
                      payload_len=c.length, dest_offset=c.dest_offset,
                      reserved1=0, reserved2=0, _aupd_data=aupd_data)
             for c in entry.chunks]
    return parts, f"no FwUp/flsh headers; using fallback table entry for firmware {entry.version}"


def get_flash_parts(fw: "FirmwareImage") -> tuple[list[FwUpPart], str]:
    """find_fwup_parts() on the decoded AUPD, falling back to the
    checksum-keyed table if that finds nothing. Returns (parts, note);
    note is empty when the normal path worked."""
    img = fw.find("AUPD")
    if img is None:
        return [], "This firmware has no AUPD image."
    try:
        data = img.decoded_data()
    except Exception as e:
        return [], f"Failed to decode AUPD: {e}"
    parts = find_fwup_parts(data)
    if parts:
        return parts, ""
    return fallback_parts(data, img.checksum)


def combine_fwup_parts(
    parts: list[FwUpPart],
    model: str,
    pad_pattern: bytes = FLASH_PAD_PATTERN,
) -> tuple[bytes, int, list[str]]:
    """Builds one flat buffer spanning every part's [dest_offset,
    last_addr] range, filling anything not covered by a part with a
    repeating `pad_pattern` (default: 0xFF 0xFF, so an uncovered region
    reads as a human-recognisable "ffff ffff ffff..." in a hex dump).
    Later parts (later in `parts` order) overwrite earlier ones if
    ranges happen to overlap.

    `model` (e.g. "3g", "4g_color", "5g", "mini1g", "nano1g", ...) is
    required: its hardcoded gfCS-insertion offset (IPOD_MODELS) is looked
    up and, only if that GFCS_REGION_SIZE-byte region is entirely
    pad-filler (i.e. not already covered by a real FwUp/flsh part), it is
    overwritten with that model's hardcoded "gfCS"/"SCfg" device-info
    struct (GFCS_STRUCTS), if one has been captured for it yet.

    Returns (buffer, base_offset, notes) where base_offset is the
    lowest dest_offset among all parts (the address the buffer's byte 0
    corresponds to), and notes is a list of human-readable strings
    describing what the gfCS-insertion step did.
    """
    if not parts:
        raise ValueError("no parts to combine")

    base = min(p.dest_offset for p in parts)
    end = max(p.last_addr for p in parts) + 1
    size = end - base

    if not pad_pattern:
        pad_pattern = FLASH_PAD_PATTERN
    reps = (size // len(pad_pattern)) + 1
    buf = bytearray((pad_pattern * reps)[:size])

    for p in parts:
        rel_start = p.dest_offset - base
        buf[rel_start:rel_start + p.payload_len] = p.payload()

    notes: list[str] = []
    info = resolve_model(model)  # raises ValueError if unknown
    offset = info.offset
    rel = offset - base

    if rel < 0 or rel + GFCS_REGION_SIZE > len(buf):
        notes.append(
            f"model {model!r}: gfCS region 0x{offset:x}-"
            f"0x{offset + GFCS_REGION_SIZE - 1:x} is outside the "
            f"combined range 0x{base:x}-0x{base + size - 1:x} - skipped")
    else:
        region = bytes(buf[rel:rel + GFCS_REGION_SIZE])
        expected_pad = (pad_pattern * ((GFCS_REGION_SIZE // len(pad_pattern)) + 1))[:GFCS_REGION_SIZE]
        if region != expected_pad:
            notes.append(
                f"model {model!r}: region 0x{offset:x}-"
                f"0x{offset + GFCS_REGION_SIZE - 1:x} is already covered "
                f"by real flash-part data - not overwritten")
        elif info.gfcs is None:
            notes.append(
                f"model {model!r}: no gfCS struct captured for "
                f"this model yet - region left padded (add it to "
                f"GFCS_STRUCTS/IPOD_MODELS once available)")
        else:
            struct_bytes = GFCS_STRUCTS[info.gfcs]
            buf[rel:rel + len(struct_bytes)] = struct_bytes
            notes.append(
                f"model {model!r}: inserted gfCS struct "
                f"{info.gfcs!r} at 0x{offset:x} ({len(struct_bytes)} bytes)")


    return bytes(buf), base, notes


# --------------------------------------------------------------------------
# HDD image building - pure Python (no dd / fdisk / mkdosfs): MBR or APM
# partition table, firmware partition, FAT32 data partition.
#
#   firmware dir version 0   -> APM only, firmware at block 4 (0x800)
#   firmware dir version 2/3 -> APM or MBR, firmware at block 63
# --------------------------------------------------------------------------

HDD_FW_BLOCK_V0 = 4
HDD_FW_BLOCK_V23 = 63
HDD_DEFAULT_SIZE_MIB = 90
HDD_DEFAULT_FW_SECTORS = 61440        # 30 MiB, as in the old shell scripts
HDD_FW_ROUND_SECTORS = 2048           # a larger firmware partition is rounded up to 1 MiB
MBR_DISK_SIGNATURE = 0x04206969
MBR_TYPE_FIRMWARE = 0x00
MBR_TYPE_FAT32_LBA = 0x0B
APM_FW_TYPE = "Apple_MDFW"
APM_FAT_TYPE = "DOS_FAT_32"
HDD_PATCH_DIR_OFFSETS = (0x4000, 0x4200)   # firmware-relative directory candidates

FAT32_RESERVED_SECTORS = 32
FAT32_MIN_CLUSTERS = 65525
FAT32_MAX_CLUSTERS = 0x0FFFFFF4
FAT32_VOLUME_ID = 0x4D65644F          # the constant mkfs.fat --invariant uses
FAT32_GEOM_SECTORS = 63
FAT32_GEOM_HEADS = 255


def hdd_firmware_block(fw: "FirmwareImage", scheme: str) -> int:
    """Block (512-byte sector) at which the firmware partition must start
    for this firmware file + partition scheme. Raises ValueError for
    unsupported combinations."""
    if scheme not in ("mbr", "apm"):
        raise ValueError(f"unknown partition scheme {scheme!r} (use 'mbr' or 'apm')")
    if fw.version == 0:
        if scheme != "apm":
            raise ValueError("version-0 firmware only supports APM images")
        ptr = struct.unpack_from("<I", fw.buf, DIR_POINTER_OFFSET)[0]
        if ptr != HDD_FW_BLOCK_V0 * SECTOR_SIZE:
            raise ValueError(
                f"version-0 firmware header offset is 0x{ptr:x}, expected "
                f"0x{HDD_FW_BLOCK_V0 * SECTOR_SIZE:x} - can't place it in an image")
        return HDD_FW_BLOCK_V0
    if fw.version in (2, 3):
        return HDD_FW_BLOCK_V23
    raise ValueError(f"unsupported firmware directory version {fw.version}")


def _lba_to_chs(lba: int) -> bytes:
    cyl = lba // (FAT32_GEOM_HEADS * FAT32_GEOM_SECTORS)
    head = (lba // FAT32_GEOM_SECTORS) % FAT32_GEOM_HEADS
    sec = lba % FAT32_GEOM_SECTORS + 1
    if cyl > 1023:
        cyl, head, sec = 1023, FAT32_GEOM_HEADS - 1, FAT32_GEOM_SECTORS
    return bytes([head, sec | ((cyl >> 8) & 3) << 6, cyl & 0xFF])


def _write_mbr(f, fw_start, fw_sectors, fat_start, fat_sectors) -> None:
    mbr = bytearray(SECTOR_SIZE)
    struct.pack_into("<I", mbr, 440, MBR_DISK_SIGNATURE)
    for i, (status, ptype, start, size) in enumerate((
            (0x80, MBR_TYPE_FIRMWARE, fw_start, fw_sectors),
            (0x00, MBR_TYPE_FAT32_LBA, fat_start, fat_sectors))):
        e = bytearray(16)
        e[0] = status
        e[1:4] = _lba_to_chs(start)
        e[4] = ptype
        e[5:8] = _lba_to_chs(start + size - 1)
        struct.pack_into("<II", e, 8, start, size)
        mbr[446 + i * 16: 446 + (i + 1) * 16] = e
    mbr[510:512] = b"\x55\xAA"
    f.seek(0)
    f.write(mbr)


def _write_apm(f, total_sectors, fw_start, fw_sectors, fat_start, fat_sectors) -> None:
    ddm = bytearray(SECTOR_SIZE)
    ddm[0:2] = b"ER"
    struct.pack_into(">H", ddm, 2, SECTOR_SIZE)
    struct.pack_into(">I", ddm, 4, total_sectors)
    f.seek(0)
    f.write(ddm)

    def entry(start, size, name, ptype):
        p = bytearray(SECTOR_SIZE)
        p[0:2] = b"PM"
        struct.pack_into(">I", p, 4, 3)            # number of map entries
        struct.pack_into(">I", p, 8, start)
        struct.pack_into(">I", p, 12, size)
        p[16:48] = name.encode().ljust(32, b"\0")
        p[48:80] = ptype.encode().ljust(32, b"\0")
        struct.pack_into(">I", p, 88, 0x33)         # status flags
        return p

    # the partition-map entry itself spans block 1 up to the firmware start
    for i, e in enumerate((
            entry(1, fw_start - 1, "Apple", "Apple_partition_map"),
            entry(fw_start, fw_sectors, "Firmware", APM_FW_TYPE),
            entry(fat_start, fat_sectors, "iPod", APM_FAT_TYPE))):
        f.seek((1 + i) * SECTOR_SIZE)
        f.write(e)


def _fat32_params(total_sectors: int) -> tuple[int, int, int]:
    """Returns (sectors_per_cluster, fat_size_sectors, cluster_count)."""
    if total_sectors <= 532480:        # <= 260 MB
        spc = 1
    elif total_sectors <= 16777216:    # <= 8 GB
        spc = 8
    elif total_sectors <= 33554432:    # <= 16 GB
        spc = 16
    elif total_sectors <= 67108864:    # <= 32 GB
        spc = 32
    else:
        spc = 64
    while spc >= 1:
        tmp1 = total_sectors - FAT32_RESERVED_SECTORS
        tmp2 = (256 * spc + 2) // 2
        fat_size = -(-tmp1 // tmp2)
        clusters = (tmp1 - 2 * fat_size) // spc
        if clusters >= FAT32_MIN_CLUSTERS:
            if clusters > FAT32_MAX_CLUSTERS:
                break
            return spc, fat_size, clusters
        spc //= 2
    raise ValueError(
        f"a {total_sectors * SECTOR_SIZE / 2**20:.1f} MiB partition is too small "
        f"(or too large) for a valid FAT32 volume - use a different --hdd-size")


def _format_fat32(f, start_sector: int, total_sectors: int) -> None:
    """Writes an empty FAT32 filesystem into the open file at
    `start_sector`. Only the metadata sectors are written; everything else
    stays zero (the file is created sparse where the OS supports it)."""
    spc, fat_size, clusters = _fat32_params(total_sectors)
    base = start_sector * SECTOR_SIZE

    boot = bytearray(SECTOR_SIZE)
    boot[0:3] = b"\xEB\x58\x90"
    boot[3:11] = b"mkfs.fat"
    struct.pack_into("<H", boot, 11, SECTOR_SIZE)
    boot[13] = spc
    struct.pack_into("<H", boot, 14, FAT32_RESERVED_SECTORS)
    boot[16] = 2                                      # number of FATs
    boot[21] = 0xF8                                   # media descriptor
    struct.pack_into("<H", boot, 24, FAT32_GEOM_SECTORS)
    struct.pack_into("<H", boot, 26, FAT32_GEOM_HEADS)
    struct.pack_into("<I", boot, 28, start_sector)    # hidden sectors
    struct.pack_into("<I", boot, 32, total_sectors)
    struct.pack_into("<I", boot, 36, fat_size)
    struct.pack_into("<I", boot, 44, 2)               # root directory cluster
    struct.pack_into("<H", boot, 48, 1)               # FSInfo sector
    struct.pack_into("<H", boot, 50, 6)               # backup boot sector
    boot[64] = 0x80                                   # drive number
    boot[66] = 0x29                                   # extended boot signature
    struct.pack_into("<I", boot, 67, FAT32_VOLUME_ID)
    boot[71:82] = b"NO NAME    "
    boot[82:90] = b"FAT32   "
    boot[510:512] = b"\x55\xAA"

    fsinfo = bytearray(SECTOR_SIZE)
    struct.pack_into("<I", fsinfo, 0, 0x41615252)
    struct.pack_into("<I", fsinfo, 484, 0x61417272)
    struct.pack_into("<I", fsinfo, 488, clusters - 1)  # free clusters (root uses one)
    struct.pack_into("<I", fsinfo, 492, 3)             # next free cluster hint
    struct.pack_into("<I", fsinfo, 508, 0xAA550000)

    for sector, data in ((0, boot), (1, fsinfo), (6, boot), (7, fsinfo)):
        f.seek(base + sector * SECTOR_SIZE)
        f.write(data)

    fat_head = bytearray(SECTOR_SIZE)
    struct.pack_into("<III", fat_head, 0, 0x0FFFFFF8, 0x0FFFFFFF, 0x0FFFFFFF)
    for n in range(2):
        f.seek(base + (FAT32_RESERVED_SECTORS + n * fat_size) * SECTOR_SIZE)
        f.write(fat_head)
    # root directory cluster (cluster 2) is left zeroed = empty


@dataclass
class HddLayout:
    scheme: str
    total_sectors: int
    fw_start: int
    fw_sectors: int
    fat_start: int
    fat_sectors: int


def hdd_layout(fw: "FirmwareImage", scheme: str, size_mib: int) -> HddLayout:
    fw_start = hdd_firmware_block(fw, scheme)
    need = -(-len(fw.buf) // SECTOR_SIZE)
    fw_sectors = HDD_DEFAULT_FW_SECTORS
    if need > fw_sectors:
        fw_sectors = -(-need // HDD_FW_ROUND_SECTORS) * HDD_FW_ROUND_SECTORS
    total = size_mib * 2048
    fat_start = fw_start + fw_sectors
    fat_sectors = total - fat_start
    if fat_sectors <= 0:
        raise ValueError(f"--hdd-size {size_mib} MiB is too small for the firmware partition")
    _fat32_params(fat_sectors)    # validates the size early
    return HddLayout(scheme, total, fw_start, fw_sectors, fat_start, fat_sectors)


def locate_hdd_firmware(path: str, scheme: str) -> tuple[int, int]:
    """Reads the partition table of an image and returns (byte_offset,
    byte_length) of the firmware partition: MBR partition 1, or the
    Apple_MDFW entry of an APM."""
    with open(path, "rb") as f:
        sec0 = f.read(SECTOR_SIZE)
        if scheme == "mbr":
            if sec0[510:512] != b"\x55\xAA":
                raise ValueError("no MBR signature found in the image")
            start, size = struct.unpack_from("<II", sec0, 446 + 8)
            if start == 0 or size == 0:
                raise ValueError("MBR partition 1 is empty")
            return start * SECTOR_SIZE, size * SECTOR_SIZE
        if sec0[0:2] != b"ER":
            raise ValueError("no Apple driver descriptor map found in the image")
        f.seek(SECTOR_SIZE)
        first = f.read(SECTOR_SIZE)
        if first[0:2] != b"PM":
            raise ValueError("no Apple partition map found in the image")
        count = struct.unpack_from(">I", first, 4)[0]
        for i in range(count):
            f.seek((1 + i) * SECTOR_SIZE)
            e = f.read(SECTOR_SIZE)
            if e[0:2] == b"PM" and e[48:80].rstrip(b"\0").decode("ascii", "replace") == APM_FW_TYPE:
                start, size = struct.unpack_from(">II", e, 8)
                return start * SECTOR_SIZE, size * SECTOR_SIZE
        raise ValueError(f"no {APM_FW_TYPE} partition found in the partition map")


def patch_hdd_aupd_ids(path: str, fw_offset: int) -> list[str]:
    """Patches the *image* (never the input firmware file): in every
    firmware directory found at fw_offset+0x4000 and/or +0x4200, sets the
    AUPD entry's id field to 1. Returns human-readable notes; raises
    ValueError if no AUPD entry could be patched."""
    notes: list[str] = []
    patched = 0
    with open(path, "r+b") as f:
        for cand in HDD_PATCH_DIR_OFFSETS:
            pos = fw_offset + cand
            f.seek(pos)
            raw = f.read(DIR_ENTRY_SIZE * 10)
            count = 0
            aupd_idx = None
            while (count + 1) * DIR_ENTRY_SIZE <= len(raw) and count < 10:
                e = raw[count * DIR_ENTRY_SIZE:(count + 1) * DIR_ENTRY_SIZE]
                if e[0:4] not in VALID_ENTRY_MARKERS or e[4:8] not in TAG_NAMES:
                    break
                if TAG_NAMES[e[4:8]] == "AUPD" and aupd_idx is None:
                    aupd_idx = count
                count += 1
            if count == 0:
                notes.append(f"no firmware directory at firmware+0x{cand:x}")
                continue
            if aupd_idx is None:
                notes.append(f"directory at firmware+0x{cand:x} has no AUPD entry")
                continue
            id_pos = pos + aupd_idx * DIR_ENTRY_SIZE + 8
            f.seek(id_pos)
            old = struct.unpack("<I", f.read(4))[0]
            f.seek(id_pos)
            f.write(struct.pack("<I", 1))
            patched += 1
            notes.append(f"directory at firmware+0x{cand:x}: AUPD id 0x{old:08x} -> 0x00000001")
    if not patched:
        raise ValueError("no firmware directory with an AUPD entry found in the image: "
                         + "; ".join(notes))
    return notes


def build_hdd_image(fw: "FirmwareImage", scheme: str, out_path: str,
                    size_mib: int = HDD_DEFAULT_SIZE_MIB) -> tuple[HddLayout, list[str]]:
    """Builds a complete HDD image (partition table + firmware partition +
    empty FAT32), then patches the AUPD id inside the image. Returns
    (layout, notes)."""
    scheme = scheme.lower()
    lay = hdd_layout(fw, scheme, size_mib)

    out_dir = os.path.dirname(os.path.abspath(out_path))
    if out_dir:
        os.makedirs(out_dir, exist_ok=True)
    with open(out_path, "wb") as f:
        f.truncate(lay.total_sectors * SECTOR_SIZE)
        if scheme == "mbr":
            _write_mbr(f, lay.fw_start, lay.fw_sectors, lay.fat_start, lay.fat_sectors)
        else:
            _write_apm(f, lay.total_sectors, lay.fw_start, lay.fw_sectors,
                       lay.fat_start, lay.fat_sectors)
        f.seek(lay.fw_start * SECTOR_SIZE)
        f.write(fw.buf)
        _format_fat32(f, lay.fat_start, lay.fat_sectors)

    # find the firmware again through the partition table we just wrote
    fw_off, _ = locate_hdd_firmware(out_path, scheme)
    notes = patch_hdd_aupd_ids(out_path, fw_off)
    return lay, notes


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

_FORCE = False


class OutputExistsError(Exception):
    pass


def _check_output(path: str) -> None:
    """Refuses to clobber an existing file unless --force was given."""
    if os.path.exists(path) and not _FORCE:
        raise OutputExistsError(f"{path!r} already exists - pass --force to overwrite it")


def _print_listing(fw: "FirmwareImage") -> None:
    print(f"fwoffset=0x{fw.fwoffset:x}  directory version={fw.version}  "
          f"{len(fw.images)} image(s):\n")
    for img in fw.images:
        try:
            ok = img.checksum_ok()
            status = "checksum OK" if ok else "CHECKSUM MISMATCH"
        except Exception as e:
            status = f"error: {e}"
        print(f"  {img.name:5s}  devOffset=0x{img.dev_offset:08x}  "
              f"len={img.length:>9,d} bytes  chksum=0x{img.checksum:08x}  [{status}]")


def _write_file(path: str, data: bytes) -> None:
    out_dir = os.path.dirname(os.path.abspath(path))
    if out_dir:
        os.makedirs(out_dir, exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)


def _write_image(img: "FirmwareImageEntry", out_path: str, decoded: bool) -> bool:
    """Writes either the raw on-disk bytes (decoded=False) or the fully
    decoded bytes (decoded=True, transparently RC4-decrypting AUPD on
    version-3 directories) to out_path. Returns True on success."""
    try:
        data = img.decoded_data() if decoded else img.raw_data()
    except Exception as e:
        print(f"[!] {img.name}: failed to {'decode' if decoded else 'read'} - {e}")
        return False

    _write_file(out_path, data)

    calc = additive_checksum(data)
    if decoded:
        status = "OK" if calc == img.checksum else "MISMATCH"
        print(f"[+] {img.name} (decoded) -> {out_path} "
              f"({len(data):,} bytes)  checksum: calc=0x{calc:08x} "
              f"expected=0x{img.checksum:08x} [{status}]")
    else:
        # Raw/as-is bytes only match the header checksum when the image
        # isn't obfuscated on this directory version (e.g. AUPD on v2,
        # or any non-AUPD image on any version) - a "mismatch" here for
        # AUPD-on-v3 is expected, not an error.
        note = " (expected: this image is obfuscated on this firmware version)" \
            if calc != img.checksum else ""
        print(f"[+] {img.name} (raw) -> {out_path} ({len(data):,} bytes)  "
              f"checksum: calc=0x{calc:08x} header=0x{img.checksum:08x}{note}")
    return True


def _find_image_or_die(fw: "FirmwareImage", name: str) -> "FirmwareImageEntry":
    img = fw.find(name)
    if img is None:
        print(f"[!] No image named {name!r} found. "
              f"Available: {', '.join(i.name for i in fw.images)}")
        sys.exit(1)
    return img


_HELP_EXAMPLES = """\
examples:
  %(prog)s -i Firmware-5.4.2.1 -l
  %(prog)s -i Firmware-5.4.2.1 -m 4g -f 4g_flash.bin
  %(prog)s -i Firmware-5.4.2.1 -d mbr 4g_hdd.bin
  %(prog)s -i Firmware-5.4.2.1 -m 4g -f 4g_flash.bin -d mbr 4g_hdd.bin
  %(prog)s -i Firmware-5.4.2.1 --extract-decoded AUPD aupd.bin

Output files go to the current directory; an existing file is an error
unless --force is given. Steps run in a fixed order, whatever the order
of the flags."""


def build_arg_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(
        description="Inspect and rebuild iPod firmware: list/extract images, "
                    "build the NOR flash image and HDD (MBR/APM) images.",
        epilog=_HELP_EXAMPLES,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-i", "--input", required=True, dest="firmware", metavar="FILE",
                     help="input Firmware file")
    ap.add_argument("-m", "--model", metavar="MODEL",
                     help="iPod model, e.g. 1g 2g 3g 4g 4g_photo 4g_color 5g mini1g mini2g "
                          "nano1g (required with -f)")
    ap.add_argument("-f", "--build-flash", nargs="?", const="", default=None,
                     metavar="OUTPUT_FILE",
                     help="rebuild the NOR flash image from the AUPD (needs -m; "
                          "default name: <input>_flash.bin)")
    ap.add_argument("-d", "--build-hdd", nargs="+", default=None,
                     metavar=("{mbr,apm}", "OUTPUT_FILE"),
                     help="build an HDD image with the firmware and an empty FAT32 "
                          "partition (default name: <input>_<type>.img). "
                          "v0 firmware: apm only; v2/v3: mbr or apm")
    ap.add_argument("-l", "--list", action="store_true",
                     help="list the images found in the firmware directory")
    ap.add_argument("--force", action="store_true",
                     help="overwrite existing output files")

    more = ap.add_argument_group("less frequent options")
    more.add_argument("--hdd-size", type=int, default=HDD_DEFAULT_SIZE_MIB, metavar="MiB",
                      help=f"total HDD image size for -d (default: {HDD_DEFAULT_SIZE_MIB})")
    more.add_argument("--extract-all", action="store_true",
                      help="extract every image, decoded, as <input>_<name>.bin")
    more.add_argument("--extract-raw", action="append", nargs=2,
                      metavar=("IMAGE", "OUTPUT_FILE"),
                      help="extract IMAGE exactly as stored on disk (repeatable)")
    more.add_argument("--extract-decoded", action="append", nargs=2,
                      metavar=("IMAGE", "OUTPUT_FILE"),
                      help="extract IMAGE decoded, i.e. AUPD RC4-decrypted on v3 (repeatable)")
    more.add_argument("--extract-flash-parts", action="store_true",
                      help="extract each flash part of the AUPD separately, as "
                           "<input>_<destOffset>-<lastAddr>.bin")
    return ap


def main():
    global _FORCE
    ap = build_arg_parser()
    args = ap.parse_args()
    _FORCE = args.force

    if not (args.list or args.extract_all or args.extract_raw or args.extract_decoded
            or args.extract_flash_parts or args.build_flash is not None
            or args.build_hdd):
        ap.error("nothing to do - pass at least one of -l, -f, -d, "
                 "--extract-all, --extract-raw, --extract-decoded, --extract-flash-parts")

    if args.build_flash is not None:
        if not args.model:
            ap.error("-f/--build-flash requires -m/--model")
        try:
            resolve_model(args.model)
        except ValueError as e:
            ap.error(str(e))

    hdd_type = hdd_out = None
    if args.build_hdd:
        if len(args.build_hdd) > 2:
            ap.error("-d/--build-hdd takes a type (mbr or apm) and an optional output file")
        hdd_type = args.build_hdd[0].lower()
        if hdd_type not in ("mbr", "apm"):
            ap.error(f"-d/--build-hdd: invalid type {args.build_hdd[0]!r} (choose 'mbr' or 'apm')")
        if len(args.build_hdd) == 2:
            hdd_out = args.build_hdd[1]

    try:
        fw = FirmwareImage.load(args.firmware)
    except Exception as e:
        print(f"[!] Failed to load {args.firmware!r}: {e}")
        sys.exit(1)

    stem = os.path.basename(args.firmware)
    flash_out = None
    if args.build_flash is not None:
        flash_out = args.build_flash or f"{stem}_flash.bin"
    if hdd_type and hdd_out is None:
        hdd_out = f"{stem}_{hdd_type}.img"

    # Refuse to overwrite anything before doing any work.
    planned = []
    if args.extract_all:
        planned += [f"{stem}_{img.name.lower()}.bin" for img in fw.images]
    planned += [p for _, p in (args.extract_raw or [])]
    planned += [p for _, p in (args.extract_decoded or [])]
    planned += [p for p in (flash_out, hdd_out) if p]
    try:
        for path in planned:
            _check_output(path)
    except OutputExistsError as e:
        print(f"[!] {e}")
        sys.exit(1)

    # Pipeline: run every requested step in a fixed, predictable order,
    # regardless of the order flags were given on the command line.
    if args.list:
        _print_listing(fw)

    if args.extract_all:
        for img in fw.images:
            _write_image(img, f"{stem}_{img.name.lower()}.bin", decoded=True)

    for image_name, out_path in (args.extract_raw or []):
        _write_image(_find_image_or_die(fw, image_name), out_path, decoded=False)

    for image_name, out_path in (args.extract_decoded or []):
        _write_image(_find_image_or_die(fw, image_name), out_path, decoded=True)

    if args.extract_flash_parts:
        parts, note = get_flash_parts(fw)
        if note:
            print(f"[*] {note}")
        if not parts:
            print("[!] No flash parts found inside AUPD.")
        else:
            addr_width = max(5, len(f"{max(p.last_addr for p in parts):x}"))
            outs = [p.suggested_filename(stem, addr_width) for p in parts]
            try:
                for path in outs:
                    _check_output(path)
            except OutputExistsError as e:
                print(f"[!] {e}")
                sys.exit(1)
            print(f"[*] Found {len(parts)} flash part(s) inside AUPD:")
            for p, path in zip(parts, outs):
                _write_file(path, p.payload())
                print(f"    dest=0x{p.dest_offset:08x}  len=0x{p.payload_len:x}  -> {path}")

    if flash_out:
        parts, note = get_flash_parts(fw)
        if note:
            print(f"[*] {note}")
        if not parts:
            print("[!] No flash parts found inside AUPD - nothing to build.")
        else:
            buf, base, notes = combine_fwup_parts(parts, args.model)
            _write_file(flash_out, buf)
            covered = sum(p.payload_len for p in parts)
            print(f"[+] Built flash image from {len(parts)} part(s) -> {flash_out} "
                  f"({len(buf):,} bytes, base=0x{base:08x}, "
                  f"{covered:,} bytes covered, "
                  f"{len(buf) - covered:,} bytes padded with 0xFFFF)")
            for note in notes:
                print(f"    {note}")

    if hdd_type:
        try:
            lay, notes = build_hdd_image(fw, hdd_type, hdd_out, args.hdd_size)
        except ValueError as e:
            print(f"[!] Can't build HDD image: {e}")
            sys.exit(1)
        print(f"[+] {hdd_type.upper()} HDD image -> {hdd_out} "
              f"({lay.total_sectors * SECTOR_SIZE:,} bytes)")
        print(f"    firmware partition: block {lay.fw_start}, {lay.fw_sectors} sectors")
        print(f"    FAT32 partition:    block {lay.fat_start}, {lay.fat_sectors} sectors")
        for note in notes:
            print(f"    {note}")


if __name__ == "__main__":
    main()
