use tokio::sync::mpsc;
use futures::task::{Context, Poll};
use crate::utils::PacketId;
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
    ResendPackets(Vec<PacketId>),
    PacketTimedOut(Vec<PacketId>)
}

const RECEIVE_WINDOW_SIZE: usize = 64;

/// Listens to the incoming packet ids and request resends if required
pub struct RtpPacketResendRequester {
    base_timestamp: u64,

    /// Last received, valid packet id
    last_packet_id: PacketId,
    /// Received timestamp of the last valid packet
    last_packet_timestamp: u32,

    /// Current lower end packet id
    receive_index: PacketId,
    /// Current lower end buffer index
    receive_timestamp_index: usize,
    /// Buffer containing the receive timestamps of each packet.
    /// Array size must be a power of two, else we'll receive unexpected results when the packet id wraps.
    receive_timestamps: [u32; RECEIVE_WINDOW_SIZE],

    /// The number of new packets which have to be arrived to start a resend
    /// This must be lower than `RECEIVE_WINDOW_SIZE`.
    threshold_start_resend: u16,

    /* must be at least the receive_timestamps length, else black magic happens */
    clipping_window: u16,

    events: (mpsc::UnboundedSender<RtpPacketResendRequesterEvent>, mpsc::UnboundedReceiver<RtpPacketResendRequesterEvent>),
    resend_delay: Option<tokio::time::Delay>
}

/* TODO: Resend after a certain time (Currently only after n packets) */
impl RtpPacketResendRequester {
    pub fn new() -> Self {
        let base_time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
            .unwrap().as_millis() as u64 - 1001u64;

        RtpPacketResendRequester {
            base_timestamp: base_time,

            last_packet_id: PacketId::new(0),
            last_packet_timestamp: 0xFFFFFFFF,

            receive_index: PacketId::new(0),
            receive_timestamp_index: 0,
            receive_timestamps: [1u32; RECEIVE_WINDOW_SIZE],

            threshold_start_resend: 5,
            clipping_window: 1024,

            events: mpsc::unbounded_channel(),
            resend_delay: None
        }
    }

    pub fn reset(&mut self) {
        self.resend_delay = None;
        self.last_packet_id = PacketId::new(0);
        self.last_packet_timestamp = 0xFFFFFFFF;

        self.flush_pending_packets(false);
        self.receive_timestamp_index = 0;
        self.receive_index = self.last_packet_id;
    }

    pub fn reset_resends(&mut self) {
        self.resend_delay = None;
        self.flush_pending_packets(false);
    }

    pub fn handle_packet_received(&mut self, id: PacketId) -> PacketReceivedResult {
        let current_timestamp = self.current_timestamp();
        if current_timestamp.wrapping_sub(self.last_packet_timestamp) > 1000 {
            /* We've not received packets for a quite long time. Marking the old sequence as finished and beginning a new one. */
            let had_sequence = self.receive_timestamps.iter().find(|e| **e != 1).is_some();
            self.flush_pending_packets(true);
            self.receive_index = id + 1 - self.receive_timestamps.len() as u16;
            self.receive_timestamp_index = 0;
            if had_sequence {
                let _ = self.events.0.send(RtpPacketResendRequesterEvent::SequenceTimedOut);
            }
        } else if id.is_less(&self.receive_index, Some(self.clipping_window)) {
            return PacketReceivedResult::PacketTooOld;
        }

        let mut distance = self.receive_index.difference(&id, Some(self.clipping_window)) as usize;
        if distance >= self.receive_timestamps.len() * 2 {
            /* Assuming we're having a new sequence, or lost more than self.receive_timestamps.len() packets. */
            let had_sequence = self.receive_timestamps.iter().find(|e| **e != 1).is_some();
            self.flush_pending_packets(true);
            self.receive_index = id + 1 - self.receive_timestamps.len() as u16;
            self.receive_timestamp_index = 0;
            distance = self.receive_timestamps.len() - 1;
            if had_sequence {
                let _ = self.events.0.send(RtpPacketResendRequesterEvent::SequenceGapped);
            }
        } else if distance >= self.receive_timestamps.len() {
            /* new_packets is ideally 1 which means that we've received an in order packet */
            let new_packets = distance - self.receive_timestamps.len() + 1;

            let mut lost_packets = [PacketId::new(0); RECEIVE_WINDOW_SIZE];
            let mut lost_packets_count = 0;

            for index in 0..new_packets {
                let timestamp_index = (self.receive_timestamp_index + index) % self.receive_timestamps.len();
                if std::mem::replace(&mut self.receive_timestamps[timestamp_index], 0) == 0 {
                    lost_packets[lost_packets_count] = self.receive_index + index as u16;
                    lost_packets_count = lost_packets_count + 1;
                }
            }

            if lost_packets_count > 0 {
                let _ = self.events.0.send(RtpPacketResendRequesterEvent::PacketTimedOut(lost_packets[0..lost_packets_count].to_vec()));
            }

            self.receive_index = self.receive_index + new_packets as u16;
            self.receive_timestamp_index = (self.receive_timestamp_index + new_packets) % self.receive_timestamps.len();
            distance = self.receive_timestamps.len() - 1;
        }

        let index = (self.receive_timestamp_index + distance) % self.receive_timestamps.len();
        self.receive_timestamps[index] = current_timestamp;

        self.last_packet_id = id;
        self.last_packet_timestamp = current_timestamp;

        self.update_packet_resend_requests(false);
        PacketReceivedResult::Success
    }

    #[cfg(test)]
    fn missing_packets(&self) -> Vec<PacketId> {
        let mut result = Vec::<PacketId>::with_capacity(self.receive_timestamps.len());
        let mut index = self.receive_timestamp_index;
        let mut packet_index = self.receive_index;
        loop {
            if self.receive_timestamps[index] == 0 {
                result.push(packet_index);
            }

            index = (index + 1) % self.receive_timestamps.len();
            if index == self.receive_timestamp_index { break; }
            packet_index = packet_index + 1;
        }
        result
    }

    fn current_timestamp(&self) -> u32 {
        let time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();

        (time.as_millis() as u64 - self.base_timestamp) as u32
    }

    /// Assume all, not yet received packets as lost.
    /// Call this if
    fn flush_pending_packets(&mut self, emit_lost: bool) {
        let mut index = self.receive_timestamp_index;
        let mut packet_index = self.receive_index;

        let mut lost_packets = [PacketId::new(0); RECEIVE_WINDOW_SIZE];
        let mut lost_packets_count = 0;

        loop {
            if self.receive_timestamps[index] == 0 {
                self.receive_timestamps[index] = 1; /* mark slot as received */
                lost_packets[lost_packets_count] = packet_index;
                lost_packets_count = lost_packets_count + 1;
            }

            index = (index + 1) % self.receive_timestamps.len();
            if index == self.receive_timestamp_index { break; }
            packet_index = packet_index + 1;
        }

        if emit_lost && lost_packets_count > 0 {
            let _ = self.events.0.send(RtpPacketResendRequesterEvent::PacketTimedOut(lost_packets[0..lost_packets_count].to_vec()));
        }
    }

    fn update_packet_resend_requests(&mut self, force: bool) {
        if self.resend_delay.is_some() && !force {
            /* nothing to do, we've already scheduled a resend */
            return;
        }

        debug_assert!((self.threshold_start_resend as usize) < self.receive_timestamps.len());

        let mut resend_packets = [PacketId::new(0); RECEIVE_WINDOW_SIZE];
        let mut resend_packets_count = 0;

        let mut index = self.receive_timestamp_index;
        let mut packet_index = self.receive_index;
        loop {
            if self.receive_timestamps[index] == 0 {
                resend_packets[resend_packets_count] = packet_index;
                resend_packets_count = resend_packets_count + 1;
            }

            index = (index + 1) % self.receive_timestamps.len();
            if (index + self.threshold_start_resend as usize) % self.receive_timestamps.len() == self.receive_timestamp_index { break; }
            packet_index = packet_index + 1;
        }


        if resend_packets_count > 0 {
            let _ = self.events.0.send(RtpPacketResendRequesterEvent::ResendPackets(resend_packets[0..resend_packets_count].to_vec()));

            /* TODO: Use RTT */
            self.resend_delay = Some(tokio::time::delay_for(Duration::from_millis(100)));
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
    use crate::utils::{PacketReceivedResult, PacketId, RtpPacketResendRequester};

    #[test]
    fn test_init_setup() {
        let mut instance = RtpPacketResendRequester::new();
        assert_eq!(instance.handle_packet_received(PacketId::new(222)), PacketReceivedResult::Success);
        assert!(instance.missing_packets().is_empty());

        assert_eq!(instance.handle_packet_received(PacketId::new(223)), PacketReceivedResult::Success);
        assert!(instance.missing_packets().is_empty());

        assert_eq!(instance.handle_packet_received(PacketId::new(225)), PacketReceivedResult::Success);
        assert_eq!(instance.missing_packets(), vec![224]);
    }

    #[test]
    fn test_wrap() {
        let mut instance = RtpPacketResendRequester::new();
        assert_eq!(instance.handle_packet_received(PacketId::new(0xFFFE)), PacketReceivedResult::Success);
        assert!(instance.missing_packets().is_empty());

        assert_eq!(instance.handle_packet_received(PacketId::new(0xFFFF)), PacketReceivedResult::Success);
        assert!(instance.missing_packets().is_empty());

        assert_eq!(instance.handle_packet_received(PacketId::new(0x0000)), PacketReceivedResult::Success);
        assert!(instance.missing_packets().is_empty());

        assert_eq!(instance.handle_packet_received(PacketId::new(0x0001)), PacketReceivedResult::Success);
        assert!(instance.missing_packets().is_empty());
    }

    #[test]
    fn test_wrap_loss_0() {
        let mut instance = RtpPacketResendRequester::new();
        assert_eq!(instance.handle_packet_received(PacketId::new(0xFFFE)), PacketReceivedResult::Success);
        assert!(instance.missing_packets().is_empty());

        assert_eq!(instance.handle_packet_received(PacketId::new(0x0000)), PacketReceivedResult::Success);
        assert_eq!(instance.missing_packets(), vec![0xFFFF]);

        assert_eq!(instance.handle_packet_received(PacketId::new(0x0001)), PacketReceivedResult::Success);
        assert_eq!(instance.missing_packets(), vec![0xFFFF]);
    }

    #[test]
    fn test_wrap_loss_1() {
        let mut instance = RtpPacketResendRequester::new();
        assert_eq!(instance.handle_packet_received(PacketId::new(0xFFFE)), PacketReceivedResult::Success);
        assert!(instance.missing_packets().is_empty());

        assert_eq!(instance.handle_packet_received(PacketId::new(0xFFFF)), PacketReceivedResult::Success);
        assert!(instance.missing_packets().is_empty());

        assert_eq!(instance.handle_packet_received(PacketId::new(0x0001)), PacketReceivedResult::Success);
        assert_eq!(instance.missing_packets(), vec![0x0000]);
    }

    #[test]
    fn test_general_loss_1() {
        let mut instance = RtpPacketResendRequester::new();
        for id in 0..10000 {
            instance.handle_packet_received(PacketId::new(id as u16));
            assert!(instance.missing_packets().is_empty());
        }
    }
}