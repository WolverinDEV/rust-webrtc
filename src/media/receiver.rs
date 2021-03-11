use futures::{Stream, StreamExt, Future, FutureExt};
use tokio::sync::mpsc;
use futures::channel::oneshot;
use crate::utils::rtp::ParsedRtpPacket;
use crate::utils::rtcp::{RtcpPacket, RtcpReportBlock};
use crate::transport::{RtcpSender};
use futures::task::{Context, Poll};
use tokio::macros::support::Pin;
use crate::utils::{RtpPacketResendRequester, RtpPacketResendRequesterEvent};
use crate::utils::rtcp::packets::{RtcpPacketTransportFeedback, RtcpTransportFeedback, RtcpPacketSenderReport, SourceDescription, RtcpPacketExtendedReport, RtcpPacketPayloadFeedback, RtcpPayloadFeedback, RtcpPacketReceiverReport};
use crate::media::{InternalMediaTrack, ControlDataSendError, ReceiverStats};
use webrtc_sdp::media_type::SdpMedia;
use std::collections::{HashMap, VecDeque, BTreeMap};
use std::collections::hash_map::RandomState;
use slog::slog_trace;
use std::io::Cursor;
use byteorder::{BigEndian, WriteBytesExt};
use tokio::time::Duration;
/* Note: When looking at extensions https://github.com/zxcpoiu/webrtc/blob/ea3dddf1d0880e89d84a7e502f65c65993d4169d/modules/rtp_rtcp/source/rtp_packet_received.cc#L50 */

#[derive(Debug)]
pub enum MediaReceiverEvent {
    /// We've received some data
    DataReceived(ParsedRtpPacket),
    /// Some sequences have been lost.
    /// We'll not re-request these any more.
    DataLost(Vec<u16>),

    /// We've received the first data since ever or the last bye signal
    ReceiverActivated,
    /// We've received a bye signal
    ByeSignalReceived(Option<String>),

    /// The sender violated the payload bandwidth limit.
    /// The first element contains the number of bytes send per minute.
    /// Note: FF might send more data if the bandwidth is bellow 2MBps
    BandwidthLimitViolation(f64)
}

pub struct MediaReceiver {
    logger: slog::Logger,

    /// The stream ssrc
    id: u32,
    /// Unique media line index
    media_line: u32,

    statistics: ReceiverStats,
    activated: bool,

    bandwidth_limit: Option<u32>,

    events: mpsc::UnboundedReceiver<InternalReceiverEvent>,
    event_queue: VecDeque<MediaReceiverEvent>,

    resend_requester: RtpPacketResendRequester,
    rtcp_sender: RtcpSender,

    timer_bandwidth_watcher: Option<tokio::time::Interval>
}

impl MediaReceiver {
    fn new(logger: slog::Logger, events: mpsc::UnboundedReceiver<InternalReceiverEvent>, ssrc: u32, media_line: u32, rtcp_sender: RtcpSender) -> Self {
        MediaReceiver{
            logger,
            id: ssrc,
            media_line,

            activated: false,
            statistics: ReceiverStats::new(),
            bandwidth_limit: None,

            events,
            event_queue: VecDeque::with_capacity(5),

            resend_requester: RtpPacketResendRequester::new(None),
            rtcp_sender,

            timer_bandwidth_watcher: None
        }
    }

    /// The media source id for the receiver
    pub fn ssrc(&self) -> u32 {
        self.id
    }

    /// Returns the media line unique id of this receiver
    pub fn media_line(&self) -> u32 {
        self.media_line
    }

    pub fn statistics(&self) -> &ReceiverStats {
        &self.statistics
    }

    /// Reset all pending resends
    pub fn reset_pending_resends(&mut self) {
        self.resend_requester.reset_resends();
    }

    pub fn resend_requester_mut(&mut self) -> &mut RtpPacketResendRequester {
        &mut self.resend_requester
    }

    pub fn send_control(&mut self, packet: &RtcpPacket) -> Result<(), ControlDataSendError> {
        self.rtcp_sender.send(packet);
        Ok(())
    }

    /// Returns the bandwidth limit signalled via REMB RTCP packets.
    pub fn bandwidth_limit(&self) -> &Option<u32> {
        &self.bandwidth_limit
    }

    /// Sets the bandwidth limit signalled via REMB RTCP packets.
    pub fn set_bandwidth_limit(&mut self, limit: Option<u32>) {
        if self.bandwidth_limit == limit {
            return;
        }

        let enforce_send = self.bandwidth_limit.is_some();
        self.bandwidth_limit = limit;

        if enforce_send || limit.is_some() {
            self.send_remb(self.bandwidth_limit.unwrap_or(50_000_000));
        }
    }

    fn send_remb(&mut self, bandwidth_limit: u32) {
        const MAX_MANTISSA: u32 = 0x3FFFF; /* 18 bits */
        const REMB_IDENTIFIER: u32 = 0x52454D42;  // 'R' 'E' 'M' 'B'.

        let mut buffer = [0u8; 255];
        let buffer_length: u64;

        {
            let mut writer = Cursor::new(&mut buffer[..]);
            writer.write_u32::<BigEndian>(REMB_IDENTIFIER).unwrap();
            writer.write_u8(1).unwrap();

            let mut mantissa = bandwidth_limit;
            let mut exponenta: u8 = 0;
            while mantissa > MAX_MANTISSA {
                mantissa >>= 1;
                exponenta += 1;
            }

            writer.write_u8((exponenta << 2) | (mantissa >> 16) as u8).unwrap();
            writer.write_u16::<BigEndian>((mantissa & 0xFFFF) as u16).unwrap();
            writer.write_u32::<BigEndian>(self.ssrc()).unwrap();

            buffer_length = writer.position();
        }

        let _ = self.send_control(&RtcpPacket::PayloadFeedback(RtcpPacketPayloadFeedback {
            media_ssrc: self.ssrc(),
            ssrc: 0, /* must be zero by rtf */
            feedback: RtcpPayloadFeedback::ApplicationSpecific(buffer[0..buffer_length as usize].to_vec())
        }));

        self.statistics.reset_bandwidth();
    }
}

enum InternalReceiverEvent {
    RtpPacket(ParsedRtpPacket),
    //RtcpPacket(RtcpPacket),
    RtcpSenderReport(RtcpPacketSenderReport),
    RtcpByeReceived(Option<String>),

    // Dummy event which will be send when checking if the receiver is still alive
    // (before generating a local description)
    FlushControl,
}

impl Stream for MediaReceiver {
    type Item = MediaReceiverEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(event) = self.event_queue.pop_front() {
            return Poll::Ready(Some(event));
        }

        while let Poll::Ready(Some(event)) = self.resend_requester.poll_next_unpin(cx) {
            match event {
                RtpPacketResendRequesterEvent::PacketTimedOut(packets) => {
                    return Poll::Ready(Some(MediaReceiverEvent::DataLost(packets.iter().map(|e| e.packet_id).collect())));
                },
                RtpPacketResendRequesterEvent::ResendPackets(packets) => {
                    let feedback = RtcpPacketTransportFeedback {
                        ssrc: 1,
                        media_ssrc: self.ssrc(),
                        feedback: RtcpTransportFeedback::create_generic_nack(packets.as_slice())
                    };

                    let mut reports = RtcpPacketReceiverReport{
                        ssrc: self.ssrc(),
                        profile_data: None,
                        reports: BTreeMap::new()
                    };
                    reports.reports.insert(self.ssrc(), RtcpReportBlock{
                        highest_sequence_received: self.resend_requester.last_packet_id() as u32,
                        cumulative_packets_lost: 0,
                        delay_since_last_sr: 0,
                        fraction_lost: 0,
                        jitter: 0,
                        timestamp_last_sr: 0
                    });

                    slog_trace!(self.logger, "Requesting packet resend for {} {:?}", self.ssrc(), packets.iter().map(|e| e.packet_id).collect::<Vec<_>>());
                    //self.rtcp_sender.send(&RtcpPacket::ReceiverReport(reports));
                    self.rtcp_sender.send(&RtcpPacket::TransportFeedback(feedback));
                },
                _ => {
                    slog_trace!(self.logger, "MediaReceiver::RtpPacketResendRequesterEvent {:?}", event);
                }
            }
        }

        if self.timer_bandwidth_watcher.is_none() {
            self.timer_bandwidth_watcher = Some(tokio::time::interval(Duration::from_secs(5)));
        }

        while let Poll::Ready(Some(_)) = self.timer_bandwidth_watcher.as_mut().unwrap().poll_next_unpin(cx) {
            self.statistics.tick();
            if let Some(max_bandwidth) = self.bandwidth_limit {
                let received_bandwidth = self.statistics.bandwidth_payload_minute();
                if received_bandwidth as u32 * 8 > max_bandwidth {
                    /* resend a remb maybe the old packet just got lost */
                    self.send_remb(max_bandwidth);

                    slog_trace!(self.logger, "MediaReceiver::BandwidthLimiter incoming bandwidth violation. Received {} bits/minute but expected {} bits/minute.", received_bandwidth * 8 as f64, max_bandwidth);
                    return Poll::Ready(Some(MediaReceiverEvent::BandwidthLimitViolation(received_bandwidth)));
                }

                //slog_trace!(self.logger, "Bandwidth Payload: Minute: {} Bps Second: {}Bps ", received_bandwidth * 8 as f64, self.statistics.bandwidth_payload_second() * 8);
            }
        }

        while let Poll::Ready(message) = self.events.poll_next_unpin(cx) {
            if message.is_none() {
                self.resend_requester.reset();
                return Poll::Ready(None);
            }

            match message.unwrap() {
                InternalReceiverEvent::RtpPacket(packet) => {
                    self.statistics.register_incoming_rtp(u16::from(packet.sequence_number()), packet.payload_offset(), packet.payload().len());
                    self.resend_requester.handle_packet_received(u16::from(packet.sequence_number()).into());

                    return if !self.activated {
                        self.activated = true;
                        if let Some(limit) = self.bandwidth_limit.clone() {
                            self.send_remb(limit);
                        }
                        self.event_queue.push_back(MediaReceiverEvent::DataReceived(packet));

                        Poll::Ready(Some(MediaReceiverEvent::ReceiverActivated))
                    } else {
                        Poll::Ready(Some(MediaReceiverEvent::DataReceived(packet)))
                    }
                },
                InternalReceiverEvent::RtcpByeReceived(reason) => {
                    self.activated = false;
                    self.statistics.reset();
                    return Poll::Ready(Some(MediaReceiverEvent::ByeSignalReceived(reason)));
                },
                InternalReceiverEvent::RtcpSenderReport(_report) => {
                    /* IDK Yet */
                },
                InternalReceiverEvent::FlushControl => {},
            }
        }

        Poll::Pending
    }
}

pub(crate) trait InternalMediaReceiver : Future<Output = ()> + Unpin {
    fn track(&self) -> &InternalMediaTrack;
    fn properties(&self) -> &HashMap<String, Option<String>>;

    fn parse_properties_from_sdp(&mut self, media: &SdpMedia);

    fn handle_rtp_packet(&mut self, packet: ParsedRtpPacket);
    fn handle_sender_report(&mut self, report: RtcpPacketSenderReport);
    fn handle_source_description(&mut self, description: &SourceDescription);
    fn handle_extended_report(&mut self, report: RtcpPacketExtendedReport);
    fn handle_bye(&mut self, reason: &Option<String>);

    /// This method will only be called, if dispatch_unknown_packets has been set on the peer connection
    fn handle_unknown_rtcp(&mut self, _data: &Vec<u8>);

    /// Return true if the receiver has been deleted
    fn flush_control(&mut self) -> bool;
    fn into_void(self: Box<Self>) -> Box<VoidInternalMediaReceiver>;
}

pub(crate) struct VoidInternalMediaReceiver {
    pub track: InternalMediaTrack,
    pub properties: HashMap<String, Option<String>>,
}

impl InternalMediaReceiver for VoidInternalMediaReceiver {
    fn track(&self) -> &InternalMediaTrack {
        &self.track
    }

    fn properties(&self) -> &HashMap<String, Option<String>, RandomState> {
        &self.properties
    }

    fn parse_properties_from_sdp(&mut self, media: &SdpMedia) {
        InternalMediaTrack::parse_properties_from_sdp(&mut self.properties, self.track.id, media);
    }

    fn handle_rtp_packet(&mut self, _packet: ParsedRtpPacket) {}

    fn handle_sender_report(&mut self, _report: RtcpPacketSenderReport) {}

    fn handle_source_description(&mut self, _description: &SourceDescription) {}

    fn handle_extended_report(&mut self, _report: RtcpPacketExtendedReport) {}

    fn handle_bye(&mut self, _reason: &Option<String>) { }

    fn handle_unknown_rtcp(&mut self, _data: &Vec<u8>) {}

    fn flush_control(&mut self) -> bool { false }

    fn into_void(self: Box<Self>) -> Box<VoidInternalMediaReceiver> {
        unreachable!();
    }
}

impl Future for VoidInternalMediaReceiver {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

pub(crate) struct ActiveInternalMediaReceiver {
    pub track: InternalMediaTrack,
    pub properties: HashMap<String, Option<String>>,

    event_sender: mpsc::UnboundedSender<InternalReceiverEvent>,
    close_channel_sender: Option<oneshot::Sender<()>>,
    close_channel_receiver: oneshot::Receiver<()>
}

impl ActiveInternalMediaReceiver {
    pub(crate) fn new(track: InternalMediaTrack, rtcp_sender: RtcpSender) -> (ActiveInternalMediaReceiver, MediaReceiver) {
        let close_channel = oneshot::channel();
        let message_channel = mpsc::unbounded_channel();

        let receiver = MediaReceiver::new(track.logger.clone(), message_channel.1, track.id, track.media_line, rtcp_sender);

        (
            ActiveInternalMediaReceiver {
                track,
                properties: HashMap::new(),

                event_sender: message_channel.0,
                close_channel_sender: Some(close_channel.0),
                close_channel_receiver: close_channel.1
            },
            receiver
        )
    }

    fn send_control_packet(&mut self, packet: InternalReceiverEvent) -> bool {
        if self.event_sender.send(packet).is_err() {
            let _ = self.close_channel_sender.take()
                .map(|sender| sender.send(()));
            false
        } else {
            true
        }
    }
}

impl InternalMediaReceiver for ActiveInternalMediaReceiver {
    fn track(&self) -> &InternalMediaTrack {
        &self.track
    }

    fn properties(&self) -> &HashMap<String, Option<String>, RandomState> {
        &self.properties
    }

    fn parse_properties_from_sdp(&mut self, media: &SdpMedia) {
        InternalMediaTrack::parse_properties_from_sdp(&mut self.properties, self.track.id, media);
    }

    fn handle_rtp_packet(&mut self, packet: ParsedRtpPacket) {
        self.send_control_packet(InternalReceiverEvent::RtpPacket(packet));
    }

    fn handle_sender_report(&mut self, report: RtcpPacketSenderReport) {
        self.send_control_packet(InternalReceiverEvent::RtcpSenderReport(report));
    }

    fn handle_source_description(&mut self, _description: &SourceDescription) {}

    fn handle_extended_report(&mut self, _report: RtcpPacketExtendedReport) { }

    fn handle_bye(&mut self, reason: &Option<String>) {
        self.send_control_packet(InternalReceiverEvent::RtcpByeReceived(reason.clone()));
    }

    fn handle_unknown_rtcp(&mut self, _data: &Vec<u8>) {}

    fn flush_control(&mut self) -> bool {
        !self.send_control_packet(InternalReceiverEvent::FlushControl)
    }

    fn into_void(self: Box<Self>) -> Box<VoidInternalMediaReceiver> {
        Box::new(VoidInternalMediaReceiver {
            properties: self.properties,
            track: self.track
        })
    }
}

impl Future for ActiveInternalMediaReceiver {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.close_channel_receiver.poll_unpin(cx).map(|_| ())
    }
}