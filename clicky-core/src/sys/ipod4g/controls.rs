use super::{Ipod4gControls, System};

use std::collections::HashMap;
use std::str::FromStr;

use crate::gui::{ButtonCallback, ScrollCallback, TakeControls};
use crate::signal::{self, gpio};

#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub enum Ipod4gKey {
    Up,
    Down,
    Left,
    Right,
    Action,
    Hold,
}

pub(super) fn key_label(key: Ipod4gKey) -> &'static str {
    match key {
        Ipod4gKey::Up => "Up",
        Ipod4gKey::Down => "Down",
        Ipod4gKey::Left => "Left",
        Ipod4gKey::Right => "Right",
        Ipod4gKey::Action => "Action",
        Ipod4gKey::Hold => "Hold",
    }
}

#[derive(Debug, Clone)]
pub(super) enum KeySink {
    ClickWheel(signal::Master),
    Gpio(gpio::Sender, bool, bool),
}

impl KeySink {
    pub(super) fn is_sticky(&self) -> bool {
        matches!(self, KeySink::Gpio(_, true, _))
    }

    pub(super) fn set(&mut self, pressed: bool) {
        match self {
            KeySink::ClickWheel(signal) => {
                if pressed {
                    signal.assert()
                } else {
                    signal.clear()
                }
            }
            KeySink::Gpio(sender, sticky, active) => {
                if *sticky {
                    // toggle on and off
                    if pressed {
                        match sender.is_set_high() {
                            false => sender.set_high(),
                            true => sender.set_low(),
                        }
                    }
                } else {
                    if pressed ^ *active {
                        sender.set_high()
                    } else {
                        sender.set_low()
                    }
                }
            }
        }
    }
}

#[derive(Default)]
pub struct Ipod4gBinds {
    pub keys: HashMap<Ipod4gKey, ButtonCallback>,
    pub wheel: Option<ScrollCallback>,
}

impl TakeControls for System {
    type Controls = Ipod4gBinds;

    fn take_controls(&mut self) -> Option<Ipod4gBinds> {
        let Ipod4gControls {
            controls: devices_controls,
            keys: mut key_sinks,
        } = self.controls.take()?;

        let mut controls = Ipod4gBinds::default();

        for key in [
            Ipod4gKey::Up,
            Ipod4gKey::Down,
            Ipod4gKey::Left,
            Ipod4gKey::Right,
            Ipod4gKey::Action,
            Ipod4gKey::Hold,
        ] {
            if let Some(mut sink) = key_sinks.remove(&key) {
                controls.keys.insert(
                    key,
                    Box::new(move |pressed| {
                        sink.set(!pressed)
                    })
                );
            }
        }

        // TODO: make sensitivity adjustable based on user's scroll speed
        let (mut wheel_active, wheel_data) = devices_controls.wheel;
        controls.wheel = Some({
            Box::new(move |(_dx, dy)| {
                // HACK: the signal is edge-triggered
                // TODO: i really aught to rework how input works...
                if wheel_active.is_asserting() {
                    wheel_active.clear();
                } else {
                    wheel_active.assert();
                }

                let mut wheel_data = wheel_data.lock().unwrap();
                // from rockbox button-clickwheel.c
                // #define WHEELCLICKS_PER_ROTATION     96 /* wheelclicks per full rotation */
                *wheel_data = (*wheel_data as i32 + (-dy * 2.0) as i32).rem_euclid(96) as u8;
            })
        });

        Some(controls)
    }
}

impl FromStr for Ipod4gKey {
    type Err = String;

    fn from_str(s: &str) -> Result<Ipod4gKey, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "up" => Ok(Ipod4gKey::Up),
            "down" => Ok(Ipod4gKey::Down),
            "left" => Ok(Ipod4gKey::Left),
            "right" => Ok(Ipod4gKey::Right),
            "action" => Ok(Ipod4gKey::Action),
            "hold" => Ok(Ipod4gKey::Hold),
            _ => Err(format!(
                "no such key: {:?} (expected one of: up, down, left, right, action, hold)",
                s
            )),
        }
    }
}
