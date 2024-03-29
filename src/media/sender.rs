use tokio::sync::mpsc;
use crate::utils::rtcp::{RtcpPacket, RtcpReportBlock};
use crate::transport::{RtpSender, RtcpSender};
use futures::{StreamExt, Stream, FutureExt};
use std::task::Context;
use futures::task::Poll;
use crate::media::{InternalMediaTrack, ControlDataSendError, NegotiationState, Codec};
use crate::utils::rtcp::packets::{RtcpTransportFeedback, RtcpPayloadFeedback, RtcpFeedbackGenericNACK, RtcpPacketBye, RtcpPacketExtendedReport};
use tokio::macros::support::Pin;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::ops::Deref;
use webrtc_sdp::media_type::SdpMedia;
use rtp_rs::Seq;
use std::future::Future;
use slog::slog_trace;

#[derive(Debug)]
pub enum MediaSenderEvent {
    UnknownRtcpPacketReceived(Vec<u8>),

    ReceiverReportReceived(RtcpReportBlock),
    TransportFeedbackReceived(RtcpTransportFeedback),
    PayloadFeedbackReceived(RtcpPayloadFeedback),
    ExtendedReportReceived(RtcpPacketExtendedReport),

    /// The remote supported codecs have been updated.
    /// This could have two reasons:
    /// 1. We initially received the remote codecs
    /// 2. The remote has narrowed their supported codecs (Not sure if this is even allowed by WebRTC)
    RemoteCodecsUpdated
}

pub(crate) enum MediaSenderControl {
    SendRtcpData(Vec<u8>),
    SendRtpData(u16, Vec<u8>),

    PropertiesChanged
}

pub(crate) struct MediaSenderSharedData {
    pub properties: HashMap<String, Option<String>>,
    pub remote_codecs: Option<Vec<Codec>>,
    pub negotiation_state: NegotiationState
}

impl MediaSenderSharedData {
    pub fn new() -> Self {
        MediaSenderSharedData {
            properties: HashMap::new(),
            remote_codecs: None,
            negotiation_state: NegotiationState::None
        }
    }

    pub fn try_change(&mut self, change_state: bool) -> bool {
        match self.negotiation_state {
            NegotiationState::None |
            NegotiationState::Changed => true,
            NegotiationState::Propagated => false,
            NegotiationState::Negotiated => {
                if change_state {
                    self.negotiation_state = NegotiationState::Changed;
                }
                true
            }
        }
    }
}

pub struct MediaSender {
    id: u32,
    shared_data: Arc<Mutex<MediaSenderSharedData>>,

    current_payload_type: u8,
    current_contributing_sources: Vec<u32>,
    current_sequence_number: u16,
    last_send_timestamp: u32,

    pub(crate) events: mpsc::UnboundedReceiver<MediaSenderEvent>,
    pub(crate) control: mpsc::UnboundedSender<MediaSenderControl>
}

pub struct LockedMediaSenderProperties<'a>(MutexGuard<'a, MediaSenderSharedData>);
impl Deref for LockedMediaSenderProperties<'_> {
    type Target = HashMap<String, Option<String>>;

    fn deref(&self) -> &Self::Target {
        &self.0.properties
    }
}

pub struct LockedMediaSenderRemoteCodecs<'a>(MutexGuard<'a, MediaSenderSharedData>);

impl Deref for LockedMediaSenderRemoteCodecs<'_> {
    type Target = Option<Vec<Codec>>;

    fn deref(&self) -> &Self::Target {
        &self.0.remote_codecs
    }
}

impl MediaSender {
    /// Get the [ssrc] of the sender.
    pub fn id(&self) -> u32 { self.id }

    pub fn payload_type(&self) -> u8 {
        self.current_payload_type
    }

    pub fn payload_type_mut(&mut self) -> &mut u8 {
        &mut self.current_payload_type
    }

    pub fn contributing_sources(&self) -> &Vec<u32> {
        &self.current_contributing_sources
    }

    pub fn contributing_sources_mut(&mut self) -> &mut Vec<u32> {
        &mut self.current_contributing_sources
    }

    pub fn current_sequence_no(&self) -> u16 {
        self.current_sequence_number
    }

    pub fn current_sequence_no_mut(&mut self) -> &mut u16 {
        &mut self.current_sequence_number
    }

    pub fn last_send_timestamp(&self) -> u32 { self.last_send_timestamp }

    pub fn send(&mut self, data: &[u8], marked: bool, timestamp: u32, extension: Option<(u16, &[u8])>) {
        let sequence_no = self.current_sequence_number;
        self.current_sequence_number = self.current_sequence_number.wrapping_add(1);

        self.send_seq(data, sequence_no, marked, timestamp, extension);
    }

    pub fn send_seq(&mut self, data: &[u8], seq_no: u16, marked: bool, timestamp: u32, extension: Option<(u16, &[u8])>) {
        self.last_send_timestamp = timestamp;

        let mut packet = rtp_rs::RtpPacketBuilder::new()
            .ssrc(self.id)
            .payload_type(self.current_payload_type)
            .marked(marked)
            .set_csrc(self.current_contributing_sources.as_slice())
            .sequence(Seq::from(seq_no))
            .timestamp(timestamp)
            .payload(data);
        if let Some(extension) = extension {
            packet = packet.extension(extension.0, extension.1);
        }

        let mut buffer = unsafe { std::mem::MaybeUninit::<[u8; 2048]>::uninit().assume_init() };
        let size = packet.build_into_unchecked(&mut buffer);
        let _ = self.control.send(MediaSenderControl::SendRtpData(seq_no, buffer[0..size].to_vec()));
    }

    pub fn send_control(&mut self, packet: RtcpPacket) -> Result<(), ControlDataSendError> {
        let mut buffer = unsafe { std::mem::MaybeUninit::<[u8; 2048]>::uninit().assume_init() };
        let write_result = packet.write(&mut buffer);
        if let Err(error) = write_result {
            return Err(ControlDataSendError::BuildFailed(error));
        }

        if let Err(_) = self.control.send(MediaSenderControl::SendRtcpData(buffer[0..write_result.unwrap()].to_vec())) {
            return Err(ControlDataSendError::SendFailed);
        }

        Ok(())
    }

    pub fn properties(&self) -> LockedMediaSenderProperties {
        LockedMediaSenderProperties(self.shared_data.lock().unwrap())
    }

    pub fn remote_codecs(&self) -> LockedMediaSenderRemoteCodecs {
        LockedMediaSenderRemoteCodecs(self.shared_data.lock().unwrap())
    }

    pub fn register_property(&mut self, key: String, value: Option<String>) {
        /* you're not allowed to change the channel name */
        if key == "cname" { return; }

        let mut properties = self.shared_data.lock().unwrap();
        if !properties.try_change(true) { return; }

        properties.properties.insert(key.clone(), value.clone());
        /* trigger a event, so the peer will look for changes */
        let _ = self.control.send(MediaSenderControl::PropertiesChanged);
    }

    pub fn unset_property(&mut self, key: &String) {
        let mut properties = self.shared_data.lock().unwrap();
        if !properties.try_change(false) { return; }

        if properties.properties.remove(key).is_some() {
            properties.try_change(true);
        }
        /* trigger a event, so the peer will look for changes */
        let _ = self.control.send(MediaSenderControl::PropertiesChanged);
    }
}

impl Stream for MediaSender {
    type Item = MediaSenderEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.poll_next_unpin(cx)
    }
}

pub(crate) struct InternalMediaSender {
    pub track: InternalMediaTrack,

    pub shared_data: Arc<Mutex<MediaSenderSharedData>>,
    rtp_sender: RtpSender,
    rtcp_sender: RtcpSender,

    pub events: mpsc::UnboundedSender<MediaSenderEvent>,
    pub control: mpsc::UnboundedReceiver<MediaSenderControl>,
}

impl InternalMediaSender {
    pub fn new(track: InternalMediaTrack, sender: (RtpSender, RtcpSender)) -> (InternalMediaSender, MediaSender) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (ctx, crx) = mpsc::unbounded_channel();

        let mut properties = MediaSenderSharedData::new();
        /* A channel name is required so we add one */
        properties.properties.insert(String::from("cname"), Some(String::from(format!("{}", track.id))));
        /* we're feeling so free to add a unique media stream as well */
        properties.properties.insert(String::from("msid"), Some(String::from(format!("{} -", track.id))));

        let properties = Arc::new(Mutex::new(properties));

        let internal_sender = InternalMediaSender {
            track,
            events: tx,
            control: crx,
            shared_data: properties.clone(),

            rtp_sender: sender.0,
            rtcp_sender: sender.1
        };

        let sender = MediaSender {
            control: ctx,
            events: rx,
            shared_data: properties,
            current_payload_type: 0,
            current_contributing_sources: vec![],
            current_sequence_number: 0,
            last_send_timestamp: 0,
            id: internal_sender.track.id
        };

        (internal_sender, sender)
    }

    pub fn negotiation_needed(&self) -> bool {
        let properties = self.shared_data.lock().unwrap();
        matches!(&properties.negotiation_state, NegotiationState::None | NegotiationState::Changed)
    }

    pub fn promote_negotiation<T>(&mut self, test: T, value: NegotiationState)
        where T: Fn(NegotiationState) -> bool
    {
        let mut properties = self.shared_data.lock().unwrap();
        if test(properties.negotiation_state) {
            properties.negotiation_state = value;
        }
    }

    pub fn handle_receiver_report(&mut self, report: RtcpReportBlock, _appdata: &Option<Vec<u8>>) {

        let _ = self.events.send(MediaSenderEvent::ReceiverReportReceived(report));
    }

    pub fn handle_transport_feedback(&mut self, feedback: RtcpTransportFeedback) {
        match &feedback {
            RtcpTransportFeedback::GenericNACK(nacks) => {
                nacks.iter().for_each(|nack| self.process_generic_nack(nack));
            }
        }

        let _ = self.events.send(MediaSenderEvent::TransportFeedbackReceived(feedback));
    }

    pub fn handle_payload_feedback(&mut self, feedback: RtcpPayloadFeedback) {

        let _ = self.events.send(MediaSenderEvent::PayloadFeedbackReceived(feedback));
    }

    #[allow(dead_code)]
    pub fn handle_extended_report(&mut self, report: RtcpPacketExtendedReport) {

        let _ = self.events.send(MediaSenderEvent::ExtendedReportReceived(report));
    }

    pub fn handle_unknown_rtcp(&mut self, data: &Vec<u8>) {

        let _ = self.events.send(MediaSenderEvent::UnknownRtcpPacketReceived(data.clone()));
    }

    pub fn write_sdp(&mut self, media: &mut SdpMedia) {
        let properties = self.shared_data.lock().unwrap();
        InternalMediaTrack::write_sdp(&properties.properties, self.track.id, media);
    }

    /// Returns true if the sender has reached his end of life
    pub fn flush_control(&mut self) -> bool {
        loop {
            match self.control.try_recv() {
                Ok(message) => self.handle_control_message(message),
                Err(mpsc::error::TryRecvError::Closed) => return true,
                _ => return false
            }
        }
    }

    fn handle_control_message(&mut self, message: MediaSenderControl) {
        match message {
            MediaSenderControl::SendRtpData(sequence, data) => {
                /* TODO: Statistics */
                self.rtp_sender.send_rtp(sequence, data.as_slice());
            },
            MediaSenderControl::SendRtcpData(data) => {
                self.rtcp_sender.send_rtcp(data.as_slice());
            },
            MediaSenderControl::PropertiesChanged => { /* event already full filled it's purpose */}
        }
    }

    fn process_generic_nack(&mut self, nack: &RtcpFeedbackGenericNACK) {
        for packet_id in nack.lost_packets() {
            self.rtp_sender.retransmit_rtp(packet_id);
        }
    }
}

impl Future for InternalMediaSender {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let _ = self.rtp_sender.poll_unpin(cx);

        while let Poll::Ready(message) = self.control.poll_next_unpin(cx) {
            if message.is_none() {
                slog_trace!(self.track.logger, "Media sender has been closed (dropped)");

                let packet = RtcpPacket::Bye(RtcpPacketBye{
                    reason: None,
                    src: vec![self.track.id]
                });
                self.rtcp_sender.send(&packet);

                return Poll::Ready(());
            }

            self.handle_control_message(message.unwrap());
        }

        Poll::Pending
    }
}