#![feature(seek_convenience)]
#![feature(trait_alias)]
#![feature(btree_drain_filter)]
#![feature(map_first_last)]

/* for std::intrinsics::breakpoint() */
#![feature(core_intrinsics)]
#![recursion_limit = "128"]

use crate::srtp2::srtp2_global_init;
use std::sync::{RwLock};
use std::ops::Deref;
use lazy_static::lazy_static;

pub mod rtc;
pub mod media;
pub mod transport;
pub mod sctp;
pub mod srtp2;
pub mod utils;
pub mod application;

lazy_static! {
    static ref GLOBAL_LOGGER: RwLock<Option<slog::Logger>> = RwLock::new(None);
}

pub fn initialize_webrtc(global_logger: slog::Logger) {
    // unsafe { libnice::sys::nice_debug_enable(1); }
    *GLOBAL_LOGGER.write().unwrap() = Some(global_logger);
    srtp2_global_init().expect("srtp2 init failed");
    openssl::init();
}

pub(crate) fn global_logger() -> slog::Logger {
    if let Ok(locked) = GLOBAL_LOGGER.read() {
        if let Some(logger) = locked.deref() {
            logger.clone()
        } else {
            /* Fixme: Use fallback */
            panic!("missing global logger")
        }
    } else {
        /* FIXME: Unpoison */
        panic!("global logger has been poisioned")
    }
}