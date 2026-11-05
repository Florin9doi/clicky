use crate::devices::prelude::*;

// CCIR 601
#[derive(Debug)]
pub struct TvOut {
    // n/a
}

impl TvOut {
    pub fn new() -> TvOut {
        TvOut {}
    }
}

impl Device for TvOut {
    fn kind(&self) -> &'static str {
        "TvOut"
    }

    fn probe(&self, offset: u32) -> Probe {
        let reg = match offset {
            0x00 => "Unk",
            0x04 => "Horiz1", // Samples(MSB),HBlank(LSB) / 863/143=PAL, 857/137=NTSC => 720
            0x08 => "Horiz2", // Samples(MSB),??(LSB) / 863/1=PAL, 857/1=NTSC
            0x0c => "Horiz3", // ??(MSB),HBlank(LSB) / 141/143=PAL, 135/137=NTSC
            0x10 => "Unk",
            0x14 => "Vert1", // LinesPerField(MSB),VBlank(LSB) / 309/21=PAL, 258/18=NTSC => 480
            0x18 => "Vert2", // 312/1=PAL, 262/1=NTSC (MSB)
            0x1c => "Vert3", // LinesPerField(MSB),VBlank(LSB) / 309/21=PAL, 258/18=NTSC
            0x20 => "Unk",   // 16/16=PAL, 10/10=NTSC (MSB)
            0x24 => "Unk",
            0x28 => "FB1 Addr",
            0x2c => "FB2 Addr",
            _ => return Probe::Unmapped,
        };

        Probe::Register(reg)
    }
}

impl Memory for TvOut {
    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        match offset {
            0x00..=0x2c => Err(StubRead(Error, 0)),
            _ => Err(Unexpected),
        }
    }

    fn w32(&mut self, offset: u32, _val: u32) -> MemResult<()> {
        match offset {
            0x00..=0x2c => Err(StubWrite(Error, ())),
            _ => Err(Unexpected)
        }
    }
}
