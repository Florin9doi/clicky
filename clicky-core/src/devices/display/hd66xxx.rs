use crate::devices::prelude::*;

use std::sync::{Arc, RwLock};

use crate::devices::display::LcdPanel;
use crate::gui::RenderCallback;

const MAX_WIDTH: usize = 220;
const MAX_HEIGHT: usize = 176;
const FB_LEN: usize = MAX_WIDTH * MAX_HEIGHT;

#[derive(Debug, Default, Copy, Clone)]
struct InternalRegs {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    cur_x: usize,
    cur_y: usize,
}

// The unknown LCD controller used on iPod Photo
pub struct Hd66xxx {
    /// Index/command register
    ir: u16,
    /// Graphics RAM
    gram: Arc<RwLock<[u16; FB_LEN]>>,

    ireg: Arc<RwLock<InternalRegs>>,
}

impl std::fmt::Debug for Hd66xxx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hd66xxx")
            .field("ir", &self.ir)
            .field("gram", &"[...]")
            .field("ireg", &self.ireg)
            .finish()
    }
}

impl Hd66xxx {
    pub fn new() -> Hd66xxx {
        Hd66xxx {
            ir: 0,
            gram: Arc::new(RwLock::new([0; FB_LEN])),
            ireg: Arc::new(RwLock::new(InternalRegs {
                w: MAX_WIDTH,
                h: MAX_HEIGHT,
                ..InternalRegs::default()
            })),
        }
    }

    fn make_render_callback(&self) -> RenderCallback {
        let gram = Arc::clone(&self.gram);

        Box::new(move |buf: &mut Vec<u32>| -> (usize, usize) {
            let gram = *gram.read().unwrap();

            let new_buf = gram.iter().map(|&px| {
                let p = px.rotate_left(8);
                0xff000000
                    | (((p & 0xf800) as u32) << 8)
                    | (((p & 0x07e0) as u32) << 5)
                    | (((p & 0x001f) as u32) << 3)
            });

            buf.splice(.., new_buf);
            (MAX_WIDTH, MAX_HEIGHT)
        })
    }

    fn handle_data_write(&mut self, val: u16) -> MemResult<()> {
        let mut ireg = self.ireg.write().unwrap();
        if ireg.cur_y < ireg.h {
            let px_x = ireg.x + ireg.cur_x;
            let px_y = ireg.y + ireg.cur_y;
            let idx = px_y * MAX_WIDTH + px_x;

            if idx < FB_LEN {
                self.gram.write().unwrap()[idx] = val;
            }

            ireg.cur_x += 1;
            if ireg.cur_x >= ireg.w {
                ireg.cur_x = 0;
                ireg.cur_y += 1;
            }
        }
        Ok(())
    }

    fn handle_data_read(&mut self) -> MemResult<u16> {
        Ok(0)
    }
}

impl LcdPanel for Hd66xxx {
    fn write_command(&mut self, val: u16) -> MemResult<()> {
        let mut ireg = self.ireg.write().unwrap();
        self.ir = val >> 8;
        let val = val as u8;

        match self.ir {
            0x12 => {
                ireg.x = (val as usize).min(MAX_WIDTH - 1);
                ireg.cur_x = 0;
                ireg.cur_y = 0;
            }
            0x13 => {
                let remaining_width = MAX_WIDTH.saturating_sub(ireg.x).max(1);
                ireg.w = (val as usize + 1).clamp(1, remaining_width); // rockbox
                // ireg.w = (val as usize + 8).clamp(1, remaining_width); // ipod bootrom
                // ireg.w = (val as usize / 2 + 4).clamp(1, remaining_width); // ipod bootrom alt
                ireg.cur_x = 0;
                ireg.cur_y = 0;
            }
            0x15 => {
                let remaining_width = MAX_HEIGHT.saturating_sub(ireg.y).max(1);
                ireg.h = (val as usize + 1).clamp(1, remaining_width);
                ireg.cur_x = 0;
                ireg.cur_y = 0;
            }
            0x16 => {
                ireg.y = (val as usize).min(MAX_HEIGHT - 1);
                ireg.cur_x = 0;
                ireg.cur_y = 0;
            }
            // 0x18 => {
            //     self.gram.write().unwrap().fill(0xaa); // debug
            // }
            0x01 | 0x02 | 0x10 | 0x18 | 0x7e | 0x7f | 0x80 | 0xce | 0xef => {
                warn!(target: "LCD", "Unhandled cmd:{:x} val:{:x}", self.ir, val);
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

    fn read_command(&mut self) -> MemResult<u16> {
        Ok(self.ir)
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
