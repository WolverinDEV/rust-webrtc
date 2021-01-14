use tokio::sync::mpsc;
use futures::task::{Context, Poll};
use crate::utils::SequenceNumber;
use std::time::SystemTime;
use tokio::stream::Stream;
use tokio::macros::support::Pin;
use futures::{StreamExt, FutureExt};
use tokio::time::Duration;

#[derive(Debug, PartialEq)]
pub enum PacketReceivedResult {
    /// The packet has been received successfully
    Success,
    /// The packet is too old
    PacketTooOld
}

#[derive(Debug, PartialEq)]
pub enum RtpPacketResendRequesterEvent {
    SequenceTimedOut,
    SequenceGapped,
    ResendPackets(Vec<SequenceNumber<u16>>),
    PacketTimedOut(Vec<SequenceNumber<u16>>)
}

/// Listens to the incoming packet ids and request resends if required
pub struct RtpPacketResendRequester {
    base_timestamp: u64,

    /// Last received, valid packet id
    last_packet_id: SequenceNumber<u16>,
    /// Received timestamp of the last valid packet
    last_packet_timestamp: u32,

    /// Current lower end packet id
    pending_index: SequenceNumber<u16>,
    /// Current lower end buffer index
    pending_timestamp_index: usize,
    /// Buffer containing the expected arrival timestamps.
    /// If zero, the packet has been received.
    /// Array size must be a power of two, else we'll receive unexpected results when the packet id wraps.
    pending_timestamps: Vec<u32>,

    /* must be at least the receive_timestamps length, else black magic happens */
    clipping_window: u16,

    nack_delay: u32,
    resend_request_interval: u32,

    events: (mpsc::UnboundedSender<RtpPacketResendRequesterEvent>, mpsc::UnboundedReceiver<RtpPacketResendRequesterEvent>),
    resend_delay: Option<tokio::time::Delay>,

    temp_resend_packets: Vec<SequenceNumber<u16>>,
    temp_lost_packets: Vec<SequenceNumber<u16>>,
}

const RESEND_PACKETS_TMP_BUFFER_LENGTH: usize = 32;
const LOST_PACKETS_TMP_BUFFER_LENGTH: usize = 32;

impl RtpPacketResendRequester {
    pub fn new(frame_size: Option<usize>) -> Self {
        let base_time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
            .unwrap().as_millis() as u64;

        RtpPacketResendRequester {
            base_timestamp: base_time,

            last_packet_id: SequenceNumber::new(0),
            last_packet_timestamp: 0xFFFF0000,

            pending_index: SequenceNumber::new(0),
            pending_timestamp_index: 0,
            pending_timestamps: vec![0xFFFFFFFF; frame_size.unwrap_or(64)],

            clipping_window: 1024,

            nack_delay: 5,
            resend_request_interval: 25,

            events: mpsc::unbounded_channel(),
            resend_delay: None,

            temp_resend_packets: Vec::with_capacity(RESEND_PACKETS_TMP_BUFFER_LENGTH),
            temp_lost_packets: Vec::with_capacity(LOST_PACKETS_TMP_BUFFER_LENGTH)
        }
    }

    pub fn set_frame_size(&mut self, frame_size: usize) {
        /* FIXME: Just update the buffer size instead of resetting everything */

        self.flush_pending_packets(true);
        self.pending_timestamps.resize(frame_size, 0xFFFFFFFF);
        self.reset();
    }

    pub fn set_nack_delay(&mut self, delay: u32) {
        self.nack_delay = delay;
    }

    pub fn set_resend_interval(&mut self, interval: u32) {
        self.resend_request_interval = interval;
    }

    pub fn reset(&mut self) {
        self.resend_delay = None;
        self.last_packet_id = SequenceNumber::new(0);
        self.last_packet_timestamp = 0xFFFF0000;

        self.flush_pending_packets(false);
        self.pending_timestamp_index = 0;
        self.pending_index = self.last_packet_id;
        self.pending_timestamps.iter_mut().for_each(|val| *val = 0xFFFFFFFF);
    }

    pub fn last_packet_id(&self) -> u16 {
        self.last_packet_id.packet_id
    }

    pub fn reset_resends(&mut self) {
        self.resend_delay = None;
        self.flush_pending_packets(false);
    }

    pub fn handle_packet_received(&mut self, id: SequenceNumber<u16>) -> PacketReceivedResult {
        let current_timestamp = self.current_timestamp();
        if current_timestamp.wrapping_sub(self.last_packet_timestamp) > 1000 {
            /* We've not received packets for a quite long time. Marking the old sequence as finished and beginning a new one. */
            let had_sequence = self.pending_timestamps.iter().find(|e| **e != 0xFFFFFFFF).is_some();
            self.flush_pending_packets(true);
            self.pending_index = id + 1 - self.pending_timestamps.len() as u16;
            self.pending_timestamp_index = 0;
            if had_sequence {
                let _ = self.events.0.send(RtpPacketResendRequesterEvent::SequenceTimedOut);
            }
        } else if id.is_less(&self.pending_index, Some(self.clipping_window)) {
            return PacketReceivedResult::PacketTooOld;
        }

        let mut distance = self.pending_index.difference(&id, Some(self.clipping_window)) as usize;
        if distance >= self.pending_timestamps.len() * 2 {
            /* Assuming we're having a new sequence, or lost more than self.receive_timestamps.len() packets. */
            let had_sequence = self.pending_timestamps.iter().find(|e| **e != 0xFFFFFFFF).is_some();
            self.flush_pending_packets(true);
            self.pending_index = id + 1 - self.pending_timestamps.len() as u16;
            self.pending_timestamp_index = 0;
            distance = self.pending_timestamps.len() - 1;
            if had_sequence {
                let _ = self.events.0.send(RtpPacketResendRequesterEvent::SequenceGapped);
            }
        } else if distance >= self.pending_timestamps.len() {
            /* new_packets is ideally 1 which means that we've received an in order packet */
            let new_packets = distance - self.pending_timestamps.len() + 1;

            /* TODO: Add RTT here, else out of order packets are directly counted as loss */
            let expected_arrival_timestamp = self.current_timestamp() + self.nack_delay;

            for index in 0..new_packets {
                let timestamp_index = (self.pending_timestamp_index + index) % self.pending_timestamps.len();
                if std::mem::replace(&mut self.pending_timestamps[timestamp_index], expected_arrival_timestamp) != 0xFFFFFFFF {
                    self.temp_lost_packets.push(self.pending_index + index as u16);
                }
            }

            if !self.temp_lost_packets.is_empty() {
                let _ = self.events.0.send(RtpPacketResendRequesterEvent::PacketTimedOut(self.temp_lost_packets.clone()));
                self.temp_lost_packets.truncate(LOST_PACKETS_TMP_BUFFER_LENGTH);
                self.temp_lost_packets.clear();
            }

            self.pending_index = self.pending_index + new_packets as u16;
            self.pending_timestamp_index = (self.pending_timestamp_index + new_packets) % self.pending_timestamps.len();
            distance = self.pending_timestamps.len() - 1;
        }

        let index = (self.pending_timestamp_index + distance) % self.pending_timestamps.len();
        self.pending_timestamps[index] = 0xFFFFFFFF; /* packet has been received */

        self.last_packet_id = id;
        self.last_packet_timestamp = current_timestamp;

        self.update_packet_resend_requests(false);
        PacketReceivedResult::Success
    }

    #[cfg(test)]
    fn missing_packets(&self) -> Vec<SequenceNumber<u16>> {
        let mut result = Vec::<SequenceNumber<u16>>::with_capacity(self.pending_timestamps.len());
        let mut index = self.pending_timestamp_index;
        let mut packet_index = self.pending_index;

        let current_timestamp = self.current_timestamp();
        loop {
            if self.pending_timestamps[index] <= current_timestamp {
                result.push(packet_index);
            }

            index = (index + 1) % self.pending_timestamps.len();
            if index == self.pending_timestamp_index { break; }
            packet_index = packet_index + 1;
        }
        result
    }

    #[cfg(test)]
    fn advance_clock(&mut self, millis: u32) {
        self.base_timestamp = self.base_timestamp - millis as u64;
    }

    fn current_timestamp(&self) -> u32 {
        let time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();

        (time.as_millis() as u64 - self.base_timestamp) as u32
    }

    /// Assume all, not yet received packets as lost.
    /// Call this if
    fn flush_pending_packets(&mut self, emit_lost: bool) {
        let mut index = self.pending_timestamp_index;
        let mut packet_index = self.pending_index;

        loop {
            if self.pending_timestamps[index] != 0xFFFFFFFF {
                self.pending_timestamps[index] = 0xFFFFFFFF; /* mark slot as received */
                self.temp_lost_packets.push(packet_index);
            }

            index = (index + 1) % self.pending_timestamps.len();
            if index == self.pending_timestamp_index { break; }
            packet_index = packet_index + 1;
        }

        if emit_lost && !self.temp_lost_packets.is_empty() {
            let _ = self.events.0.send(RtpPacketResendRequesterEvent::PacketTimedOut(self.temp_lost_packets.clone()));
            self.temp_lost_packets.truncate(LOST_PACKETS_TMP_BUFFER_LENGTH);
            self.temp_lost_packets.clear();
        }
    }

    fn update_packet_resend_requests(&mut self, force: bool) {
        if self.resend_delay.is_some() && !force {
            /* nothing to do, we've already scheduled a resend */
            return;
        }

        let mut index = self.pending_timestamp_index;
        let mut packet_index = self.pending_index;

        let current_time = self.current_timestamp();
        let mut next_minimal_expected_time = 0xFFFFFFFF;

        loop {
            if self.pending_timestamps[index] <= current_time {
                let difference = current_time - self.pending_timestamps[index];
                if difference >= 1000 {
                    /* that packet is being lost... don't recover, count as timeout */
                    self.temp_lost_packets.push(packet_index);
                    self.pending_timestamps[index] = 0xFFFFFFFF;
                } else {
                    self.temp_resend_packets.push(packet_index);
                }
            } else {
                next_minimal_expected_time = next_minimal_expected_time.min(self.pending_timestamps[index]);
            }

            index = (index + 1) % self.pending_timestamps.len();
            if index == self.pending_timestamp_index { break; }
            packet_index = packet_index + 1;
        }

        if !self.temp_lost_packets.is_empty() {
            let _ = self.events.0.send(RtpPacketResendRequesterEvent::PacketTimedOut(self.temp_lost_packets.clone()));
            self.temp_lost_packets.truncate(LOST_PACKETS_TMP_BUFFER_LENGTH);
            self.temp_lost_packets.clear();
        }

        if !self.temp_resend_packets.is_empty() {
            let _ = self.events.0.send(RtpPacketResendRequesterEvent::ResendPackets(self.temp_resend_packets.clone()));
            self.temp_resend_packets.truncate(RESEND_PACKETS_TMP_BUFFER_LENGTH);
            self.temp_resend_packets.clear();
            /* TODO: Use RTT */
            self.resend_delay = Some(tokio::time::delay_for(Duration::from_millis(self.resend_request_interval as u64)));
        } else if next_minimal_expected_time != 0xFFFFFFFF {
            self.resend_delay = Some(tokio::time::delay_for(Duration::from_millis((next_minimal_expected_time - current_time) as u64)));
        }
    }
}

impl Stream for RtpPacketResendRequester {
    type Item = RtpPacketResendRequesterEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.events.1.poll_next_unpin(cx) {
            Poll::Ready(Some(event)) => {
                Poll::Ready(Some(event))
            },
            _ => {
                if let Some(timer) = &mut self.resend_delay {
                    if let Poll::Ready(_) = timer.poll_unpin(cx) {
                        self.resend_delay.take();
                        self.update_packet_resend_requests(false);
                    }
                }
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::utils::{PacketReceivedResult, SequenceNumber, RtpPacketResendRequester};
    use futures::StreamExt;

    #[test]
    fn test_init_setup() {
        tokio_test::block_on(async {
            let mut instance = RtpPacketResendRequester::new(Some(32));
            assert_eq!(instance.handle_packet_received(SequenceNumber::new(222)), PacketReceivedResult::Success);
            assert!(instance.missing_packets().is_empty());

            assert_eq!(instance.handle_packet_received(SequenceNumber::new(223)), PacketReceivedResult::Success);
            assert!(instance.missing_packets().is_empty());

            assert_eq!(instance.handle_packet_received(SequenceNumber::new(225)), PacketReceivedResult::Success);
            let (event, instance) = instance.into_future().await;
            println!("1: {:?}", event);
            let (event, instance) = instance.into_future().await;
            println!("2: {:?}", event);
            let (event, instance) = instance.into_future().await;
            println!("3: {:?}", event);
            let (event, mut instance) = instance.into_future().await;
            println!("4: {:?}", event);
            instance.advance_clock(500);
            assert_eq!(instance.missing_packets(), vec![224]);
            let (event, _instance) = instance.into_future().await;
            println!("5: {:?}", event);
        });
    }

    #[test]
    fn test_wrap() {
        tokio_test::block_on(async {
            let mut instance = RtpPacketResendRequester::new(Some(32));
            assert_eq!(instance.handle_packet_received(SequenceNumber::new(0xFFFE)), PacketReceivedResult::Success);
            assert!(instance.missing_packets().is_empty());

            assert_eq!(instance.handle_packet_received(SequenceNumber::new(0xFFFF)), PacketReceivedResult::Success);
            assert!(instance.missing_packets().is_empty());

            assert_eq!(instance.handle_packet_received(SequenceNumber::new(0x0000)), PacketReceivedResult::Success);
            assert!(instance.missing_packets().is_empty());

            assert_eq!(instance.handle_packet_received(SequenceNumber::new(0x0001)), PacketReceivedResult::Success);
            assert!(instance.missing_packets().is_empty());

            instance.advance_clock(500);
            assert!(instance.missing_packets().is_empty());
        });
    }

    #[test]
    fn test_wrap_loss_0() {
        tokio_test::block_on(async {
            let mut instance = RtpPacketResendRequester::new(Some(32));
            assert_eq!(instance.handle_packet_received(SequenceNumber::new(0xFFFE)), PacketReceivedResult::Success);
            assert!(instance.missing_packets().is_empty());

            assert_eq!(instance.handle_packet_received(SequenceNumber::new(0x0000)), PacketReceivedResult::Success);
            assert!(instance.missing_packets().is_empty());

            instance.advance_clock(500);
            assert_eq!(instance.missing_packets(), vec![0xFFFF]);

            assert_eq!(instance.handle_packet_received(SequenceNumber::new(0x0001)), PacketReceivedResult::Success);
            assert_eq!(instance.missing_packets(), vec![0xFFFF]);
        });
    }

    #[test]
    fn test_wrap_loss_1() {
        tokio_test::block_on(async {
            let mut instance = RtpPacketResendRequester::new(Some(32));
            assert_eq!(instance.handle_packet_received(SequenceNumber::new(0xFFFE)), PacketReceivedResult::Success);
            assert!(instance.missing_packets().is_empty());

            assert_eq!(instance.handle_packet_received(SequenceNumber::new(0xFFFF)), PacketReceivedResult::Success);
            assert!(instance.missing_packets().is_empty());

            assert_eq!(instance.handle_packet_received(SequenceNumber::new(0x0001)), PacketReceivedResult::Success);
            assert!(instance.missing_packets().is_empty());

            instance.advance_clock(500);
            assert_eq!(instance.missing_packets(), vec![0x0000]);
        });
    }

    #[test]
    fn test_general_loss_1() {
        tokio_test::block_on(async {
            let mut instance = RtpPacketResendRequester::new(Some(32));
            for id in 0..10000 {
                instance.handle_packet_received(SequenceNumber::new(id as u16));
                instance.advance_clock(1000);
                assert!(instance.missing_packets().is_empty());
            }
        });
    }
}