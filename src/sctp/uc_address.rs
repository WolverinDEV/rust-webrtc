use crate::sctp::{SctpRecvInfo, size_t};
use tokio::sync::mpsc;
use std::ffi::c_void;
use std::sync::Arc;
use std::{ptr, panic};
use std::os::raw::c_char;

/// This module contains a wrapper around the SCTP user callback socket address


#[derive(Debug)]
pub enum SctpUserCallbackAddressEvent {
    MessageReceived{ buffer: Vec<u8>, info: SctpRecvInfo, flags: i32 },
    MessageSend{ buffer: Vec<u8> }
}

pub struct SctpUserCallbackAddress {
    socket_id: u32,
    channel: mpsc::UnboundedSender<SctpUserCallbackAddressEvent>
}

impl Drop for SctpUserCallbackAddress {
    fn drop(&mut self) {
        unsafe {
            ffi::usrsctp_deregister_address(self.socket_id as *mut c_void);
        }
    }
}

impl SctpUserCallbackAddress {
    fn new(socket_id: u32, channel: mpsc::UnboundedSender<SctpUserCallbackAddressEvent>) -> Arc<SctpUserCallbackAddress> {
        let socket = SctpUserCallbackAddress {
            socket_id,
            channel
        };

        unsafe {
            ffi::usrsctp_register_address(socket.socket_id as *mut c_void);
        }

        Arc::new(socket)
    }

    /* We've a decoded message */
    fn callback_read(&self, buffer: &mut [u8], info: ffi::sctp_rcvinfo, flags: i32) {
        self.channel.send(SctpUserCallbackAddressEvent::MessageReceived { buffer: Vec::from(buffer), info: SctpRecvInfo::from(&info), flags })
            .unwrap();
    }

    /* We should send an encoded message */
    fn callback_write(&self, buffer: &mut [u8]) {
        self.channel.send(SctpUserCallbackAddressEvent::MessageSend { buffer: Vec::from(buffer) })
            .unwrap();
    }
}

struct SctpInstanceData {
    address_instances: Vec<Arc<SctpUserCallbackAddress>>,
    address_ids: Vec<u32>,
    address_id_index: u32
}

impl SctpInstanceData {
    fn new() -> Self {
        unsafe {
            ffi::usrsctp_init(0, Some(usrsctp_write_callback), None);
        }

        SctpInstanceData {
            address_instances: Vec::with_capacity(8),
            address_ids: Vec::with_capacity(8),
            address_id_index: 0x10F000
        }
    }

    fn create_address(&mut self) -> (Arc<SctpUserCallbackAddress>, mpsc::UnboundedReceiver<SctpUserCallbackAddressEvent>) {
        let socket_id = self.address_id_index;
        self.address_id_index = self.address_id_index.wrapping_add(1);

        let (tx, rx) = mpsc::unbounded_channel::<SctpUserCallbackAddressEvent>();

        let socket = SctpUserCallbackAddress::new(socket_id, tx);
        self.address_instances.push(socket.clone());
        self.address_ids.push(socket_id);

        (socket, rx)
    }

    fn destroy_address(&mut self, socket_id: u32) {
        let position = self.address_ids.iter()
            .position(|id| *id == socket_id);

        if let Some(position) = position {
            self.address_ids.remove(position);
            self.address_instances.remove(position);
        }
    }

    fn find_address(&mut self, socket_id: u32) -> Option<Arc<SctpUserCallbackAddress>> {
        self.address_ids.iter()
            .position(|id| *id == socket_id)
            .map(|pos| self.address_instances[pos].clone())
    }
}

lazy_static! {
    static ref SCTP_INSTANCE: Mutex<SctpInstanceData> = Mutex::new(SctpInstanceData::new());
}

unsafe extern "C" fn usrsctp_write_callback(
    addr: *mut ::std::os::raw::c_void,
    buffer: *mut ::std::os::raw::c_void,
    length: size_t,
    _tos: u8,
    _set_df: u8,
) -> ::std::os::raw::c_int {
    /* FIXME: Unwrap handler! */
    let socket = SCTP_INSTANCE.lock().unwrap().find_address(addr as u32);
    if let Some(socket) = socket {
        let buffer = ptr::slice_from_raw_parts_mut(buffer as *mut u8, length).as_mut().unwrap();
        socket.callback_write(buffer);
    } else {
        panic!("usrsctp_write_callback called with an invalid socket");
    }

    0
}

unsafe extern "C" fn usrsctp_read_callback(
    _sock: *mut ffi::socket,
    _addr: ffi::sctp_sockstore,
    data: *mut ::std::os::raw::c_void,
    datalen: size_t,
    recv_info: ffi::sctp_rcvinfo,
    flags: ::std::os::raw::c_int,
    ulp_info: *mut ::std::os::raw::c_void,
) -> ::std::os::raw::c_int {
    let result = panic::catch_unwind(|| {
        let socket = SCTP_INSTANCE.lock().unwrap().find_address(ulp_info as u32);
        if let Some(socket) = socket {
            if data.is_null() {
                println!("usrsctp_read_callback with nullptr as sata. This should not happen!");
            } else {
                let buffer = ptr::slice_from_raw_parts_mut(data as *mut u8, datalen).as_mut().unwrap();
                socket.callback_read(buffer, recv_info, flags);
                /*
                 * This is a really hacky way to free out buffer!
                 * UsrSctp want's the the callback frees the buffer via ::free.
                 * Since we can't call this method, we're not an C application we've to use something else.
                 * Luckely as it turns out, usrsctp_freedumpbuffer does the job for us (it's just a wrapper around free)
                 */
                ffi::usrsctp_freedumpbuffer(data as *mut c_char);
            }
        } else {
            panic!("usrsctp_read_callback called with an invalid socket");
        }
    });

    if result.is_err() {
        /* TODO: Abort! */
    }
    /* FIXME: Unwrap handler! */


    1
}