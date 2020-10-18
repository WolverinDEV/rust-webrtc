use futures::{Stream, StreamExt};
use tokio::sync::mpsc;
use crate::utils::rtp::ParsedRtpPacket;
use crate::utils::rtcp::RtcpPacket;
use crate::transport::{RtcpSender};
use futures::task::{Context, Poll};
use tokio::macros::support::Pin;
use crate::utils::{RtpPacketResendRequester, RtpPacketResendRequesterEvent};
use crate::utils::rtcp::packets::{RtcpPacketTransportFeedback, RtcpTransportFeedback, RtcpPacketSenderReport, SourceDescription};
use crate::media::{InternalMediaTrack, ControlDataSendError};
use webrtc_sdp::media_type::SdpMedia;
use std::collections::HashMap;

/* TODO: When looking at extensions https://github.com/zxcpoiu/webrtc/blob/ea3dddf1d0880e89d84a7e502f65c65993d4169d/modules/rtp_rtcp/source/rtp_packet_received.cc#L50 */

#[derive(Debug)]
pub enum MediaReceiverEvent {
    /// We've received some data
    DataReceived(ParsedRtpPacket),
    /// Some sequences have been lost.
    /// We'll not re-request these any more.
    DataLost(Vec<u16>),
    /// We've received a control packet
    RtcpPacketReceived(RtcpPacket),
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

    pub fn send_control(&mut self, packet: RtcpPacket) -> Result<(), ControlDataSendError> {
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


pub(crate) struct InternalMediaReceiver {
    pub track: InternalMediaTrack,

    pub properties: HashMap<String, Option<String>>,

    pub event_sender: mpsc::UnboundedSender<InternalReceiverEvent>,
    pub control_receiver: mpsc::UnboundedReceiver<InternalReceiverControl>,

    pub resend_requester: RtpPacketResendRequester,
    pub rtcp_sender: RtcpSender,
}

impl InternalMediaReceiver {
    pub fn handle_rtp_packet(&mut self, packet: ParsedRtpPacket) {
        self.resend_requester.handle_packet_received(u16::from(packet.sequence_number()).into());
        let _ = self.event_sender.send(InternalReceiverEvent::MediaEvent(MediaReceiverEvent::DataReceived(packet)));
    }

    pub fn handle_sender_report(&mut self, _report: RtcpPacketSenderReport) {}

    pub fn handle_source_description(&mut self, _description: &SourceDescription) {}

    pub fn handle_unknown_rtcp(&mut self, _data: &Vec<u8>) {}

    pub fn parse_properties_from_sdp(&mut self, media: &SdpMedia) {
        InternalMediaTrack::parse_properties_from_sdp(&mut self.properties, self.track.id, media);
    }

    pub fn poll_control(&mut self, cx: &mut Context<'_>) {
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
            self.handle_control_message(message.expect("unexpected stream ending"));
        }
    }

    pub fn flush_control(&mut self) {
        while let Ok(message) = self.control_receiver.try_recv() {
            self.handle_control_message(message);
        }
    }

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