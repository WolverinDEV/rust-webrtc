#![feature(seek_convenience)]
#![feature(trait_alias)]
#![feature(btree_drain_filter)]

use crate::srtp2::srtp2_global_init;

pub mod rtc;
pub mod media;
pub mod transport;
pub mod sctp;
pub mod srtp2;
pub mod utils;
pub mod application;

pub fn initialize_webrtc() {
    // unsafe { libnice::sys::nice_debug_enable(1); }
    srtp2_global_init().expect("srtp2 init failed");
    openssl::init();
}

pub fn test() {
    let mut x = Box::new(2);
    let mut px = &mut x;
    *px = Box::new(**px);
}
