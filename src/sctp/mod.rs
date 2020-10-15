#![allow(dead_code)]

use libusrsctp_sys as ffi;
use std::pin::Pin;
use std::os::raw::{c_void, c_char};
use std::sync::{Mutex, Arc};
use lazy_static::lazy_static;
use libusrsctp_sys::{usrsctp_conninput, usrsctp_close, usrsctp_setsockopt, usrsctp_set_non_blocking, usrsctp_sendv, usrsctp_getsockopt};
use std::io::{Read, Write};
use futures::{StreamExt, Stream};
use futures::task::{Context, Poll};
use futures::io::ErrorKind;
use tokio::sync::mpsc;
use libc;
use crate::sctp::notification::{SctpNotificationType, SctpNotification};
use crate::sctp::sctp_macros::{SCTP_EVENT, IPPROTO_SCTP, SCTP_ALL_ASSOC, SCTP_PEER_ADDR_PARAMS, SCTP_SENDV_SNDINFO, MSG_NOTIFICATION, AF_CONN, SOCK_STREAM, SCTP_ENABLE_STREAM_RESET, SCTP_NODELAY, SCTP_INITMSG, SCTP_STREAM_RESET_OUTGOING, SCTP_RESET_STREAMS, SCTP_DATA_LAST_FRAG};
use std::{ptr, panic};

pub mod notification;
pub mod message;
pub mod sctp_macros;

const SOL_SOCKET: i32 = 1;
const SO_SNDBUF: i32  = 7;
const SO_RCVBUF: i32  = 8;

#[allow(non_camel_case_types)]
type size_t = usize;

#[derive(Debug)]
/// Containing the recv info.
/// This info has been normalized to the host endianess already
pub struct SctpRecvInfo {
    pub rcv_sid: u16,
    pub rcv_ssn: u16,
    pub rcv_flags: u16,
    pub rcv_ppid: u32,
    pub rcv_tsn: u32,
    pub rcv_cumtsn: u32,
    pub rcv_context: u32,
    pub rcv_assoc_id: u32,
}

pub struct SctpRecvInfoFlags {}
impl SctpRecvInfoFlags {
    const LAST_FRAGMENT: u16 = SCTP_DATA_LAST_FRAG;
}

impl SctpRecvInfo {
    fn from(ffi: &ffi::sctp_rcvinfo) -> Self {
        SctpRecvInfo {
            rcv_sid: ffi.rcv_sid,
            rcv_ssn: ffi.rcv_ssn,
            rcv_flags: ffi.rcv_flags,
            rcv_ppid: u32::from_be(ffi.rcv_ppid),
            rcv_tsn: ffi.rcv_tsn,
            rcv_cumtsn: ffi.rcv_cumtsn,
            rcv_context: ffi.rcv_context,
            rcv_assoc_id: ffi.rcv_assoc_id
        }
    }
}

/// See values [here](https://tools.ietf.org/html/rfc6458#section-5.3.2)
pub struct SctpSendInfo {
    pub snd_sid: u16,
    pub snd_flags: u16,
    pub snd_ppid: u32,
    pub snd_context: u32,
    pub snd_assoc_id: u32,
}

impl SctpSendInfo {
    /// Create a new `SctpSendInfo` struct containing zeros only.
    pub fn new() -> Self {
        SctpSendInfo {
            snd_sid: 0,
            snd_flags: 0,
            snd_ppid: 0,
            snd_context: 0,
            snd_assoc_id: 0
        }
    }

    fn to_ffi(&self) -> ffi::sctp_sndinfo {
        let mut result = unsafe { std::mem::zeroed::<ffi::sctp_sndinfo>() };
        result.snd_sid = self.snd_sid;
        result.snd_flags = self.snd_flags;
        result.snd_ppid = self.snd_ppid.to_be();
        result.snd_context = self.snd_context;
        result.snd_assoc_id = self.snd_assoc_id;
        result
    }
}



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
    let result = panic::catch_unwind(|| {
        let socket = SCTP_INSTANCE.lock().unwrap().find_address(addr as u32);
        if let Some(socket) = socket {
            let buffer = ptr::slice_from_raw_parts_mut(buffer as *mut u8, length).as_mut().unwrap();
            socket.callback_write(buffer);
        } else {
            panic!("usrsctp_write_callback called with an invalid socket");
        }
    });

    if result.is_err() {
        eprintln!("usrsctp_write_callback(..) catch_unwind received: {:?}", result.unwrap_err());
        std::process::abort();
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

        eprintln!("usrsctp_read_callback(..) catch_unwind received: {:?}", result.unwrap_err());
        std::process::abort();
    }

    1
}

#[derive(Debug)]
pub enum UsrSctpConnectResult {
    Success,
    BindFailed(i32),
    ConnectFailed(i32)
}

pub struct UsrSCTPSocket {
    socket: *mut ffi::socket,
    target_address_id: u32 /* needed? */
}

impl UsrSCTPSocket {
    fn new(target_address_id: u32) -> Option<Self> {
        let socket = unsafe { ffi::usrsctp_socket(AF_CONN, SOCK_STREAM, IPPROTO_SCTP, Some(usrsctp_read_callback), None, 0, target_address_id as *mut c_void) };
        if socket.is_null() { return None; }
        Some(UsrSCTPSocket {
            socket,
            target_address_id
        })
    }

    /// Returning the underlying user address as mutable void pointer.
    /// Event though we're not mutable, it's fine for our socket and required a lot by usrsctp
    fn address_void_ptr(&self) -> *mut c_void {
        self.target_address_id as *mut c_void
    }

    fn do_connect(&mut self, local_port: u16, remote_port: u16) -> UsrSctpConnectResult {
        unsafe {
            let mut addr: ffi::sockaddr_conn = std::mem::MaybeUninit::zeroed().assume_init();
            addr.sconn_family = AF_CONN as u16;
            addr.sconn_port = local_port.to_be();
            addr.sconn_addr = self.address_void_ptr();

            let result = ffi::usrsctp_bind(self.socket, &mut addr as *mut ffi::sockaddr_conn as *mut libc::sockaddr, std::mem::size_of::<ffi::sockaddr_conn>() as u32);
            if result < 0 {
                UsrSctpConnectResult::BindFailed(result)
            } else {
                addr.sconn_port = remote_port.to_be();

                /* the connect call sadly blocks... */
                let result = ffi::usrsctp_connect(self.socket, &mut addr as *mut ffi::sockaddr_conn as *mut libc::sockaddr, std::mem::size_of::<ffi::sockaddr_conn>() as u32);
                if result < 0 {
                    UsrSctpConnectResult::ConnectFailed(result)
                } else {
                    UsrSctpConnectResult::Success
                }
            }
        }
    }
}

impl Drop for UsrSCTPSocket {
    fn drop(&mut self) {
        unsafe {
            if !self.socket.is_null() {
                usrsctp_close(self.socket);
            }
        }
    }
}

unsafe impl Send for UsrSCTPSocket {}
unsafe impl Sync for UsrSCTPSocket {}

pub enum UsrSctpSessionEvent {
    MessageReceived{ buffer: Vec<u8>, info: SctpRecvInfo },
    EventReceived{ notification: SctpNotification },
}

pub struct UsrSctpSession<T: Read + Write + Unpin> {
    stream: T,
    socket: UsrSCTPSocket,
    socket_channel: mpsc::UnboundedReceiver<SctpUserCallbackAddressEvent>,
    usr_socket_id: u32,

    pub local_port: u16,
    pub remote_port: Option<u16>
}

impl<T: Read + Write + Unpin> UsrSctpSession<T> {
    pub fn new(stream: T, local_port: u16) -> Option<Self> {
        let (socket, rx) = SCTP_INSTANCE.lock().unwrap().create_address();

        let socket = UsrSCTPSocket::new(socket.socket_id);
        if socket.is_none() { return None; }

        let usr_socket_id = socket.as_ref().unwrap().target_address_id;
        Some(UsrSctpSession{
            stream,
            socket: socket.unwrap(),
            socket_channel: rx,
            usr_socket_id,

            local_port,
            remote_port: None
        })
    }

    pub fn connect(&mut self, remote_port: u16) -> UsrSctpConnectResult {
        self.socket.do_connect(self.local_port, remote_port)
    }

    /* TODO: Something like connect async if we want  to use the socket in blocking mode
    pub fn start_connect(&mut self, remote_port: u16) {
        self.remote_port = Some(remote_port);

        let mut connect = UsrSctpConnect::new(self.local_port, remote_port, self.socket.clone());
        connect.start_connect();
        self.connect_future = Some(connect);

        if let Some(waker) = &self.poll_waker {
            waker.wake_by_ref();
        }
    }
    */
    pub fn set_receive_buffer(&mut self, mut size: usize) -> Result<(), std::io::Error> {
        let result = unsafe {
            usrsctp_setsockopt(self.socket.socket, SOL_SOCKET, SO_RCVBUF, &mut size as *mut usize as *mut c_void, std::mem::size_of::<usize>() as u32)
        };
        if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    pub fn set_send_buffer(&mut self, mut size: usize) -> Result<(), std::io::Error> {
        let result = unsafe {
            usrsctp_setsockopt(self.socket.socket, SO_SNDBUF, SO_RCVBUF, &mut size as *mut usize as *mut c_void, std::mem::size_of::<usize>() as u32)
        };
        if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    pub fn toggle_non_block(&mut self, enabled: bool) -> Result<(), std::io::Error> {
        let result = unsafe {
            usrsctp_set_non_blocking(self.socket.socket, if enabled { 1 } else { 0 })
        };
        if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    pub fn toggle_notifications(&mut self, notification: SctpNotificationType, flag: bool) -> Result<(), std::io::Error> {
        let mut event: ffi::sctp_event = unsafe { std::mem::MaybeUninit::zeroed().assume_init() };
        event.se_assoc_id = SCTP_ALL_ASSOC;
        event.se_on = if flag { 1 } else { 0 };
        event.se_type = notification.value();
        let result = unsafe {
            usrsctp_setsockopt(self.socket.socket, IPPROTO_SCTP, SCTP_EVENT, &mut event as *mut ffi::sctp_event as *mut c_void, std::mem::size_of::<ffi::sctp_event>() as u32)
        };
        if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    /* for some reason, with Windows the usrsctp_getsockopt SCTP_PEER_ADDR_PARAMS failed with error 6... */
    pub fn change_peer_addr_params<F>(&mut self, cb: F) -> Result<(), std::io::Error>
        where
            F: Fn(&mut ffi::sctp_paddrparams) -> ()
    {
        let mut param = unsafe { std::mem::zeroed::<ffi::sctp_paddrparams>() };
        let mut param_length = std::mem::size_of::<ffi::sctp_paddrparams>() as u32;
        let result = unsafe {
            usrsctp_getsockopt(self.socket.socket, IPPROTO_SCTP, SCTP_PEER_ADDR_PARAMS, &mut param as *mut ffi::sctp_paddrparams as *mut c_void, &mut param_length as *mut u32)
        };
        if result != 0 { return Err(std::io::Error::last_os_error()); }

        cb(&mut param);
        let result = unsafe {
            usrsctp_setsockopt(self.socket.socket, IPPROTO_SCTP, SCTP_PEER_ADDR_PARAMS, &mut param as *mut ffi::sctp_paddrparams as *mut c_void, std::mem::size_of::<ffi::sctp_paddrparams>() as u32)
        };
        if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    pub fn toggle_assoc_resets(&mut self, assoc: u32, flag: bool) -> Result<(), std::io::Error> {
        let mut param = unsafe { std::mem::zeroed::<ffi::sctp_assoc_value>() };
        param.assoc_id = assoc;
        param.assoc_value = if flag { 1 } else { 0 };
        let result = unsafe {
            usrsctp_setsockopt(self.socket.socket, IPPROTO_SCTP, SCTP_ENABLE_STREAM_RESET, &mut param as *mut ffi::sctp_assoc_value  as *mut c_void, std::mem::size_of::<ffi::sctp_assoc_value>() as u32)
        };
        if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    pub fn toggle_no_delay(&mut self, flag: bool) -> Result<(), std::io::Error> {
        let mut value = if flag { 1 } else { 0 };
        let result = unsafe {
            usrsctp_setsockopt(self.socket.socket, IPPROTO_SCTP, SCTP_NODELAY, &mut value as *mut i32 as *mut c_void, std::mem::size_of::<i32>() as u32)
        };
        if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    pub fn set_init_parameters(&mut self, num_out_streams: u16, max_in_streams: u16, max_attempts: u16, max_init_timeout: u16) -> Result<(), std::io::Error> {
        let mut param = unsafe { std::mem::zeroed::<ffi::sctp_initmsg>() };
        param.sinit_num_ostreams = num_out_streams;
        param.sinit_max_instreams = max_in_streams;
        param.sinit_max_attempts = max_attempts;
        param.sinit_max_init_timeo = max_init_timeout;
        let result = unsafe {
            usrsctp_setsockopt(self.socket.socket, IPPROTO_SCTP, SCTP_INITMSG, &mut param as *mut ffi::sctp_initmsg  as *mut c_void, std::mem::size_of::<ffi::sctp_initmsg>() as u32)
        };
        if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    pub fn reset_streams(&mut self, streams: &[u16]) -> Result<(), std::io::Error> {
        let mut buffer = Vec::<u8>::new();
        buffer.resize(streams.len() * 2 + std::mem::size_of::<ffi::sctp_reset_streams>(), 0);

        unsafe {
            let mut param = buffer.as_mut_ptr() as *mut ffi::sctp_reset_streams;
            (*param).srs_flags = SCTP_STREAM_RESET_OUTGOING as u16;
            (*param).srs_number_streams = streams.len() as u16;

            let streams_ptr = buffer[std::mem::size_of::<ffi::sctp_reset_streams>()..].as_mut_ptr() as *mut u16;
            for index in 0..streams.len() {
                *streams_ptr.offset(index as isize) = streams[index];
            }
        }

        let result = unsafe {
            usrsctp_setsockopt(self.socket.socket, IPPROTO_SCTP, SCTP_RESET_STREAMS, buffer.as_mut_ptr() as *mut c_void, buffer.len() as u32)
        };
        if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    pub fn send(&mut self, buffer: &[u8], info: &SctpSendInfo) -> Result<(), std::io::Error> {
        let mut info = info.to_ffi();
        let result = unsafe {
            usrsctp_sendv(
                self.socket.socket,
                buffer.as_ptr() as *mut c_void,
                buffer.len(),
                std::ptr::null_mut(),
                0,
                &mut info as *mut ffi::sctp_sndinfo as *mut c_void,
                std::mem::size_of::<ffi::sctp_sndinfo>() as u32,
                SCTP_SENDV_SNDINFO,
                0
            )
        };

        if result == -1 {
            Err(std::io::Error::last_os_error())
        } else if result as usize == buffer.len() {
            Ok(())
        } else {
            Err(std::io::Error::new(ErrorKind::Interrupted, format!("write {} out of {} bytes", result, buffer.len())))
        }
    }
}

impl<T: Read + Write + Unpin> Drop for UsrSctpSession<T> {
    fn drop(&mut self) {
        SCTP_INSTANCE.lock().unwrap().destroy_address(self.usr_socket_id);
    }
}

impl<T: Read + Write + Unpin> Stream for UsrSctpSession<T> {
    type Item = UsrSctpSessionEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let self_mut = self.get_mut();

        while let Poll::Ready(message) = self_mut.socket_channel.poll_next_unpin(cx) {
            if let Some(message) = message {
                match message {
                    SctpUserCallbackAddressEvent::MessageReceived { buffer, flags, info } => {
                        if (flags as u32 & MSG_NOTIFICATION) != 0 {
                            match SctpNotification::parse(&buffer) {
                                Ok(notification) => return Poll::Ready(Some(UsrSctpSessionEvent::EventReceived { notification })),
                                Err(error) => {
                                    eprintln!("Failed to parse SCTP notification: {}", error);
                                }
                            }
                        } else {
                            return Poll::Ready(Some(UsrSctpSessionEvent::MessageReceived { buffer, info }));
                        }
                    },
                    SctpUserCallbackAddressEvent::MessageSend { buffer } => {
                        /* TODO: We need some kind of contract that every stream we receive will try to send the complete message without blocking  */
                        self_mut.stream.write(&buffer).expect("failed to send message");
                    }
                }
            } else {
                panic!("unexpected channel close");
            }
        }

        /* process all incoming messages */
        let mut buffer = [0u8; 2048];
        loop {
            match self_mut.stream.read(&mut buffer) {
                Ok(bytes) => {
                    unsafe {
                        usrsctp_conninput(self_mut.usr_socket_id as *mut c_void, buffer.as_ptr() as *const c_void, bytes, 0);
                    }
                }
                Err(error) => {
                    if error.kind() == ErrorKind::WouldBlock {
                        break;
                    } else {
                        /* TODO better handling, maybe pip through */
                        println!("Having some critical underlying IO error!");
                        break;
                    }
                }
            }
        }

        Poll::Pending
    }
}