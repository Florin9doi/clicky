use crate::devices::prelude::*;
use crate::gui::RenderCallback;
use crate::devices::display::LcdPanel;

/*
The first layer (L1) is to combine 2x16b values (and possibly 8b values, TBD) into 32b values.
 - Only the data/raddr/waddr values are split (32b long).
 - The handshake/status/ready are handled in a single 16b or 32b request (16b long).
The second layer is a transaction, like:
 - read handshake
 - write raddr / write waddr (2x16/1x32)
 - read status until status is OK
 - read data / write data (2x16/1x32)
The L2 goal, ignoring the noise, is to read/write data such:
 - write 0x0 - write firmware
 - write 0x1f8 - set command
 - read 0x1000_2824 - read LCD ID
 - write 0xe0000 - set cmd args / LCD data
 - write 0xc020_0000 - write tv-out data
 The third layer combines multiple L2 operations:
 - set command id (0x1f8) to 0 (update full frame), then send data corresponding to cmd0 to 0xe0000
 - set command id (0x1f8) to 5 (update window), then send args/data corresponding to cmd5 to 0xe0000
   - These parameters are not actual LCD controller commands; those are probably generated internally by the BCM firmware.
   - Instead, they provide an abstraction layer between the PP5022 and BCM2722, regardless of the connected display.
*/

const DATA: u32 = 0x00000;
const WRITE_ADDR: u32 = 0x10000;
const READ_ADDR: u32 = 0x20000;
const STATUS: u32 = 0x30000;
const READY: u32 = 0x60000;
const HANDSHAKE: u32 = 0x70000;

#[derive(Debug)]
pub struct Bcm2722 {
    write_addr_latch: Option<u16>,
    read_latch: Option<u16>,
    raddr: u32,
    waddr: u32,
    cmd: u32,
    cmd05_index: u32,
    panel: Box<dyn LcdPanel>,
}

impl Bcm2722 {
    pub fn new(panel: Box<dyn LcdPanel>) -> Bcm2722 {
        Bcm2722 {
            write_addr_latch: None,
            read_latch: None,
            raddr: 0,
            waddr: 0,
            cmd: 0,
            cmd05_index: 0,
            panel,
        }
    }

    pub fn render_callback(&self) -> RenderCallback {
        self.panel.render_callback()
    }

    fn read_mem(&mut self, offset: u32) -> MemResult<u32> {
        let val = match offset {
            DATA => {
                match self.raddr {
                    0x1f8 => 1,
                    0x1fc => 0,
                    0x1000_0c00 => 1,
                    0x1000_2800 => 0,
                    0x1000_2804 => 0, // diag mode - tv out
                    0x1000_280c => 0,
                    0x1000_2824 => 0x0004_0000, // LCD ID
                                // 0x000c_0000 -> LCD ID = 1 (SHP)
                                // 0x0008_0000 -> LCD ID = 2 (TSB)
                                // 0x0004_0000 -> LCD ID = 3 (SAM)
                                // 0x0000_0000 -> LCD ID = 4 (ERR)
                    _ => {
                        {debug!(target: "LCD", "      read_mem : ({}) addr:{:x} val:{:x}", self.probe(offset), self.raddr, 0xdead_deadu32);}
                        0xdead_dead
                    }
                }
            }
            STATUS => 0x0013,
            WRITE_ADDR | READ_ADDR | 0x40000 | 0x50000 | READY => 0x0001,
            HANDSHAKE => 0x0040,
            _ => 0x0000,
        };
        // if offset != STATUS {debug!(target: "LCD", "    read_mem : ({}) addr:{:x} val:{:x}", self.probe(offset), offset, val);}
        Ok(val)
    }

    fn write_mem(&mut self, offset: u32, val: u32) -> MemResult<()> {
        // if offset != 0
        // {debug!(target: "LCD", "    write_mem : addr:{:x} val:0x{:x}({})", offset, val, val);}
        match offset {
            WRITE_ADDR => {
                self.waddr = val;
                Err(StubWrite(Trace, ()))
            }
            READ_ADDR => {
                self.raddr = val;
                Err(StubWrite(Trace, ()))
            }
            DATA | 0x40000 | 4 => {
                // if self.waddr & 0x000e_000f == 0x000e_0002
                // {debug!(target: "LCD", "    write_mem : addr:{:x} val:0x{:x}({})", self.waddr, val, val);}
                match self.waddr {
                    0x0000 => { // load firmware
                        // diag mode -> 0x16000 bytes from NOR 0xa1f44-0xb7f43
                        // let _ = Self::append_firmware(val);
                        Ok(())
                    }
                    0x01f8 => { // cmd
                        self.cmd = val & 0xffff;
                        match self.cmd {
                            0x00 => { // full frame
                                // these are not real lcd commands but we're not running the bcm firmware
                                let _ = self.panel.write_command32(1);
                                let _ = self.panel.write_data32(0);
                                let _ = self.panel.write_command32(2);
                                let _ = self.panel.write_data32(0);
                                let _ = self.panel.write_command32(3);
                                let _ = self.panel.write_data32(320 - 1);
                                let _ = self.panel.write_command32(4);
                                let _ = self.panel.write_data32(240 - 1);
                                let _ = self.panel.write_command32(self.cmd);
                            }
                            0x05 => { // window
                                self.cmd05_index = 0;
                            }
                            _ => {}
                        }
                        Ok(())
                    }
                    0x000e_0000 ..= 0x001f_0000 => { // lcd data
                        if self.cmd == 0x05 && self.cmd05_index >= 1 && self.cmd05_index <= 4 {
                            let _ = self.panel.write_command32(self.cmd05_index);
                            let _ = self.panel.write_data32(val);
                            self.cmd05_index += 1;
                        }
                        else if self.cmd == 0x05 && self.cmd05_index < 8 {
                            let _ = self.panel.write_command32(self.cmd);
                            // ignore data
                            self.cmd05_index += 1;
                        } else {
                            let _ = self.panel.write_data32(val);
                        }
                        Ok(())
                    }
                    0xc020_0000 ..= 0xc02f_0000 => { // diag mode - tv out
                        // NTSC -> BMP file incl the header, 720 x 480 x 24 BPP
                        // PAL  -> BMP file incl the header, 720 x 576 x 24 BPP
                        // let _ = Self::append_tvout(val);
                        Ok(())
                    }
                    _ => {
                        {debug!(target: "LCD", "    write_mem : addr:{:x} val:0x{:x}({})", self.waddr, val, val);}
                        Ok(())
                    }
                }
            }
            _ => Err(StubWrite(Error, ())),
        }
    }
}

impl Device for Bcm2722 {
    fn kind(&self) -> &'static str {
        "BCM2722 Bridge"
    }

    fn probe(&self, offset: u32) -> Probe {
        let reg = match offset {
            DATA => "Data",
            WRITE_ADDR => "Write Addr",
            READ_ADDR => "Read Addr",
            STATUS => "Status",
            READY => "Ready",
            HANDSHAKE => "Handshake",
            _ => return Probe::Unmapped,
        };

        Probe::Register(reg)
    }
}

impl Memory for Bcm2722 {
    fn w8(&mut self, offset: u32, val: u8) -> MemResult<()> { // used with dma by retailos
        let val = match self.write_addr_latch.take() {
            None => {
                self.write_addr_latch = Some(val as u16);
                return Ok(());
            }
            Some(lo) => (lo as u32) | (val as u32) << 8,
        };
        // {debug!(target: "LCD", "w8 offset:{:x} val:0x{:x}({})", offset, val, val);}
        let _ = offset;
        let _ = self.panel.write_command32(0x000e_0000);
        self.panel.write_data32(val as u32)
        // self.w16(offset, val)
        // self.w16(DATA, val as u16)
        // Ok(())
    }

    fn r16(&mut self, offset: u32) -> MemResult<u16> {
        // {debug!(target: "LCD", "r16 ({}) offset:{:x}", self.probe(offset), offset);}
        if let Some(val) = self.read_latch.take() {
            return Ok(val);
        }

        if offset == DATA  {
            let val = self.read_mem(offset)?;
            self.read_latch = Some((val >> 16) as u16);
            return Ok(val as u16);
        }
        let val = self.read_mem(offset)?;
        Ok(val as u16)
    }

    fn w16(&mut self, offset: u32, val: u16) -> MemResult<()> {
        // {debug!(target: "LCD", "w16 ({}) offset:{:x} val:0x{:x}({})", self.probe(offset), offset, val, val);}
        match offset {
            DATA | WRITE_ADDR | READ_ADDR => {
                let val = match self.write_addr_latch.take() {
                    None => {
                        self.write_addr_latch = Some(val);
                        return Ok(());
                    }
                    Some(lo) => (lo as u32) | ((val as u32) << 16),
                };

                self.write_mem(offset, val)
            }
            _ => Ok(()),
        }
    }

    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        // {debug!(target: "LCD", "r32 ({}) offset:{:x}", self.probe(offset), offset);}
        self.read_mem(offset)
    }

    fn w32(&mut self, offset: u32, val: u32) -> MemResult<()> {
        // {debug!(target: "LCD", "w32 ({}) offset:{:x} val:0x{:x}({})", self.probe(offset), offset, val, val);}
        self.write_mem(offset, val)
    }
}
