use futures::{Stream, StreamExt, Future};
use tokio::sync::mpsc;
use crate::utils::rtp::ParsedRtpPacket;
use crate::utils::rtcp::RtcpPacket;
use crate::transport::{RtcpSender};
use futures::task::{Context, Poll};
use tokio::macros::support::Pin;
use crate::utils::{RtpPacketResendRequester, RtpPacketResendRequesterEvent};
use crate::utils::rtcp::packets::{RtcpPacketTransportFeedback, RtcpTransportFeedback, RtcpPacketSenderReport, SourceDescription, RtcpPacketExtendedReport};
use crate::media::{InternalMediaTrack, ControlDataSendError};
use webrtc_sdp::media_type::SdpMedia;
use std::collections::HashMap;
use std::collections::hash_map::RandomState;

/* Note: When looking at extensions https://github.com/zxcpoiu/webrtc/blob/ea3dddf1d0880e89d84a7e502f65c65993d4169d/modules/rtp_rtcp/source/rtp_packet_received.cc#L50 */

#[derive(Debug)]
pub enum MediaReceiverEvent {
    /// We've received some data
    DataReceived(ParsedRtpPacket),
    /// Some sequences have been lost.
    /// We'll not re-request these any more.
    DataLost(Vec<u16>),
    /// We've received a control packet
    RtcpPacketReceived(RtcpPacket),
    /// We've received a bye signal
    ByeSignalReceived(Option<String>)
}

pub(crate) enum InternalReceiverEvent {
    MediaEvent(MediaReceiverEvent),
    //UpdateCodecAndExtensions(Vec<Codec>, Vec<SdpAttributeExtmap>)
}

pub(crate) enum InternalReceiverControl {
    SendRtcpPacket(Vec<u8>),
    ResetPendingResends,
}

pub struct MediaReceiver {
    pub id: u32,
    /// Unique media line index
    pub media_line: u32,

    pub(crate) events: mpsc::UnboundedReceiver<InternalReceiverEvent>,
    pub(crate) control: mpsc::UnboundedSender<InternalReceiverControl>
}

impl Stream for MediaReceiver {
    type Item = MediaReceiverEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        while let Poll::Ready(message) = self.events.poll_next_unpin(cx) {
            if message.is_none() {
                return Poll::Ready(None);
            }

            return match message.unwrap() {
                InternalReceiverEvent::MediaEvent(event) => {
                    Poll::Ready(Some(event))
                }
            }
        }

        Poll::Pending
    }
}

impl MediaReceiver {
    /// Reset all pending resends
    pub fn reset_pending_resends(&mut self) {
        let _ = self.control.send(InternalReceiverControl::ResetPendingResends);
    }

    pub fn send_control(&mut self, packet: &RtcpPacket) -> Result<(), ControlDataSendError> {
        let mut buffer = [0u8; 2048];
        let write_result = packet.write(&mut buffer);
        if let Err(error) = write_result {
            return Err(ControlDataSendError::BuildFailed(error));
        }

        if let Err(_) = self.control.send(InternalReceiverControl::SendRtcpPacket(buffer[0..write_result.unwrap()].to_vec())) {
            return Err(ControlDataSendError::SendFailed);
        }

        Ok(())
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

    pub event_sender: mpsc::UnboundedSender<InternalReceiverEvent>,
    pub control_receiver: mpsc::UnboundedReceiver<InternalReceiverControl>,

    pub resend_requester: RtpPacketResendRequester,
    pub rtcp_sender: RtcpSender,
}

impl ActiveInternalMediaReceiver {
    fn handle_control_message(&mut self, message: InternalReceiverControl) {
        match message {
            InternalReceiverControl::SendRtcpPacket(packet) => {
                self.rtcp_sender.send_rtcp(packet.as_slice());
            },
            InternalReceiverControl::ResetPendingResends => {
                self.resend_requester.reset_resends();
            }
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
        self.resend_requester.handle_packet_received(u16::from(packet.sequence_number()).into());
        let _ = self.event_sender.send(InternalReceiverEvent::MediaEvent(MediaReceiverEvent::DataReceived(packet)));
    }

    fn handle_sender_report(&mut self, _report: RtcpPacketSenderReport) {}

    fn handle_source_description(&mut self, _description: &SourceDescription) {}

    fn handle_extended_report(&mut self, _report: RtcpPacketExtendedReport) { }

    fn handle_bye(&mut self, reason: &Option<String>) {
        let _ = self.event_sender.send(InternalReceiverEvent::MediaEvent(MediaReceiverEvent::ByeSignalReceived(reason.clone())));
    }

    fn handle_unknown_rtcp(&mut self, _data: &Vec<u8>) {}

    fn flush_control(&mut self) -> bool {
        loop {
            match self.control_receiver.try_recv() {
                Ok(message) => self.handle_control_message(message),
                Err(mpsc::error::TryRecvError::Closed) => return true,
                _ => return false
            }
        }
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
        while let Poll::Ready(event) = self.resend_requester.poll_next_unpin(cx) {
            let event = event.expect("unexpected stream close");
            match event {
                RtpPacketResendRequesterEvent::PacketTimedOut(packets) => {
                    let _ = self.event_sender.send(InternalReceiverEvent::MediaEvent(MediaReceiverEvent::DataLost(packets.iter().map(|e| e.packet_id).collect())));
                },
                RtpPacketResendRequesterEvent::ResendPackets(packets) => {
                    let feedback = RtcpPacketTransportFeedback {
                        ssrc: 1,
                        media_ssrc: self.track.id,
                        feedback: RtcpTransportFeedback::create_generic_nack(packets.as_slice())
                    };

                    println!("Resending packets on {} {:?} -> {:?}", self.track.id, packets, &feedback);
                    self.rtcp_sender.send(&RtcpPacket::TransportFeedback(feedback));
                },
                _ => {
                    println!("InternalMediaReceiver::RtpPacketResendRequesterEvent {:?}", event);
                }
            }
        }

        while let Poll::Ready(message) = self.control_receiver.poll_next_unpin(cx) {
            if message.is_none() {
                /* receiver consumer is gone, shutdown this receiver */
                return Poll::Ready(());
            }

            self.handle_control_message(message.unwrap());
        }

        Poll::Pending
    }
}