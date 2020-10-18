use tokio::sync::mpsc;
use crate::utils::rtcp::{RtcpPacket, RtcpReportBlock};
use crate::transport::{RtpSender, RtcpSender};
use futures::{StreamExt, Stream, FutureExt};
use std::task::Context;
use futures::task::Poll;
use crate::media::{InternalMediaTrack, ControlDataSendError, NegotiationState};
use crate::utils::rtcp::packets::{RtcpTransportFeedback, RtcpPayloadFeedback, RtcpFeedbackGenericNACK};
use tokio::macros::support::Pin;
use tokio::sync::mpsc::error::TryRecvError;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::ops::Deref;
use webrtc_sdp::media_type::SdpMedia;
use rtp_rs::Seq;

#[derive(Debug)]
pub enum MediaSenderEvent {
    /// This event will only trigger if the option is enabled within the RTCPeer
    /// TODO: Add this option!
    UnknownRtcpPacketReceived(Vec<u8>),

    ReceiverReportReceived(RtcpReportBlock),
    TransportFeedbackReceived(RtcpTransportFeedback),
    PayloadFeedbackReceived(RtcpPayloadFeedback),
}

pub(crate) enum MediaSenderControl {
    SendRtcpData(Vec<u8>),
    SendRtpData(u16, Vec<u8>),

    PropertiesChanged
}

struct MediaSenderProperties {
    properties: HashMap<String, Option<String>>,
    negotiation_state: NegotiationState
}

impl MediaSenderProperties {
    pub fn new() -> Self {
        MediaSenderProperties {
            properties: HashMap::new(),
            negotiation_state: NegotiationState::None
        }
    }

    pub fn try_change(&mut self) -> bool {
        match self.negotiation_state {
            NegotiationState::None |
            NegotiationState::Changed => true,
            NegotiationState::Propagated => false,
            NegotiationState::Negotiated => {
                self.negotiation_state = NegotiationState::Changed;
                true
            }
        }
    }
}

pub struct MediaSender {
    pub id: u32,

    properties: Arc<Mutex<MediaSenderProperties>>,

    payload_type: u8,
    contributing_sources: Vec<u32>,

    current_sequence_number: u16,

    pub(crate) events: mpsc::UnboundedReceiver<MediaSenderEvent>,
    pub(crate) control: mpsc::UnboundedSender<MediaSenderControl>
}

pub struct LockedMediaSenderProperties<'a> {
    properties: MutexGuard<'a, MediaSenderProperties>,
}

impl Deref for LockedMediaSenderProperties<'_> {
    type Target = HashMap<String, Option<String>>;

    fn deref(&self) -> &Self::Target {
        &self.properties.properties
    }
}

impl MediaSender {
    pub fn payload_type(&self) -> u8 {
        self.payload_type
    }

    pub fn payload_type_mut(&mut self) -> &mut u8 {
        &mut self.payload_type
    }

    pub fn contributing_sources(&self) -> &Vec<u32> {
        &self.contributing_sources
    }

    pub fn contributing_sources_mut(&mut self) -> &mut Vec<u32> {
        &mut self.contributing_sources
    }

    pub fn send(&mut self, data: &[u8], marked: bool, timestamp: u32, extension: Option<(u16, &[u8])>) {
        let sequence_no = self.current_sequence_number;
        self.current_sequence_number = self.current_sequence_number.wrapping_add(1);

        let mut packet = rtp_rs::RtpPacketBuilder::new()
            .ssrc(self.id)
            .payload_type(self.payload_type)
            .marked(marked)
            .set_csrc(self.contributing_sources.as_slice())
            .sequence(Seq::from(sequence_no))
            .timestamp(timestamp)
            .payload(data);
        if let Some(extension) = extension {
            packet = packet.extension(extension.0, extension.1);
        }

        let mut buffer = unsafe { std::mem::MaybeUninit::<[u8; 2048]>::uninit().assume_init() };
        let size = packet.build_into_unchecked(&mut buffer);
        self.control.send(MediaSenderControl::SendRtpData(sequence_no, buffer[0..size].to_vec()));
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
        LockedMediaSenderProperties { properties: self.properties.lock().unwrap() }
    }

    pub fn register_property(&mut self, key: String, value: Option<String>) {
        /* you're not allowed to change the channel name */
        if key == "cname" { return; }

        let mut properties = self.properties.lock().unwrap();
        if !properties.try_change() { return; }

        properties.properties.insert(key.clone(), value.clone());
        let _ = self.control.send(MediaSenderControl::PropertiesChanged);
    }

    pub fn unset_property(&mut self, key: &String) {
        let mut properties = self.properties.lock().unwrap();
        if !properties.try_change() { return; }

        if properties.properties.remove(key).is_some() {
            if properties.negotiation_state != NegotiationState::None {
                properties.negotiation_state = NegotiationState::Changed;
            }
            let _ = self.control.send(MediaSenderControl::PropertiesChanged);
        }
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

    properties: Arc<Mutex<MediaSenderProperties>>,
    rtp_sender: RtpSender,
    rtcp_sender: RtcpSender,

    pub events: mpsc::UnboundedSender<MediaSenderEvent>,
    pub control: Option<mpsc::UnboundedReceiver<MediaSenderControl>>,
}

impl InternalMediaSender {
    pub fn new(track: InternalMediaTrack, sender: (RtpSender, RtcpSender)) -> (InternalMediaSender, MediaSender) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (ctx, crx) = mpsc::unbounded_channel();

        let mut properties = MediaSenderProperties::new();
        /* A channel name is required so we add one */
        properties.properties.insert(String::from("cname"), Some(String::from(format!("{}", track.id))));
        let properties = Arc::new(Mutex::new(properties));

        let internal_sender = InternalMediaSender {
            track,
            events: tx,
            control: Some(crx),
            properties: properties.clone(),

            rtp_sender: sender.0,
            rtcp_sender: sender.1
        };

        let sender = MediaSender {
            control: ctx,
            events: rx,
            properties,
            payload_type: 0,
            contributing_sources: vec![],
            current_sequence_number: 0,
            id: internal_sender.track.id
        };

        (internal_sender, sender)
    }

    pub fn negotiation_needed(&self) -> bool {
        let properties = self.properties.lock().unwrap();
        matches!(&properties.negotiation_state, NegotiationState::None | NegotiationState::Changed)
    }

    pub fn promote_negotiation<T>(&mut self, test: T, value: NegotiationState)
        where T: Fn(NegotiationState) -> bool
    {
        let mut properties = self.properties.lock().unwrap();
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

    pub fn handle_unknown_rtcp(&mut self, data: &Vec<u8>) {

        let _ = self.events.send(MediaSenderEvent::UnknownRtcpPacketReceived(data.clone()));
    }

    pub fn write_sdp(&mut self, media: &mut SdpMedia) {
        let properties = self.properties.lock().unwrap();
        InternalMediaTrack::write_sdp(&properties.properties, self.track.id, media);
    }

    pub fn poll_control(&mut self, cx: &mut Context<'_>) {
        let _ = self.rtp_sender.poll_unpin(cx);

        while let Some(Poll::Ready(message)) = self.control.as_mut().map(|ctrl| ctrl.poll_next_unpin(cx)) {
            if message.is_none() {
                self.handle_close();
                break;
            }

            self.handle_control(message.unwrap());
        }
    }

    pub fn flush_control(&mut self) {
        loop {
            match self.control.as_mut().map(|e| e.try_recv()) {
                Some(Ok(message)) => self.handle_control(message),
                Some(Err(TryRecvError::Empty)) |
                None => {
                    return
                },
                Some(Err(TryRecvError::Closed)) => {
                    self.handle_close();
                    return
                },
            }
        }
    }

    fn handle_control(&mut self, message: MediaSenderControl) {
        match message {
            MediaSenderControl::SendRtpData(sequence, data) => {
                /* TODO: Statistics */
                self.rtp_sender.send_rtp(sequence, data.as_slice());
            },
            MediaSenderControl::SendRtcpData(data) => {
                self.rtcp_sender.send_rtcp(data.as_slice());
            },
            MediaSenderControl::PropertiesChanged => {
                /* TODO: test nego state */
            }
        }
    }

    fn handle_close(&mut self) {
        self.control = None;
    }

    fn process_generic_nack(&mut self, nack: &RtcpFeedbackGenericNACK) {
        for packet_id in nack.lost_packets() {
            self.rtp_sender.retransmit_rtp(packet_id);
        }
    }

}