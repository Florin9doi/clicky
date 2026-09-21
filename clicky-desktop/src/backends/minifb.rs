use std::collections::HashMap;
use std::sync::mpsc as chan;

use minifb::{Key, Window, WindowOptions};

use clicky_core::gui::{ButtonCallback, RenderCallback, ScrollCallback};

pub struct MinifbControls {
    pub keymap: HashMap<Key, ButtonCallback>,
    pub on_scroll: Option<ScrollCallback>,
}

#[derive(Debug)]
pub struct MinifbRenderer {}

impl MinifbRenderer {
    /// Render one logical pixel as a physical dot.
    fn render_lcd_pixel(
        scale: usize,
        buffer: &mut [u32],
        buffer_width: usize,
        x: usize,
        y: usize,
        pixel: u32,
    ) {
        let base_x = x * scale;
        let base_y = y * scale;

        let r = ((pixel >> 16) & 0xff) as f32;
        let g = ((pixel >> 8) & 0xff) as f32;
        let b = (pixel & 0xff) as f32;

        for py in 0..scale {
            for px in 0..scale {
                #[allow(dead_code)]
                enum Effect {
                    None,
                    TopRight,
                    AllSides,
                }
                const SELECTED_MODE: Effect = Effect::TopRight;
                let edge = match SELECTED_MODE {
                    Effect::None => false,
                    Effect::TopRight => py == 0 || px == scale - 1,
                    Effect::AllSides => px == 0 || py == 0 || px == scale - 1 || py == scale - 1,
                };

                let factor = if edge {
                    0.8
                } else {
                    1.0
                };

                let r = (r * factor).min(255.0) as u32;
                let g = (g * factor).min(255.0) as u32;
                let b = (b * factor).min(255.0) as u32;

                let dst_x = base_x + px;
                let dst_y = base_y + py;

                let index = dst_y * buffer_width + dst_x;

                if index < buffer.len() {
                    buffer[index] = (r << 16) | (g << 8) | b;
                }
            }
        }
    }

    /// (width, height) crops the framebuffer to the specified screen size
    /// (starting from the top-left corner)
    fn render_lcd(
        output: &mut [u32],
        scale: usize,
        output_width: usize,
        source: &[u32],
        src_width: usize,
        src_height: usize,
        logical_width: usize,
        logical_height: usize,
    ) {
        let width = src_width.min(logical_width);
        let height = src_height.min(logical_height);

        for y in 0..height {
            for x in 0..width {
                let src_index = y * src_width + x;

                if src_index >= source.len() {
                    continue;
                }

                Self::render_lcd_pixel(
                    scale,
                    output,
                    output_width,
                    x,
                    y,
                    source[src_index],
                );
            }
        }
    }

    pub fn run(
        title: &str,
        (width, height): (usize, usize),
        mut update_fb: RenderCallback,
        controls: impl Into<MinifbControls>,
        kill_rx: chan::Receiver<()>,
    ) {
        let mut controls = controls.into();

        let scale = if width > 300 { 2 } else { 4 };
        let scaled_width = width * scale;
        let scaled_height = height * scale;
        let mut buffer = vec![0; scaled_width * scaled_height];
        let mut emu_buffer = Vec::new();

        let mut window = Window::new(
            title,
            scaled_width,
            scaled_height,
            WindowOptions {
                scale: minifb::Scale::X1,
                resize: true,
                ..WindowOptions::default()
            },
        )
        .expect("could not create minifb window");

        // ~60 fps
        window.limit_update_rate(Some(std::time::Duration::from_micros(16600)));

        let mut key_down: HashMap<Key, bool> = HashMap::new();
        'ui_loop: while window.is_open() && kill_rx.try_recv().is_err() {
            if window.is_key_down(Key::Escape) {
                break 'ui_loop;
            }

            for (k, cb) in controls.keymap.iter_mut() {
                let down = window.is_key_down(*k);
                let was_down = key_down.entry(*k).or_insert(false);
                if *was_down != down {
                    *was_down = down;
                    cb(down)
                }
            }

            if let Some(scroll) = window.get_scroll_wheel() {
                if let Some(ref mut on_scroll) = controls.on_scroll {
                    on_scroll(scroll)
                }
            }

            // update the framebuffer
            let (w, h) = update_fb(&mut emu_buffer);

            Self::render_lcd(
                &mut buffer,
                scale,
                scaled_width,
                &emu_buffer,
                w,
                h,
                width,
                height,
            );

            window
                .update_with_buffer(&buffer, scaled_width, scaled_height)
                .expect("could not update minifb window");
        }
    }
}
