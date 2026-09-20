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

Usage (pipeline-style: every flag is independent and can be combined in a
single invocation, executed in this fixed order: list, extract-all,
extract-raw, extract-decoded):

    # List everything in a Firmware file
    python3 ipod_firmware_tool.py -i Firmware-5.4.2.1 -l

    # Extract every image, decoded, into a folder
    python3 ipod_firmware_tool.py -i Firmware-5.4.2.1 -a -o extracted/

    # Extract AUPD exactly as stored on disk (still RC4-obfuscated on v3,
    # plain on v2) - useful if you want the raw/original bytes
    python3 ipod_firmware_tool.py -i Firmware-5.4.2.1 -x AUPD aupd_raw.bin

    # Extract AUPD fully decoded (RC4-decrypted on v3, as-is on v2)
    python3 ipod_firmware_tool.py -i Firmware-5.4.2.1 -d AUPD aupd_decoded.bin

    # Everything at once, one shot
    python3 ipod_firmware_tool.py -i Firmware-5.4.2.1 -l -a -o extracted/ \
        -x AUPD aupd_raw.bin -d AUPD aupd_decoded.bin -x OSOS osos_raw.bin

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
# "gfCS"/"SCfg" device-info structs, hardcoded per iPod generation/model,
# captured verbatim from real devices. Each element's comment gives its
# logical (un-reversed) 4-char tag, e.g. "mNrS" on disk is logical "SrNm".
# --------------------------------------------------------------------------

GFCS_STRUCTS: dict[str, bytes] = {
    # Replace with proper dumps when/if aval    # !!! DO NOT RE-ORDER THE ELEMENTS !!! #
    "1g2g": _build_gfcs_struct([ # copy-paste from 3g + unk elem removed + HwVr fixed
        ("6D 4E 72 53", "32 58 35 31 36 30 32 52 50 51 35 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "72 15 4C 00 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "0A 43 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "00 00 00 00 01 A1 FE FF 00 00 00 00 00 00 00 00"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 01 00 02 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
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
        ("64 49 77 48", "4A 76 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 14 00 05 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 00 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "4D 41 30 37 39 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
        ("74 6E 6F 43", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # tnoC/Cont
        ("56 6D 72 44", "00 00 00 00 06 00 00 00 00 00 00 00 00 00 00 00"),  # VmrD/DrmV
    ]),
    "4g_photo": _build_gfcs_struct([ # copy-paste from 4g color + HwVr fixed
        ("6D 4E 72 53", "4A 51 35 33 37 41 30 4E 54 44 53 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "00 00 00 01 36 4F 79 14 00 27 0A 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "4A 76 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 00 00 06 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 00 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "4D 41 30 37 39 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
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
        ("64 49 77 48", "4A 76 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 13 00 04 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 00 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "4D 41 30 37 39 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
        ("74 6E 6F 43", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # tnoC/Cont
        ("56 6D 72 44", "00 00 00 00 06 00 00 00 00 00 00 00 00 00 00 00"),  # VmrD/DrmV
    ]),
    "mini2g": _build_gfcs_struct([ # copy-paste from 4g color + HwVr fixed
        ("6D 4E 72 53", "4A 51 35 33 37 41 30 4E 54 44 53 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "00 00 00 01 36 4F 79 14 00 27 0A 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "4A 76 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("79 72 74 42", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # yrtB/Btry
        ("41 63 74 52", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # ActR/RtcA
        ("72 56 77 48", "00 00 00 00 02 00 07 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 00 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "4D 41 30 37 39 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
        ("74 6E 6F 43", "FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF"),  # tnoC/Cont
        ("56 6D 72 44", "00 00 00 00 06 00 00 00 00 00 00 00 00 00 00 00"),  # VmrD/DrmV
    ]),
    "nano1g": _build_gfcs_struct([ # copy-paste from 5g + HwVr fixed
        ("6D 4E 72 53", "34 4A 36 30 38 32 59 37 54 58 4B 00 00 00 00 00"),  # mNrS/SrNm
        ("64 49 77 46", "00 00 00 01 26 E7 EF 14 00 27 0A 00 00 00 00 00"),  # dIwF/FwId
        ("64 49 77 48", "3A 76 01 82 00 00 00 00 00 00 00 00 00 00 00 00"),  # dIwH/HwId
        ("72 56 77 48", "00 00 00 00 06 00 0c 00 00 00 00 00 00 00 00 00"),  # rVwH/HwVr
        ("6E 67 65 52", "01 00 02 00 01 00 02 00 00 00 00 00 00 00 00 00"),  # ngeR/Regn
        ("23 64 6F 4D", "4D 41 31 34 36 00 00 00 00 00 00 00 00 00 00 00"),  # #doM/Mod#
        ("56 6D 72 44", "00 00 00 00 06 00 00 00 00 00 00 00 00 00 00 00"),  # VmrD/DrmV
    ]),
}

# --------------------------------------------------------------------------
# Generation -> (gfCS insertion offset, gfCS struct to use) lookup.
# `gfcs` is None where we don't yet have a captured struct for that exact
# generation/model - insertion is simply skipped (with a note) for those
# until real data is available to add here.
# --------------------------------------------------------------------------

@dataclass
class GenerationInfo:
    offset: int
    gfcs: Optional[str]  # key into GFCS_STRUCTS, or None if not yet known


IPOD_GENERATIONS: dict[str, GenerationInfo] = {
    "1g2g":     GenerationInfo(0x2000, "1g2g"),
    "3g":       GenerationInfo(0x2000, "3g"),
    "4g_mono":  GenerationInfo(0x2000, "4g_mono"),
    "4g_photo": GenerationInfo(0x2000, "4g_photo"),
    "4g_color": GenerationInfo(0x2000, "4g_color"),
    "5g":       GenerationInfo(0x4000, "5g"),
    "mini1g":   GenerationInfo(0x2000, "mini1g"),
    "mini2g":   GenerationInfo(0x2000, "mini2g"),
    "nano1g":   GenerationInfo(0x4000, "nano1g"),
}

# A few convenience aliases so common ways of typing a generation still work.
_GENERATION_ALIASES = {
    "1": "1g2g", "gen1": "1g2g",
    "2": "1g2g", "gen2": "1g2g",
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


def resolve_generation(name: str) -> GenerationInfo:
    """Looks up a generation string, case/whitespace/punctuation
    insensitively, with a helpful error listing valid names if it
    doesn't match anything known."""
    key = name.strip().lower().replace(" ", "_").replace("-", "_")
    key = _GENERATION_ALIASES.get(key, key)
    if key not in IPOD_GENERATIONS:
        valid = ", ".join(sorted(set(IPOD_GENERATIONS) | set(_GENERATION_ALIASES)))
        raise ValueError(f"unknown iPod generation {name!r}. Valid options: {valid}")
    return IPOD_GENERATIONS[key]


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
    """Represents a parsed iPod firmware partition / Firmware file."""
    buf: bytes
    partition_base: int
    fwoffset: int
    version: int
    images: list[FirmwareImageEntry]

    @classmethod
    def load(cls, path: str, partition_base: int = 0) -> "FirmwareImage":
        with open(path, "rb") as f:
            buf = f.read()
        return cls.from_bytes(buf, partition_base=partition_base)

    @classmethod
    def from_bytes(cls, buf: bytes, partition_base: int = 0) -> "FirmwareImage":
        base = partition_base
        if len(buf) < base + 0x110:
            raise ValueError("file too small to contain a firmware header")

        magic = buf[base + HEADER_MAGIC_OFFSET: base + HEADER_MAGIC_OFFSET + 4]
        if magic != HEADER_MAGIC:
            raise ValueError(
                f"no firmware header found at offset 0x{base:x} "
                f"(expected magic {HEADER_MAGIC!r}, got {magic!r}). "
                "This doesn't look like a firmware partition / Firmware "
                "file - note a raw bootloader/NOR ROM dump normally does "
                "NOT contain this header; it lives in the firmware "
                "partition on disk instead."
            )

        dir_ptr = struct.unpack_from("<I", buf, base + DIR_POINTER_OFFSET)[0]
        diroffset = dir_ptr + DIR_POINTER_ADDEND
        version = struct.unpack_from("<H", buf, base + DIR_VERSION_OFFSET)[0]

        images: list[FirmwareImageEntry] = []
        pos = base + diroffset
        while pos + DIR_ENTRY_SIZE <= len(buf) and len(images) < 10:
            marker = buf[pos:pos + DIR_ENTRY_MARKER_SIZE]
            if marker not in VALID_ENTRY_MARKERS:
                break

            fields_off = pos + DIR_ENTRY_MARKER_SIZE
            tag_raw = buf[fields_off:fields_off + 4]
            if tag_raw not in TAG_NAMES:
                break

            id_        = struct.unpack_from("<I", buf, fields_off + 0x04)[0]
            dev_offset = struct.unpack_from("<I", buf, fields_off + 0x08)[0]
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

        if len(images) > 1 and version == 2:
            fwoffset = base
        else:
            fwoffset = base + SECTOR_SIZE

        fw = cls(buf=buf, partition_base=base, fwoffset=fwoffset,
                 version=version, images=images)
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


def combine_fwup_parts(
    parts: list[FwUpPart],
    pad_pattern: bytes = FLASH_PAD_PATTERN,
    generation: Optional[str] = None,
) -> tuple[bytes, int, list[str]]:
    """Builds one flat buffer spanning every part's [dest_offset,
    last_addr] range, filling anything not covered by a part with a
    repeating `pad_pattern` (default: 0xFF 0xFF, so an uncovered region
    reads as a human-recognisable "ffff ffff ffff..." in a hex dump).
    Later parts (later in `parts` order) overwrite earlier ones if
    ranges happen to overlap.

    If `generation` is given (e.g. "3g", "4g_color", "5g", "mini1",
    "nano1", ...), looks up its hardcoded gfCS-insertion offset
    (IPOD_GENERATIONS) and, only if that GFCS_REGION_SIZE-byte region is
    currently entirely unpadded-filler (i.e. not already covered by a
    real FwUp/flsh part), overwrites it with that generation's
    hardcoded "gfCS"/"SCfg" device-info struct (GFCS_STRUCTS) if one has
    been captured for it yet.

    Returns (buffer, base_offset, notes) where base_offset is the
    lowest dest_offset among all parts (the address the buffer's byte 0
    corresponds to), and notes is a list of human-readable strings
    describing what the generation-handling step did (empty if
    `generation` was not given).
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
    if generation is not None:
        info = resolve_generation(generation)  # raises ValueError if unknown
        offset = info.offset
        rel = offset - base

        if rel < 0 or rel + GFCS_REGION_SIZE > len(buf):
            notes.append(
                f"generation {generation!r}: gfCS region 0x{offset:x}-"
                f"0x{offset + GFCS_REGION_SIZE - 1:x} is outside the "
                f"combined range 0x{base:x}-0x{base + size - 1:x} - skipped")
        else:
            region = bytes(buf[rel:rel + GFCS_REGION_SIZE])
            expected_pad = (pad_pattern * ((GFCS_REGION_SIZE // len(pad_pattern)) + 1))[:GFCS_REGION_SIZE]
            if region != expected_pad:
                notes.append(
                    f"generation {generation!r}: region 0x{offset:x}-"
                    f"0x{offset + GFCS_REGION_SIZE - 1:x} is already covered "
                    f"by real flash-part data - not overwritten")
            elif info.gfcs is None:
                notes.append(
                    f"generation {generation!r}: no gfCS struct captured for "
                    f"this generation yet - region left padded (add it to "
                    f"GFCS_STRUCTS/IPOD_GENERATIONS once available)")
            else:
                struct_bytes = GFCS_STRUCTS[info.gfcs]
                buf[rel:rel + len(struct_bytes)] = struct_bytes
                notes.append(
                    f"generation {generation!r}: inserted gfCS struct "
                    f"{info.gfcs!r} at 0x{offset:x} ({len(struct_bytes)} bytes)")

    return bytes(buf), base, notes


# --------------------------------------------------------------------------
# CLI - pipeline style: every flag is independent and they can all be
# combined in a single invocation, e.g.:
#   ipod_firmware_tool.py -i Firmware-5_4_2.1 -l -a -o extracted/
#   ipod_firmware_tool.py -i Firmware-5_4_2.1 -x AUPD aupd_raw.bin -d AUPD aupd_dec.bin
# --------------------------------------------------------------------------

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


def _write_image(img: "FirmwareImageEntry", out_path: str, decoded: bool) -> bool:
    """Writes either the raw on-disk bytes (decoded=False) or the fully
    decoded bytes (decoded=True, transparently RC4-decrypting AUPD on
    version-3 directories) to out_path. Returns True on success."""
    try:
        data = img.decoded_data() if decoded else img.raw_data()
    except Exception as e:
        print(f"[!] {img.name}: failed to {'decode' if decoded else 'read'} - {e}")
        return False

    out_dir = os.path.dirname(os.path.abspath(out_path))
    if out_dir:
        os.makedirs(out_dir, exist_ok=True)
    with open(out_path, "wb") as f:
        f.write(data)

    calc = additive_checksum(data)
    if decoded:
        status = "OK" if calc == img.checksum else "MISMATCH"
        print(f"[+] {img.name} ({'decoded' if decoded else 'raw'}) -> {out_path} "
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


def build_arg_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-i", "--input", required=True, dest="firmware",
                     help="Input Firmware file / firmware partition / disk image")
    ap.add_argument("-l", "--list", action="store_true",
                     help="List the images found in the firmware directory")
    ap.add_argument("-a", "--extract-all", action="store_true",
                     help="Extract every image, decoded (decrypting AUPD when needed), "
                          "into --out-dir")
    ap.add_argument("-x", "--extract-raw", action="append", nargs=2,
                     metavar=("IMAGE", "OUTPUT_FILE"),
                     help="Extract IMAGE (e.g. AUPD or OSOS) exactly as stored on disk "
                          "(still RC4-obfuscated for AUPD on version 3, plain on version 2). "
                          "Can be given more than once.")
    ap.add_argument("-d", "--extract-decoded", action="append", nargs=2,
                     metavar=("IMAGE", "OUTPUT_FILE"),
                     help="Extract IMAGE fully decoded (AUPD is RC4-decrypted on version 3, "
                          "left as-is on version 2; every other image is unaffected). "
                          "Can be given more than once.")
    ap.add_argument("-o", "--out-dir", default="extracted",
                     help="Output directory for -a/--extract-all (default: ./extracted)")
    ap.add_argument("-p", "--extract-flash-parts", action="store_true",
                     help="Find every 'FwUp'/'flsh' part inside the (decoded) AUPD image "
                          "and extract each one independently into --out-dir, named "
                          "'<input>_<destOffset>-<lastAddr>.bin'")
    ap.add_argument("-c", "--combine-flash", nargs="?", const="", default=None,
                     metavar="OUTPUT_FILE",
                     help="Combine every 'FwUp'/'flsh' part found inside the (decoded) "
                          "AUPD image into a single flat file spanning their full address "
                          "range, padding any uncovered region with a repeating 0xFF 0xFF "
                          "pattern. Default output name: '<input>_flash.bin' in --out-dir; "
                          "pass a path to override.")
    ap.add_argument("-g", "--generation",
                     help="iPod generation (e.g. 3g, 4g, 4g_color, 5g, mini1, mini2, nano1) "
                          "used with -c/--combine-flash: if the generation's known gfCS "
                          "insertion region isn't already covered by a real flash part, "
                          "fills it in with that generation's hardcoded gfCS/SCfg struct.")
    ap.add_argument("--partition-base", type=lambda x: int(x, 0), default=0,
                     help="Offset within --input where the firmware partition header starts")
    return ap


def _get_decoded_aupd_or_die(fw: "FirmwareImage") -> bytes:
    img = fw.find("AUPD")
    if img is None:
        print("[!] This firmware has no AUPD image.")
        sys.exit(1)
    try:
        return img.decoded_data()
    except Exception as e:
        print(f"[!] Failed to decode AUPD: {e}")
        sys.exit(1)


def main():
    args = build_arg_parser().parse_args()

    if not (args.list or args.extract_all or args.extract_raw or args.extract_decoded
            or args.extract_flash_parts or args.combine_flash is not None):
        build_arg_parser().error(
            "nothing to do - pass at least one of -l, -a, -x, -d, -p, -c")

    try:
        fw = FirmwareImage.load(args.firmware, partition_base=args.partition_base)
    except Exception as e:
        print(f"[!] Failed to load {args.firmware!r}: {e}")
        sys.exit(1)

    stem = os.path.basename(args.firmware)

    # Pipeline: run every requested step in a fixed, predictable order,
    # regardless of the order flags were given on the command line.
    if args.list:
        _print_listing(fw)

    if args.extract_all:
        os.makedirs(args.out_dir, exist_ok=True)
        for img in fw.images:
            out_path = os.path.join(args.out_dir, f"{img.name.lower()}.bin")
            _write_image(img, out_path, decoded=True)

    for image_name, out_path in (args.extract_raw or []):
        img = _find_image_or_die(fw, image_name)
        _write_image(img, out_path, decoded=False)

    for image_name, out_path in (args.extract_decoded or []):
        img = _find_image_or_die(fw, image_name)
        _write_image(img, out_path, decoded=True)

    if args.extract_flash_parts:
        aupd_data = _get_decoded_aupd_or_die(fw)
        parts = find_fwup_parts(aupd_data)
        if not parts:
            print("[!] No 'FwUp'/'flsh' parts found inside AUPD.")
        else:
            os.makedirs(args.out_dir, exist_ok=True)
            addr_width = max(5, len(f"{max(p.last_addr for p in parts):x}"))
            print(f"[*] Found {len(parts)} flash part(s) inside AUPD:")
            for p in parts:
                out_path = os.path.join(args.out_dir, p.suggested_filename(stem, addr_width))
                with open(out_path, "wb") as f:
                    f.write(p.payload())
                print(f"    dest=0x{p.dest_offset:08x}  len=0x{p.payload_len:x}  "
                      f"-> {out_path}")

    if args.combine_flash is not None:
        aupd_data = _get_decoded_aupd_or_die(fw)
        parts = find_fwup_parts(aupd_data)
        if not parts:
            print("[!] No 'FwUp'/'flsh' parts found inside AUPD - nothing to combine.")
        else:
            out_path = args.combine_flash or os.path.join(args.out_dir, f"{stem}_flash.bin")
            out_dir = os.path.dirname(os.path.abspath(out_path))
            if out_dir:
                os.makedirs(out_dir, exist_ok=True)
            try:
                buf, base, notes = combine_fwup_parts(parts, generation=args.generation)
            except ValueError as e:
                print(f"[!] {e}")
                sys.exit(1)
            with open(out_path, "wb") as f:
                f.write(buf)
            covered = sum(p.payload_len for p in parts)
            print(f"[+] Combined {len(parts)} part(s) -> {out_path} "
                  f"({len(buf):,} bytes, base=0x{base:08x}, "
                  f"{covered:,} bytes covered, "
                  f"{len(buf) - covered:,} bytes padded with 0xFFFF)")
            for note in notes:
                print(f"    {note}")


if __name__ == "__main__":
    main()
