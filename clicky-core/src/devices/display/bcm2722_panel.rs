use crate::devices::prelude::*;
use crate::gui::RenderCallback;
use crate::devices::display::LcdPanel;

use std::sync::{Arc, RwLock};

const MAX_WIDTH: usize = 320;
const MAX_HEIGHT: usize = 240;
const GRAM_LEN: usize = MAX_WIDTH * MAX_HEIGHT;

#[derive(Debug, Default, Copy, Clone)]
struct InternalRegs {
    cmd: u32,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,

    cur_x: usize,
    cur_y: usize,
}

pub struct Bcm2722Panel {
    gram: Arc<RwLock<[u16; GRAM_LEN]>>,
    ireg: Arc<RwLock<InternalRegs>>,
}

impl std::fmt::Debug for Bcm2722Panel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bcm2722Panel")
            .field("gram", &"[...]")
            .field("ireg", &self.ireg)
            .finish()
    }
}

impl Bcm2722Panel {
    pub fn new() -> Bcm2722Panel {
        Bcm2722Panel {
            gram: Arc::new(RwLock::new([0x2e; GRAM_LEN])),
            ireg: Arc::new(RwLock::new(InternalRegs {
                x_end: MAX_WIDTH - 1,
                y_end: MAX_HEIGHT - 1,
                ..InternalRegs::default()
            })),
        }
    }

    fn width(ireg: &InternalRegs) -> usize {
        ireg.x_start.abs_diff(ireg.x_end) + 1
    }

    fn height(ireg: &InternalRegs) -> usize {
        ireg.y_start.abs_diff(ireg.y_end) + 1
    }

    fn reset_cursor(ireg: &mut InternalRegs) {
        ireg.cur_x = 0;
        ireg.cur_y = 0;
    }

    fn cursor_position(ireg: &InternalRegs) -> Option<(usize, usize)> {
        let width = Self::width(ireg);
        let height = Self::height(ireg);

        if ireg.cur_x >= width || ireg.cur_y >= height {
            return None;
        }

        let x = if ireg.x_start <= ireg.x_end {
            ireg.x_start + ireg.cur_x
        } else {
            ireg.x_start - ireg.cur_x
        };

        let y = if ireg.y_start <= ireg.y_end {
            ireg.y_start + ireg.cur_y
        } else {
            ireg.y_start - ireg.cur_y
        };

        Some((x, y))
    }

    fn advance(ireg: &mut InternalRegs) {
        let width = Self::width(ireg);
        let height = Self::height(ireg);
        ireg.cur_x += 2;
        if ireg.cur_x >= width {
            ireg.cur_x = 0;
            ireg.cur_y += 1;
        }
        if ireg.cur_y >= height {
            Self::reset_cursor(ireg);
        }
    }

    fn write_ram(&mut self, val: u32) {
        let mut ireg = self.ireg.write().unwrap();
        let Some((x, y)) = Self::cursor_position(&ireg) else {
            return;
        };
        if x < MAX_WIDTH && y < MAX_HEIGHT {
            let idx = y * MAX_WIDTH + x;
            self.gram.write().unwrap()[idx + 0] = val as u16;
            self.gram.write().unwrap()[idx + 1] = (val >> 16) as u16;
        }
        Self::advance(&mut ireg);
    }

    pub fn make_render_callback(&self) -> RenderCallback {
        let gram = Arc::clone(&self.gram);
        Box::new(move |buf: &mut Vec<u32>| -> (usize, usize) {
            let gram = *gram.read().unwrap();
            let new_buf = gram.iter().map(|&px| {
                let p = px;
                0xff000000
                    | (((p & 0xf800) as u32) << 8)
                    | (((p & 0x07e0) as u32) << 5)
                    | (((p & 0x001f) as u32) << 3)
            });

            buf.splice(.., new_buf);
            (MAX_WIDTH, MAX_HEIGHT)
        })
    }
}

impl LcdPanel for Bcm2722Panel {

    fn read_data32(&mut self) -> MemResult<u32> {
        Ok(1)
    }
    fn read_command32(&mut self) -> MemResult<u32> {
        Ok(1)
    }

    fn write_command32(&mut self, val: u32) -> MemResult<()> {
        let mut ireg = self.ireg.write().unwrap();
        Ok(ireg.cmd = val)
    }

    fn write_data32(&mut self, val: u32) -> MemResult<()> {
        let mut ireg = self.ireg.write().unwrap();
        let index = (ireg.cmd >> 2) - 0x3_8000;

        // match ireg.cmd {
        //     0xe0000 => {}
        //     0x62 => {}
        //     _ => {trace!(target: "LCD", "write_data ireg:{:x}/{:x} val:0x{:x}({})", ireg.cmd, index, val, val);}
        // }
        // match index {
        //     8 ..= 76800 => {}
        //     0..=7 | _ => {trace!(target: "LCD", "write_data cmd:{:x}/{:x} val:0x{:x}({})", ireg.cmd, index, val, val);}
        // }
        match index {
            1 => {
                ireg.x_start = val as usize;
                ireg.cur_x = 0;
            }
            2 => {
                ireg.y_start = val as usize;
                ireg.cur_y = 0;
            }
            3 => ireg.x_end = val as usize,
            4 => ireg.y_end = val as usize,
            0 | 8 ..= 153608 => { // 8 .. (8 + 320x240x2)
                drop(ireg);
                self.write_ram(val);
            }
            _ => {}
        }
        Ok(())
    }

    fn render_callback(&self) -> RenderCallback {
        self.make_render_callback()
    }
}
