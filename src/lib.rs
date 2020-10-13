#![feature(seek_convenience)]

use crate::srtp2::srtp2_global_init;

pub mod rtc;
pub mod media;
pub mod ice;
pub mod sctp;
pub mod srtp2;
pub mod utils;

pub fn initialize_webrtc() {
    // unsafe { libnice::sys::nice_debug_enable(1); }
    srtp2_global_init().expect("srtp2 init failed");
    openssl::init();
}