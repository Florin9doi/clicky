use crate::devices::prelude::*;

use byteorder::{ByteOrder, LittleEndian};

#[derive(PartialEq, Clone, Copy)]
enum CFIState {
    ReadArrayMode,
    CommandPreambleAA,
    CommandPreamble55,
    ReadSoftwareID,
    ReadStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlashChip {
    pub size: u32,
    pub vendor_id: u16,
    pub device_id: u16,
}

impl FlashChip {
    // 3g
    pub const LH28F800BGHB: FlashChip = FlashChip {
        size: 1024 * 1024,
        vendor_id: 0x00b0,
        device_id: 0x0060,
    };
    // 4g / color / 5g
    pub const SST39WF800A: FlashChip = FlashChip {
        size: 1024 * 1024,
        vendor_id: 0x00bf,
        device_id: 0x273f,
    };
    // nano1g
    pub const SST39WF400A: FlashChip = FlashChip {
        size: 512 * 1024,
        vendor_id: 0x00bf,
        device_id: 0x272f,
    };
}

/// Internal iPod Flash ROM. Defaults to HLE mode (where only a few critical
/// memory locations can be read). Use the `use_dump` method if you have a dump
/// of a real iPod's flash ROM.
pub struct Flash {
    dump: Option<Box<[u8]>>,
    state: CFIState,
    chip: FlashChip,
}

impl std::fmt::Debug for Flash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flash")
            .field("dump", &self.dump.as_ref().map(|_| "[...]"))
            .field("chip", &self.chip)
            .finish()
    }
}

impl Flash {
    pub fn new(chip: FlashChip) -> Flash {
        Flash {
            dump: None,
            state: CFIState::ReadArrayMode,
            chip,
        }
    }

    pub fn new_with_dump(dump: Box<[u8]>, chip: FlashChip) -> Result<Flash, &'static str> {
        if dump.len() != chip.size as usize {
            return Err("Flash ROM dump must be exactly 512KB or 1MB");
        }
        Ok(Flash {
            dump: Some(dump),
            state: CFIState::ReadArrayMode,
            chip,
        })
    }

    pub fn use_dump(&mut self, dump: Box<[u8]>) -> Result<(), &'static str> {
        if dump.len() != self.chip.size as usize {
            return Err("Flash ROM dump must be exactly 512KB or 1MB");
        }
        self.dump = Some(dump);
        Ok(())
    }

    pub fn is_hle(&self) -> bool {
        self.dump.is_none()
    }

    fn hle_vals(offset: u32) -> MemResult<u32> {
        match offset {
            // used to checks whether SCfg is stored at 0x2000 or 0x4000 (5th gen/Nano1G)
            0x2000 => Ok(u32::from_le_bytes(*b"gfCS")),
            // hardware revision magic number
            // see: https://www.rockbox.org/wiki/IpodHardwareInfo
            0x2084 => Ok(0x0005_0014), // iPod 4th Gen
            0x405c => Ok(0x000B_0005), // iPod 5th Gen
            _ => Err(Unimplemented),
        }
    }
}

impl Device for Flash {
    fn kind(&self) -> &'static str {
        "Flash Rom"
    }

    fn label(&self) -> Option<&'static str> {
        Some(if self.is_hle() { "HLE" } else { "Dumped" })
    }

    fn probe(&self, offset: u32) -> Probe {
        if offset >= self.chip.size {
            Probe::Unmapped
        } else {
            Probe::Register("<flash rom>")
        }
    }
}

impl Memory for Flash {
    fn r8(&mut self, offset: u32) -> MemResult<u8> {
        {debug!(target: "FLS", "r8 offset:{:x} ", offset);}
        if offset >= self.chip.size {
            return Err(Unexpected);
        }

        if let Some(dump) = self.dump.as_ref() {
            let offset = offset as usize;
            let val = dump[offset];
            return Ok(val);
        }

        // don't support unaligned HLE reads
        Err(Unimplemented)
    }

    fn r16(&mut self, offset: u32) -> MemResult<u16> {
        {debug!(target: "FLS", "r16 offset:{:x} ", offset);}
        if offset >= self.chip.size {
            return Err(Unexpected);
        }

        match (self.state, offset >> 1) {
            (CFIState::ReadArrayMode, _) => {
                if let Some(dump) = self.dump.as_ref() {
                    let offset = offset as usize;
                    let val = LittleEndian::read_u16(&dump[offset..offset + 2]);
                    Ok(val)
                } else {
                    Err(Unimplemented)
                }
                
            }
            (CFIState::ReadSoftwareID, 0x0) => Ok(self.chip.vendor_id),
            (CFIState::ReadSoftwareID, 0x1) => Ok(self.chip.device_id),
            (CFIState::ReadStatus, _) => Ok(0x80),
            _ => Err(Unimplemented),
        }        
    }

    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        {debug!(target: "FLS", "r32 offset:{:x} ", offset);}
        if offset >= self.chip.size {
            return Err(Unexpected);
        }

        if let Some(dump) = self.dump.as_ref() {
            let offset = offset as usize;
            let val = LittleEndian::read_u32(&dump[offset..offset + 4]);
            return Ok(val);
        }

        Self::hle_vals(offset)
    }

    fn w8(&mut self, _offset: u32, _val: u8) -> MemResult<()> {
        {trace!(target: "FLS", "w8 offset:{:x} val:0x{:x}", _offset, _val);}
        Err(Unimplemented)
    }

    fn w16(&mut self, offset: u32, val: u16) -> MemResult<()> {
        {trace!(target: "FLS", "w16 offset:{:x} val:0x{:x}", offset, val);}
        // Simplified CFI state machine
        match (offset, val & 0xFF, self.state) {
            (0xAAAA, 0xAA, CFIState::ReadArrayMode) => {
                self.state = CFIState::CommandPreambleAA;
                Ok(())
            },
            (0x0000, 0xFF, _) => {
                // See 'A1.2 CFI Query Flowchart' from Intel AP-646
                self.state = CFIState::ReadArrayMode;
                Ok(())
            }
            (0x5554, 0x55, CFIState::CommandPreambleAA) => {
                self.state = CFIState::CommandPreamble55;
                Ok(())
            }
            (0xAAAA, 0x90, CFIState::CommandPreamble55) => {
                self.state = CFIState::ReadSoftwareID;
                Ok(())
            }
            (0x0000, 0xF0, CFIState::ReadSoftwareID) => {
                self.state = CFIState::ReadArrayMode;
                Ok(())
            }
            (_, 0x90, _) => { // read id
                self.state = CFIState::ReadSoftwareID;
                Ok(())
            }
            (_, 0x70, _) => { // read status
                self.state = CFIState::ReadStatus;
                Ok(())
            }
            (_, 0xff, _) => { // reset
                self.state = CFIState::ReadArrayMode;
                Ok(())
            }
            _ => Err(Unimplemented),
        }
    }

    fn w32(&mut self, _offset: u32, _val: u32) -> MemResult<()> {
        {trace!(target: "FLS", "w32 offset:{:x} val:0x{:x}", _offset, _val);}
        Err(Unimplemented)
    }
}
