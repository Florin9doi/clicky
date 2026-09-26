use crate::devices::prelude::*;

use crate::devices::platform::pp::Controls;
use crate::signal::{self, gpio};

#[derive(Debug)]
pub struct ScrollWheel {
    controls: Option<Controls<signal::Slave>>,
    pin1: Option<gpio::Sender>,
    pin2: Option<gpio::Sender>,
    last_val: u8,
    phase: u8,
}

impl ScrollWheel {
    pub fn new() -> ScrollWheel {
        ScrollWheel {
            controls: None,
            pin1: None,
            pin2: None,
            last_val: 0,
            phase: 0,
        }
    }

    pub fn register_controls(
        &mut self,
        controls: Controls<signal::Slave>,
        pin1: gpio::Sender,
        pin2: gpio::Sender,
    ) {
        self.controls = Some(controls);
        self.pin1 = Some(pin1);
        self.pin2 = Some(pin2);
    }

    pub fn on_change(&mut self) {
        let val = match &self.controls {
            Some(controls) => {
                *controls.wheel.1.lock().unwrap()
            }
            None => return,
        };
        let pin1 = match self.pin1.as_mut() {
            Some(pin) => pin,
            None => return,
        };

        let pin2 = match self.pin2.as_mut() {
            Some(pin) => pin,
            None => return,
        };

        const WHEEL_RANGE: i32 = 96;
        const HALF_RANGE: i32 = WHEEL_RANGE / 2;

        let delta = (val as i32 - self.last_val as i32 + HALF_RANGE)
            .rem_euclid(WHEEL_RANGE)
            - HALF_RANGE;

        if delta > 0 {
            self.phase = (self.phase + 1) & 3;
        } else if delta < 0 {
            self.phase = (self.phase + 3) & 3;
        } else {
            return;
        }

        const STATES: [u8; 4] = [0, 1, 3, 2];
        let state = STATES[self.phase as usize];

        self.last_val = val;

        if state & 1 > 0 {
            pin1.set_high();
        } else {
            pin1.set_low();
        }

        if state & 2 > 0 {
            pin2.set_high();
        } else {
            pin2.set_low();
        }
    }
}
