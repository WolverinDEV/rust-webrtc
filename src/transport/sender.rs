use crate::srtp2::{Srtp2, Srtp2ErrorCode};
use libnice::ice::ComponentWriter;
use std::sync::{Mutex, Arc};
use std::io::Write;
use crate::utils::rtcp::RtcpPacket;
use futures::{Future, FutureExt};
use futures::task::{Context, Poll};
use tokio::macros::support::Pin;
use crate::transport::packet_history::RtpPacketHistory;
use slog::{ slog_error };
use crate::global_logger;

const SRTP_ADDITIONAL_HEADER_SIZE: usize = 148;

pub(crate) struct SenderBackend {
    pub srtp: Srtp2,
    pub transport: ComponentWriter
}

struct SenderBase {
    backend: Arc<Mutex<Option<SenderBackend>>>,
    transport: Option<ComponentWriter>
}

#[derive(Debug)]
enum PacketProtectError {
    BufferTooSmall,
    BackendMissing,
    ErrorProtect(Srtp2ErrorCode)
}

impl SenderBase {
    /// Try to write data to the transport, returns true if succeeded
    pub fn write_data(&mut self, data: &[u8]) -> bool {
        /*
        if rand::random::<u8>() < 90 {
            println!("Droppign RTP/RTCP");
            return true;
        }
        */

        if let Some(transport) = &mut self.transport {
            if let Err(_) = transport.write(data) {
                /* transport pipe has been broken */
                self.transport = None;
            } else {
                return true;
            }
        }
        if let Some(backend) = self.backend.lock().unwrap().as_mut() {
            if let Err(_) = backend.transport.write(data) {
                /* the backend transport pipe has been broken as well, don't update our transport reference */
            } else {
                self.transport = Some(backend.transport.clone());
                return true;
            }
        }

        false
    }

    pub fn protect_rtp(&mut self, buffer: &mut [u8], payload_length: usize) -> Result<usize, PacketProtectError> {
        if buffer.len() < payload_length + SRTP_ADDITIONAL_HEADER_SIZE {
            return Err(PacketProtectError::BufferTooSmall);
        }

        return if let Some(backend) = self.backend.lock().unwrap().as_mut() {
            backend.srtp.protect(buffer, payload_length)
                .map_err(|err| PacketProtectError::ErrorProtect(err))
        } else {
            Err(PacketProtectError::BackendMissing)
        }
    }

    pub fn protect_rtcp(&mut self, buffer: &mut [u8], payload_length: usize) -> Result<usize, PacketProtectError> {
        if buffer.len() < payload_length + SRTP_ADDITIONAL_HEADER_SIZE {
            return Err(PacketProtectError::BufferTooSmall);
        }

        return if let Some(backend) = self.backend.lock().unwrap().as_mut() {
            backend.srtp.protect_rtcp(buffer, payload_length)
                .map_err(|err| PacketProtectError::ErrorProtect(err))
        } else {
            Err(PacketProtectError::BackendMissing)
        }
    }
}

pub struct RtpSender {
    base: SenderBase,
    history: RtpPacketHistory
}

impl RtpSender {
    pub(crate) fn new(backend: Arc<Mutex<Option<SenderBackend>>>) -> Self {
        RtpSender {
            base: SenderBase {
                backend,
                transport: None
            },
            history: RtpPacketHistory::new()
        }
    }

    /// Retransmit a RTP packet
    pub fn retransmit_rtp(&mut self, sequence_no: u16) {
        if let Some(buffer) = self.history.resend_packet(sequence_no) {
            self.base.write_data(buffer);
        }
    }

    /// Send a RTP packet
    pub fn send_rtp(&mut self, sequence_no: u16, data: &[u8]) {
        let mut buffer = unsafe { std::mem::MaybeUninit::<[u8; 2048]>::uninit().assume_init() };
        buffer[0..data.len()].clone_from_slice(data);

        match self.base.protect_rtp(&mut buffer, data.len()) {
            Ok(length) => {
                self.history.enqueue_send_packet(sequence_no, buffer[0..length].to_vec());
                self.base.write_data(&buffer[0..length]);
            },
            Err(PacketProtectError::BackendMissing) => { /* seems like we're not yet/anymore connected to anything */ }
            Err(error) => {
                /* TODO: Move away from global logger */
                slog_error!(global_logger(), "Failed to protect RTP packet: {:?}", error);
            }
        }
    }
}

impl Future for RtpSender {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.history.poll_unpin(cx)
    }
}

pub struct RtcpSender {
    base: SenderBase
}

impl RtcpSender {
    pub(crate) fn new(backend: Arc<Mutex<Option<SenderBackend>>>) -> Self {
        RtcpSender {
            base: SenderBase {
                backend,
                transport: None
            }
        }
    }

    pub fn send_rtcp(&mut self, data: &[u8]) {
        let mut buffer = unsafe { std::mem::MaybeUninit::<[u8; 2048]>::uninit().assume_init() };
        buffer[0..data.len()].clone_from_slice(data);

        self.encrypt_and_send(&mut buffer, data.len());
    }

    pub fn send(&mut self, data: &RtcpPacket) {
        let mut buffer = unsafe { std::mem::MaybeUninit::<[u8; 2048]>::uninit().assume_init() };
        match data.write(&mut buffer) {
            Ok(length) => {
                self.encrypt_and_send(&mut buffer, length);
            },
            Err(error) => {
                /* TODO: Move away from global logger */
                slog_error!(global_logger(), "Failed to create RtcpPacket: {}", error);
            }
        }
    }

    fn encrypt_and_send(&mut self, buffer: &mut [u8], length: usize) {
        match self.base.protect_rtcp(buffer, length) {
            Ok(length) => {
                self.base.write_data(&buffer[0..length]);
            },
            Err(PacketProtectError::BackendMissing) => { /* seems like we're not yet/anymore connected to anything */ }
            Err(error) => {
                /* TODO: Move away from global logger */
                slog_error!(global_logger(), "Failed to protect RTCP packet: {:?}", error);
            }
        }
    }
}