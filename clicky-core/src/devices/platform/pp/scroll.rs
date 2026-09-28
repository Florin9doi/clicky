use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;
use crate::devices::platform::pp::Controls;
use crate::signal::{self, gpio};

#[derive(Debug)]
pub struct ScrollWheel {
    controls: Option<Controls<signal::Slave>>,
    pending: Option<Sender<u8>>,
    last_val: u8,
    phase: u8,
}

impl ScrollWheel {
    pub fn new() -> ScrollWheel {
        ScrollWheel {
            controls: None,
            pending: None,
            last_val: 0,
            phase: 0,
        }
    }

    pub fn register_controls(
        &mut self,
        controls: Controls<signal::Slave>,
        mut pin1: gpio::Sender,
        mut pin2: gpio::Sender,
    ) {
        self.controls = Some(controls);
        let (tx, rx) = mpsc::channel::<u8>();
        thread::spawn(move || {
            while let Ok(state) = rx.recv() {
                if state & 1 != 0 {
                    pin1.set_high();
                } else {
                    pin1.set_low();
                }

                if state & 2 != 0 {
                    pin2.set_high();
                } else {
                    pin2.set_low();
                }

                thread::sleep(Duration::from_millis(2));
            }
        });

        self.pending = Some(tx);
    }

    pub fn on_change(&mut self) {
        let val = match &self.controls {
            Some(controls) => {
                *controls.wheel.1.lock().unwrap()
            }
            None => return,
        };

        const WHEEL_RANGE: i32 = 96;
        const HALF_RANGE: i32 = WHEEL_RANGE / 2;
        const STATES: [u8; 4] = [0, 1, 3, 2];

        let delta = (val as i32 - self.last_val as i32 + HALF_RANGE)
            .rem_euclid(WHEEL_RANGE)
            - HALF_RANGE;

        if delta == 0 {
            return;
        }

        self.last_val = val;

        let sender = match &self.pending {
            Some(sender) => sender,
            None => return,
        };

        let steps = delta.unsigned_abs();

        for _ in 0..steps {
            if delta > 0 {
                self.phase = (self.phase + 1) & 3;
            } else {
                self.phase = (self.phase + 3) & 3;
            }

            let state = STATES[self.phase as usize];
            if sender.send(state).is_err() {
                return;
            }
        }
    }
}
