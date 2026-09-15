use crate::devices::prelude::*;

use crate::devices::display::LcdPanel;
use crate::gui::RenderCallback;

#[derive(Debug)]
pub enum LcdBridge {
    Mono(MonoLcdBridge),
    Color(ColorLcdBridge),
}

macro_rules! dispatch {
    ($self:ident, $inner:ident, $call:expr) => {
        match $self {
            LcdBridge::Mono($inner) => $call,
            LcdBridge::Color($inner) => $call,
        }
    };
}

impl LcdBridge {
    pub fn new_mono(panel: Box<dyn LcdPanel>) -> LcdBridge {
        LcdBridge::Mono(MonoLcdBridge::new(panel))
    }

    pub fn new_color(panel: Box<dyn LcdPanel>) -> LcdBridge {
        LcdBridge::Color(ColorLcdBridge::new(panel))
    }

    pub fn render_callback(&self) -> RenderCallback {
        dispatch!(self, b, b.render_callback())
    }
}

impl Device for LcdBridge {
    fn kind(&self) -> &'static str {
        dispatch!(self, b, b.kind())
    }

    fn probe(&self, offset: u32) -> Probe {
        dispatch!(self, b, b.probe(offset))
    }
}

impl Memory for LcdBridge {
    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        dispatch!(self, b, b.r32(offset))
    }

    fn w32(&mut self, offset: u32, val: u32) -> MemResult<()> {
        dispatch!(self, b, b.w32(offset, val))
    }
}

/// PP5020 monochrome LCD controller.
///
/// The panel is driven over an 8-bit interface, so each 16-bit transfer takes
/// two accesses. Writes latch the high byte and commit on the second write;
/// reads return the high byte first and latch the low byte for the next read.
#[derive(Debug)]
pub struct MonoLcdBridge {
    // FIXME: not sure if there are separate latches for the command and data
    // registers...
    write_byte_latch: Option<u8>,
    read_byte_latch: Option<u8>,

    panel: Box<dyn LcdPanel>,
}

impl MonoLcdBridge {
    pub fn new(panel: Box<dyn LcdPanel>) -> MonoLcdBridge {
        MonoLcdBridge {
            write_byte_latch: None,
            read_byte_latch: None,
            panel,
        }
    }

    /// Returns a callback to update the framebuffer.
    pub fn render_callback(&self) -> RenderCallback {
        self.panel.render_callback()
    }
}

impl Device for MonoLcdBridge {
    fn kind(&self) -> &'static str {
        "Mono LCD Bridge"
    }

    fn probe(&self, offset: u32) -> Probe {
        let reg = match offset {
            0x0 => "LCD Control",
            0x8 => "LCD Command",
            0x10 => "LCD Data",
            _ => return Probe::Unmapped,
        };

        Probe::Register(reg)
    }
}

impl Memory for MonoLcdBridge {
    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        if offset == 0x0 {
            // bypass the latch
            //
            // Bit 15 is BUSY (iPodLinux: `lcd_busy_mask = 0x8000`), which
            // guests poll before each transfer. HACK: the emulated bridge
            // completes transfers instantly, so it is never busy.
            return Ok(0);
        }

        if let Some(val) = self.read_byte_latch.take() {
            return Ok(val as u32);
        }

        let val: u16 = match offset {
            0x8 => self.panel.read_command()?,
            0x10 => self.panel.read_data()?,
            _ => return Err(Unexpected),
        };

        self.read_byte_latch = Some(val as u8); // latch lower 8 bits
        Ok((val >> 8) as u32) // returning the higher 8 bits first
    }

    fn w32(&mut self, offset: u32, val: u32) -> MemResult<()> {
        if offset == 0x0 {
            // bypass the latch
            return Err(StubWrite(Error, ()));
        }

        // warn!(target: "LCD", "wr offset:{:x} val:{:x}", offset, val);

        // the iPod uses the controller via an 8-bit interface
        let val = val as u8; // FIXME: this should use trunc_to_u8, but it crashes...
        let val = match self.write_byte_latch.take() {
            None => {
                self.write_byte_latch = Some(val);
                // warn!(target: "LCD", "  wr offset:{:x} 2:{:x}", offset, val);
                return Ok(());
            }
            Some(hi) => (hi as u16) << 8 | (val as u16),
        };

        // warn!(target: "LCD", "    wr offset:{:x} 3:{:x}", offset, val);
        // mini2g pp5022 quirk (?)
        if offset == 0x08 && val > 0xff {
            let _ = self.panel.write_command(val >> 8);
            let _ = self.panel.write_data(val & 0xff);
            return Ok(());
        }

        match offset {
            0x8 => self.panel.write_command(val),
            0x10 => self.panel.write_data(val),
            _ => Err(Unexpected),
        }
    }
}

/// PP5020 color LCD controller.
///
/// The panel is driven over an 8-bit interface, so each 16-bit transfer takes
/// two accesses. Writes latch the high byte and commit on the second write;
/// reads return the high byte first and latch the low byte for the next read.
#[derive(Debug)]
pub struct ColorLcdBridge {
    // FIXME: not sure if there are separate latches for the command and data
    // registers...
    write_byte_latch: Option<u32>,
    read_byte_latch: Option<u32>,

    panel: Box<dyn LcdPanel>,
}

impl ColorLcdBridge {
    pub fn new(panel: Box<dyn LcdPanel>) -> ColorLcdBridge {
        ColorLcdBridge {
            write_byte_latch: None,
            read_byte_latch: None,
            panel,
        }
    }

    /// Returns a callback to update the framebuffer.
    pub fn render_callback(&self) -> RenderCallback {
        self.panel.render_callback()
    }
}

impl Device for ColorLcdBridge {
    fn kind(&self) -> &'static str {
        "Color LCD Bridge"
    }

    fn probe(&self, offset: u32) -> Probe {
        let reg = match offset {
            0x00 => "LCD Control",
            0x0c => "Port",
            0x20 => "Control",
            0x24 => "Config",
            0x100 => "Data",
            _ => return Probe::Unmapped,
        };

        Probe::Register(reg)
    }
}

impl Memory for ColorLcdBridge {
    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        if let Some(val) = self.read_byte_latch.take() {
            return Ok(val as u32);
        }

        let val: u32 = match offset {
            // 0x8 => self.panel.read_command()?,
            // 0x10 => self.panel.read_data()?,
            0x0c => 0x0000_0000,
            0x20 => 0x0500_0000,
            _ => return Err(Unexpected),
        };
        return Ok(val as u32);
    }

    fn w32(&mut self, offset: u32, val: u32) -> MemResult<()> {
        if offset == 0x0c {
            match self.write_byte_latch.take() {
                None => {
                    self.write_byte_latch = Some(val);
                    return Ok(());
                }
                Some(hi) => {
                    let cmd16 = ((hi & 0xff) << 8 | (val & 0xff)) as u16;
                    if (val & 0xff00_0000) == 0x8000_0000 {
                        return self.panel.write_command(cmd16);
                    } else {
                        return self.panel.write_data(cmd16);
                    }
                }
            };
        }

        match offset {
            0x00 => Ok(()),
            0x04 => Ok(()),
            0x24 => Ok(()),
            0x20 => Ok(()),
            0x100 => {
                let _ = self.panel.write_data(val as u16);
                let _ = self.panel.write_data((val >> 16) as u16);
                return Ok(());
            },
            _ => Err(Unexpected),
        }
    }
}
