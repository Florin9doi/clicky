use crate::devices::prelude::*;

use std::sync::{Arc, RwLock};

use crate::devices::display::LcdPanel;
use crate::gui::RenderCallback;

const GRAM_WIDTH: usize = 320;
const GRAM_HEIGHT: usize = 240;
const GRAM_LEN: usize = GRAM_WIDTH * GRAM_HEIGHT;

#[derive(Debug, Default, Copy, Clone)]
struct InternalRegs {
    // Driver Output Control (R01)
    ss: bool,
    sm: bool,
    // Entry Mode (R03)
    id: u8, // 2 bits, address-increment direction (AM / I/D in HD66753 terms)
    am: bool, // horizontal vs vertical auto-increment
    bgr: bool,
    // Display Control (R07)
    d: u8,  // 2 bits
    ptr: bool,
    gon: bool,
    dte: bool,
    cl: bool,
    // Power Control (R10)
    slp: bool,
    stb: bool,
    // Horizontal/Vertical RAM Address Position (R44/R45/R46)
    hsa: u16,
    hea: u16,
    vsa: u16,
    vea: u16,
    // GRAM Address Set (R20/R21)
    ax: u16,
    ay: u16,
}

/// Renesas Hd66789 320x240 color LCD Controller.
pub struct Hd66789 {
    /// Index Register
    ir: u16,
    /// Graphics RAM
    gram: Arc<RwLock<[u16; GRAM_LEN]>>,

    ireg: Arc<RwLock<InternalRegs>>,
}

impl std::fmt::Debug for Hd66789 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hd66789")
            .field("ir", &self.ir)
            .field("gram", &"[...]")
            .field("ireg", &self.ireg)
            .finish()
    }
}

impl Hd66789 {
    pub fn new() -> Hd66789 {
        let gram = Arc::new(RwLock::new([0; GRAM_LEN]));
        let ireg = Arc::new(RwLock::new(InternalRegs {
            hea: (GRAM_WIDTH - 1) as u16,
            vea: (GRAM_HEIGHT - 1) as u16,
            ..InternalRegs::default()
        }));

        Hd66789 {
            ir: 0,
            gram,
            ireg,
        }
    }

    // Convert the current (ax, ay) window-relative address into a flat GRAM
    // index, rotated 180deg so the render callback can read GRAM directly
    // without any reverse step.
    fn gram_idx(ireg: &InternalRegs) -> usize {
        let x = ireg.ax as usize;
        let y = ireg.ay as usize;

        let phys_x = GRAM_WIDTH - 1 - x.min(GRAM_WIDTH - 1) - (GRAM_WIDTH - 176 - 1);
        let phys_y = GRAM_HEIGHT - 1 - y.min(GRAM_HEIGHT - 1) - (GRAM_HEIGHT - 132 - 1);

        (phys_y * GRAM_WIDTH) + phys_x
    }

    fn advance_ac(ireg: &mut InternalRegs) {
        let step: i32 = if ireg.id.get_bit(0) { 1 } else { -1 };

        if !ireg.am {
            // horizontal-first traversal
            let mut x = ireg.ax as i32 + step;
            if x > ireg.hea as i32 || x < ireg.hsa as i32 {
                x = if step > 0 { ireg.hsa as i32 } else { ireg.hea as i32 };
                let mut y = ireg.ay as i32 + if ireg.id.get_bit(1) { 1 } else { -1 };
                if y > ireg.vea as i32 || y < ireg.vsa as i32 {
                    y = if ireg.id.get_bit(1) { ireg.vsa as i32 } else { ireg.vea as i32 };
                }
                ireg.ay = y as u16;
            }
            ireg.ax = x as u16;
        } else {
            // vertical-first traversal
            let mut y = ireg.ay as i32 + step;
            if y > ireg.vea as i32 || y < ireg.vsa as i32 {
                y = if step > 0 { ireg.vsa as i32 } else { ireg.vea as i32 };
                let mut x = ireg.ax as i32 + if ireg.id.get_bit(1) { 1 } else { -1 };
                if x > ireg.hea as i32 || x < ireg.hsa as i32 {
                    x = if ireg.id.get_bit(1) { ireg.hsa as i32 } else { ireg.hea as i32 };
                }
                ireg.ax = x as u16;
            }
            ireg.ay = y as u16;
        }
    }

    /// Returns a callback to update the framebuffer.
    ///
    /// The callback accepts a minifb framebuffer, and returns the rendered
    /// dimensions.
    fn make_render_callback(&self) -> RenderCallback {
        let gram = Arc::clone(&self.gram);
        let ireg = Arc::clone(&self.ireg);

        Box::new(move |buf: &mut Vec<u32>| -> (usize, usize) {
            let gram = *gram.read().unwrap();
            let ireg = *ireg.read().unwrap();

            let new_buf: Vec<u32> = gram.iter().map(|&px| {
                let p = px.rotate_left(8);
                if !ireg.d.get_bit(0) || !ireg.gon || !ireg.dte {
                    0xff123456u32
                } else {
                    0xff000000
                        | (((p & 0xf800) as u32) << 8)
                        | (((p & 0x07e0) as u32) << 5)
                        | (((p & 0x001f) as u32) << 3)
                }
            }).collect();

            buf.splice(.., new_buf);
            (GRAM_WIDTH, GRAM_HEIGHT)
        })
    }

    fn handle_data_write(&mut self, val: u16) -> MemResult<()> {
        let mut ireg = self.ireg.write().unwrap();

        match self.ir {
            // Driver Output Control
            0x01 => {
                ireg.ss = val.get_bit(8);
                ireg.sm = val.get_bit(10);
            }
            // Entry Mode
            0x03 => {
                ireg.id = val.get_bits(4..=5) as u8;
                ireg.am = val.get_bit(3);
                ireg.bgr = val.get_bit(12);
            }
            // Display Control
            0x07 => {
                ireg.d = val.get_bits(0..=1) as u8;
                ireg.cl = val.get_bit(3);
                ireg.dte = val.get_bit(4);
                ireg.gon = val.get_bit(5);
                ireg.ptr = val.get_bit(12);
            }
            // Power Control
            0x10 => {
                ireg.stb = val.get_bit(0);
                ireg.slp = val.get_bit(1);
            }
            // GRAM Address Set (horizontal)
            0x20 => ireg.ax = val.get_bits(0..=7),
            // GRAM Address Set (vertical)
            0x21 => ireg.ay = val,
            // Write Data to GRAM
            0x22 => {
                let mut gram = self.gram.write().unwrap();
                let idx = Hd66789::gram_idx(&ireg);
                if idx < GRAM_LEN {
                    gram[idx] = val;
                }
                Hd66789::advance_ac(&mut ireg);
            }
            // Horizontal RAM Address Position (start..end)
            0x44 => {
                ireg.hsa = val.get_bits(0..=7);
                ireg.hea = val.get_bits(8..=15);
            }
            // Vertical RAM Address Position (start..end)
            0x45 => {
                ireg.vsa = val.get_bits(0..=7);
                ireg.vea = val.get_bits(8..=15);
            }
            invalid_cmd => {
                return Err(Fatal(format!(
                    "attempted to execute invalid LCD command {:#x?}",
                    invalid_cmd
                )))
            }
        }

        Ok(())
    }

    fn handle_data_read(&mut self) -> MemResult<u16> {
        match self.ir {
            0x00 => Ok(0x0789),
            0x22 => {
                // Read Data from GRAM, auto-increments like a write does
                let mut ireg = self.ireg.write().unwrap();
                let gram = self.gram.read().unwrap();
                let idx = Hd66789::gram_idx(&ireg);
                let val = if idx < GRAM_LEN { gram[idx] } else { 0 };
                Hd66789::advance_ac(&mut ireg);
                Ok(val)
            }
            invalid_cmd => Err(Fatal(format!(
                "attempted to execute invalid LCD command {:#x?}",
                invalid_cmd
            ))),
        }
    }
}

impl LcdPanel for Hd66789 {
    fn write_command(&mut self, val: u16) -> MemResult<()> {
        self.ir = val;

        if self.ir > 0x46 {
            return Err(ContractViolation {
                msg: format!("set invalid LCD Command: {:#04x?}", val),
                severity: Error,
                stub_val: None,
            });
        }

        Ok(())
    }

    fn read_command(&mut self) -> MemResult<u16> {
        // XXX: not currently tracking driving raster-row position
        Ok(0)
    }

    fn write_data(&mut self, val: u16) -> MemResult<()> {
        self.handle_data_write(val)
    }

    fn read_data(&mut self) -> MemResult<u16> {
        self.handle_data_read()
    }

    fn render_callback(&self) -> RenderCallback {
        self.make_render_callback()
    }
}
