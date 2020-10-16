use tokio::sync::mpsc;
use crate::utils::rtcp::RtcpPacket;
use crate::transport::RTCTransportControl;
use futures::{StreamExt, Stream};
use std::task::Context;
use futures::task::Poll;
use std::collections::HashMap;
use webrtc_sdp::media_type::SdpMedia;
use crate::media::{InternalMediaTrack, ControlDataSendError};
use crate::utils::rtcp::packets::{RtcpPacketReceiverReport, RtcpTransportFeedback, RtcpPayloadFeedback};
use tokio::macros::support::Pin;

/*
    TODO: Over all goal:
          Having a send method which accepts Vec<u8>, a timestamp, the marked flag, payload type and csrc,
          Everything else like the ssrc, sequence id, resending etc will be handled by InternalMediaSender.
 */

pub enum MediaSenderEvent {
    /// We've received a control packet
    RtcpPacketReceived(RtcpPacket),
}

pub(crate) enum MediaSenderControl {
    SendRtcpData(Vec<u8>),
    SendRtpData(Vec<u8>),
}

pub struct MediaSender {
    pub id: u32,

    pub(crate) events: mpsc::UnboundedReceiver<MediaSenderEvent>,
    pub(crate) control: mpsc::UnboundedSender<MediaSenderControl>
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
}

impl Stream for MediaSender {
    type Item = MediaSenderEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.poll_next_unpin(cx)
    }
}

pub(crate) struct InternalMediaSender {
    pub track: InternalMediaTrack,

    pub events: mpsc::UnboundedSender<MediaSenderEvent>,
    pub control: Option<mpsc::UnboundedReceiver<MediaSenderControl>>,
}

impl InternalMediaSender {
    pub fn handle_receiver_report(&mut self, _report: RtcpPacketReceiverReport) {}

    pub fn handle_transport_feedback(&mut self, _feedback: RtcpTransportFeedback) {}

    pub fn handle_payload_feedback(&mut self, _feedback: RtcpPayloadFeedback) {}

    pub fn handle_unknown_rtcp(&mut self, _data: &Vec<u8>) {}

    pub fn poll_control(&mut self, cx: &mut Context<'_>) {
        if let Some(control) = &mut self.control {
            while let Poll::Ready(message) = control.poll_next_unpin(cx) {
                if message.is_none() {
                    self.control = None;
                    break;
                }

                match message.unwrap() {
                    MediaSenderControl::SendRtpData(data) => {
                        /* TODO: Statistics */
                        let _ = self.track.transport.send(RTCTransportControl::SendRtpMessage(data));
                    },
                    MediaSenderControl::SendRtcpData(data) => {
                        let _ = self.track.transport.send(RTCTransportControl::SendRtcpMessage(data));
                    }
                }
            }
        }
    }
}