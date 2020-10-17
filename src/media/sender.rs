use tokio::sync::mpsc;
use crate::utils::rtcp::{RtcpPacket, RtcpReportBlock};
use crate::transport::RTCTransportControl;
use futures::{StreamExt, Stream};
use std::task::Context;
use futures::task::Poll;
use crate::media::{InternalMediaTrack, ControlDataSendError, NegotiationState};
use crate::utils::rtcp::packets::{RtcpTransportFeedback, RtcpPayloadFeedback};
use tokio::macros::support::Pin;
use tokio::sync::mpsc::error::TryRecvError;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::ops::Deref;
use webrtc_sdp::media_type::SdpMedia;

/*
    TODO: Over all goal:
          Having a send method which accepts Vec<u8>, a timestamp, the marked flag, payload type and csrc,
          Everything else like the ssrc, sequence id, resending etc will be handled by InternalMediaSender.
 */

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
    SendRtpData(Vec<u8>),

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
    /// Send a RTP packet (the ssrc should be a local one!)
    pub fn send_data(&mut self, packet: Vec<u8>) -> Result<(), ()> {
        self.control.send(MediaSenderControl::SendRtpData(packet))
            .map_err(|_| ())
    }

    pub fn send_control(&mut self, packet: RtcpPacket) -> Result<(), ControlDataSendError> {
        let mut buffer = [0u8; 2048];
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

    pub events: mpsc::UnboundedSender<MediaSenderEvent>,
    pub control: Option<mpsc::UnboundedReceiver<MediaSenderControl>>,
}

impl InternalMediaSender {
    pub fn new(track: InternalMediaTrack) -> (InternalMediaSender, MediaSender) {
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
            properties: properties.clone()
        };

        let sender = MediaSender {
            control: ctx,
            events: rx,
            properties,
            id: internal_sender.track.id
        };

        (internal_sender, sender)
    }

    pub fn handle_receiver_report(&mut self, report: RtcpReportBlock, _appdata: &Option<Vec<u8>>) {

        let _ = self.events.send(MediaSenderEvent::ReceiverReportReceived(report));
    }

    pub fn handle_transport_feedback(&mut self, feedback: RtcpTransportFeedback) {

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
            MediaSenderControl::SendRtpData(data) => {
                /* TODO: Statistics */
                let _ = self.track.transport.send(RTCTransportControl::SendRtpMessage(data));
            },
            MediaSenderControl::SendRtcpData(data) => {
                let _ = self.track.transport.send(RTCTransportControl::SendRtcpMessage(data));
            },
            MediaSenderControl::PropertiesChanged => {
                /* TODO: test nego state */
            }
        }
    }

    fn handle_close(&mut self) {
        self.control = None;
    }
}