use crate::devices::prelude::*;

#[derive(Debug)]
pub struct PpVer5002 {
    version: [u8; 8],
}

impl PpVer5002 {
    const VERSION_SELECTION: usize = 3;
    const VERSIONS: [[u8; 8]; 5] = [
        *b"TANGO-51",
        *b"PP5001E1",
        *b"PP5001E3",
        *b"PP5002E0",
        *b"PP5002C\0",
    ];

    pub fn new() -> PpVer5002 {
        PpVer5002 {
            version: Self::VERSIONS[Self::VERSION_SELECTION],
        }
    }
}

impl Device for PpVer5002 {
    fn kind(&self) -> &'static str {
        "PpVer5002"
    }

    fn probe(&self, offset: u32) -> Probe {
        let reg = match offset {
            0x00 => "Ver1",
            0x04 => "Ver2",
            0x08 => "Ver3",
            0x0c => "Ver4",
            _ => return Probe::Unmapped,
        };

        Probe::Register(reg)
    }
}

impl Memory for PpVer5002 {
    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        match offset {
            0x00 => Err(StubRead(Trace, (self.version[6] as u32) << 8 | self.version[7] as u32 )),
            0x04 => Err(StubRead(Trace, (self.version[4] as u32) << 8 | self.version[5] as u32 )),
            0x08 => Err(StubRead(Trace, (self.version[2] as u32) << 8 | self.version[3] as u32 )),
            0x0c => Err(StubRead(Trace, (self.version[0] as u32) << 8 | self.version[1] as u32 )),
            _ => Err(Unexpected),
        }
    }

    fn w32(&mut self, _offset: u32, _val: u32) -> MemResult<()> {
        Err(InvalidAccess)
    }
}
