use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub trait DevConDevice: std::fmt::Debug {
    fn reset(&mut self);
    fn reset_requested(&self) -> Arc<AtomicBool>;
}
