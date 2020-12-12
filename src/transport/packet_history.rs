use std::time::SystemTime;
use std::collections::VecDeque;
use futures::{Future, StreamExt};
use futures::task::{Context, Poll};
use tokio::macros::support::Pin;
use tokio::time::Duration;

pub struct RtpSendPacket {
    pub sequence: u16,
    pub first_send: u32,
    pub last_send: u32,
    pub buffer: Vec<u8>,
}

pub struct RtpPacketHistory {
    base_timestamp: u64,

    max_saved_packets: usize,
    max_packet_age: u32,
    max_saved_bytes: usize,

    min_resend_interval: u32,

    current_byte_count: usize,
    packets: VecDeque<Box<RtpSendPacket>>,

    cleanup_interval: Option<tokio::time::Interval>
}

impl RtpPacketHistory {
    pub fn new() -> Self {
        let base_time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
            .unwrap().as_millis() as u64;

        let max_packet_age = 10_000;
        RtpPacketHistory{
            base_timestamp: base_time - max_packet_age as u64,

            max_saved_packets: 1000,
            max_packet_age,
            max_saved_bytes: 1_000_000,

            /* TODO: Some kind of RTT */
            min_resend_interval: 50,

            current_byte_count: 0,
            packets: VecDeque::with_capacity(1000),

            cleanup_interval: None
        }
    }

    pub fn enqueue_send_packet(&mut self, sequence: u16, buffer: Vec<u8>) {
        let current_timestamp = self.current_timestamp();

        let packet = Box::new(RtpSendPacket {
            sequence,
            first_send: current_timestamp,
            last_send: current_timestamp,
            buffer
        });

        self.current_byte_count += packet.buffer.len();
        self.packets.push_back(packet);
        self.cleanup();
    }

    pub fn resend_packet(&mut self, sequence: u16) -> Option<&[u8]> {
        let current_timestamp = self.current_timestamp();
        let min_resend_interval = self.min_resend_interval;

        self.packets.iter_mut()
            .find(|pkt| pkt.sequence == sequence)
            .filter(|pkt| pkt.last_send + min_resend_interval < current_timestamp)
            .map(|pkt| {
                pkt.last_send = current_timestamp;
                pkt.buffer.as_slice()
            })
    }

    fn current_timestamp(&self) -> u32 {
        let time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();

        (time.as_millis() as u64 - self.base_timestamp) as u32
    }

    fn cleanup(&mut self) {
        debug_assert!(self.max_saved_bytes > 0);
        while self.current_byte_count > self.max_saved_bytes {
            let packet = self.packets.pop_front()
                .expect("having saved bytes but no packet?!");

            self.current_byte_count -= packet.buffer.len();
        }

        debug_assert!(self.current_timestamp() >= self.max_packet_age);
        let timeout_time = self.current_timestamp() - self.max_packet_age;
        while let Some(packet) = self.packets.front() {
            if packet.last_send <= timeout_time {
                self.current_byte_count -= packet.buffer.len();
                self.packets.pop_front();
            } else {
                break;
            }
        }

        debug_assert!(self.max_saved_packets > 0);
        while self.packets.len() > self.max_saved_packets {
            let packet = self.packets.pop_front().unwrap();
            self.current_byte_count -= packet.buffer.len();
        }
    }
}

impl Future for RtpPacketHistory {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.cleanup_interval.is_none() {
            self.cleanup_interval = Some(tokio::time::interval(Duration::from_secs(1)));
        }

        if let Poll::Ready(_) = self.cleanup_interval.as_mut().unwrap().poll_next_unpin(cx) {
            self.cleanup()
        }
        Poll::Pending
    }
}