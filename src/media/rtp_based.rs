#![allow(dead_code)]

use crate::media::{MediaSource, Codec, MediaChannelIncomingEvent};
use crate::ice::{PeerICEConnectionControl};
use webrtc_sdp::media_type::{SdpMedia, SdpMediaLine, SdpMediaValue, SdpFormatList, SdpProtocolValue};
use crate::rtc::MediaId;
use webrtc_sdp::SdpConnection;
use webrtc_sdp::address::ExplicitlyTypedAddress;
use std::net::{IpAddr, Ipv4Addr};
use tokio::sync::mpsc;
use futures::{Stream, StreamExt};
use futures::task::{Context, Poll};
use tokio::macros::support::Pin;
use webrtc_sdp::attribute_type::{SdpAttribute, SdpAttributeDirection, SdpAttributeType, SdpAttributeExtmap};
use std::fmt::Debug;
use crate::utils::rtcp::RtcpPacket;
use std::collections::{HashMap, BTreeMap};
use crate::utils::rtp::ParsedRtpPacket;
use crate::utils::{RtpPacketResendRequester, SequenceNumber, RtpPacketResendRequesterEvent};
use crate::utils::rtcp::packets::{RtcpPacketTransportFeedback, RtcpTransportFeedback};
use std::cell::RefCell;
use std::io::Error;

/* TODO: When looking at extensions https://github.com/zxcpoiu/webrtc/blob/ea3dddf1d0880e89d84a7e502f65c65993d4169d/modules/rtp_rtcp/source/rtp_packet_received.cc#L50 */

struct ReceivingMediaSource {
    /// The remote media stream id
    id: u32,
    resend_requester: RefCell<RtpPacketResendRequester>,
}

#[derive(Debug)]
pub enum MediaChannelRtpBasedEvents {
    /// We've received some data
    DataReceived(ParsedRtpPacket),
    /// Some data has been lost and will not be re-requested by generic nacks any more
    DataLost(Vec<u16>),
    /// We've received a control packet
    RtcpPacketReceived(RtcpPacket),
}

pub enum ControlDataSendError {
    BuildFailed(Error),
    SendFailed
}

pub struct MediaChannelRtpBased {
    media_type: SdpMediaValue,
    media_id: MediaId,
    direction: SdpAttributeDirection,

    ice_control: mpsc::UnboundedSender<PeerICEConnectionControl>,

    remote_codes: Vec<Codec>,
    local_codecs: Vec<Codec>,

    remote_sources: Vec<MediaSource>,
    local_sources: Vec<MediaSource>,

    remote_extensions: Vec<SdpAttributeExtmap>,
    local_extensions: Vec<SdpAttributeExtmap>,

    event_sender: mpsc::UnboundedSender<MediaChannelRtpBasedEvents>,
    event_receiver: mpsc::UnboundedReceiver<MediaChannelRtpBasedEvents>,

    receiving_sources: BTreeMap<u32, ReceivingMediaSource>,
}

impl MediaChannelRtpBased {
    pub fn new(media_type: SdpMediaValue, media_id: MediaId, ice_control: mpsc::UnboundedSender<PeerICEConnectionControl>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        MediaChannelRtpBased {
            media_type,
            media_id,
            direction: SdpAttributeDirection::Sendrecv,

            ice_control,

            remote_codes: Vec::new(),
            local_codecs: Vec::new(),

            remote_sources: Vec::new(),
            local_sources: Vec::new(),

            remote_extensions: Vec::new(),
            local_extensions: Vec::new(),

            event_sender: tx,
            event_receiver: rx,

            receiving_sources: BTreeMap::new()
        }
    }

    /// Get all remote supported codecs.
    pub fn remote_codecs(&self) -> &Vec<Codec> {
        &self.remote_codes
    }

    /// Get all locally supported codecs.
    pub fn local_codecs(&self) -> &Vec<Codec> {
        &self.local_codecs
    }

    /// Get all locally supported codecs.
    /// Modifying this map will only have an effect before generating an answer.
    pub fn local_codecs_mut(&mut self) -> &mut Vec<Codec> {
        &mut self.local_codecs
    }

    /// Get all remote sources
    pub fn remote_sources(&self) -> &Vec<MediaSource> {
        &self.remote_sources
    }

    /// Get all local sources
    pub fn local_sources(&self) -> &Vec<MediaSource> {
        &self.local_sources
    }

    /// Get all local sources
    pub fn local_sources_mut(&mut self) -> Vec<&mut MediaSource> {
        self.local_sources.iter_mut().collect()
    }

    /// Register a new local source.
    /// This should be done before generating an answer.
    /// If not the local source just gets ignored.
    pub fn register_local_source(&mut self) -> &mut MediaSource {
        let source = MediaSource {
            id: rand::random::<u32>(),
            properties: HashMap::new()
        };
        self.local_sources.push(source);
        self.local_sources.last_mut().unwrap()
    }

    /// Get all remote supported extensions
    pub fn remote_extensions(&self) -> &Vec<SdpAttributeExtmap> {
        &self.remote_extensions
    }

    /// Get all local registered extensions
    pub fn local_extensions(&self) -> &Vec<SdpAttributeExtmap> {
        &self.local_extensions
    }

    /// Get all local registered extensions
    pub fn local_extensions_mut(&mut self) -> &mut Vec<SdpAttributeExtmap> {
        &mut self.local_extensions
    }

    /// Send a RTP packet (the ssrc should be a local one!)
    pub fn send_data(&mut self, packet: Vec<u8>) -> Result<(), ()> {
        self.ice_control.send(PeerICEConnectionControl::SendRtpMessage(packet))
            .map_err(|_| ())
    }

    /// Send a control packet
    pub fn send_control_data(&mut self, packet: &RtcpPacket) -> Result<(), ControlDataSendError> {
        self.send_control_data_internal(packet)
    }

    /// Reset all pending resends for the target stream
    pub fn reset_pending_resends(&mut self, stream: u32) {
        self.receiving_sources.get_mut(&stream)
            .map(|stream| stream.resend_requester.get_mut().reset_resends());
    }

    fn send_control_data_internal(&self, packet: &RtcpPacket) -> Result<(), ControlDataSendError> {
        let mut buffer = [0u8; 2048];
        let write_result = packet.write(&mut buffer);
        if let Err(error) = write_result {
            return Err(ControlDataSendError::BuildFailed(error));
        }

        if let Err(_) = self.ice_control.send(PeerICEConnectionControl::SendRtcpMessage(buffer[0..write_result.unwrap()].to_vec())) {
            return Err(ControlDataSendError::SendFailed);
        }

        Ok(())
    }
}

impl MediaChannelRtpBased {
    pub fn media_id(&self) -> &MediaId {
        &self.media_id
    }

    pub fn configure(&mut self, media: &SdpMedia) -> Result<(), String> {
        if let SdpFormatList::Integers(formats) = media.get_formats() {
            self.remote_codes.clear();
            self.remote_codes.reserve(formats.len());
            for format in formats.iter() {
                let codec = Codec::from_sdp(*format as u8, media);
                if codec.is_none() {
                    return Err(String::from(format!("Missing codec info for format {}", format)));
                }

                self.remote_codes.push(codec.unwrap());
            }
        } else {
            return Err(String::from("Expected an integer format list"));
        }

        if media.get_attribute(SdpAttributeType::Recvonly).is_none() {
            for source in media.get_attributes_of_type(SdpAttributeType::Ssrc)
                .iter()
                .map(|e| if let SdpAttribute::Ssrc(data) = e { data } else { panic!() })
            {
                if self.remote_sources.iter().find(|e| e.id == source.id).is_some() {
                    continue;
                }

                self.remote_sources.push(MediaSource::parse_sdp(source.id, media));
                self.receiving_sources.insert(source.id, ReceivingMediaSource{
                    id: source.id,
                    resend_requester: RefCell::new(RtpPacketResendRequester::new())
                });
            }
        }

        self.remote_extensions = media.get_attributes_of_type(SdpAttributeType::Extmap)
            .iter()
            .map(|e| if let SdpAttribute::Extmap(data) = e { data.clone() } else { panic!() })
            .collect();

        println!("Remote codecs: {:?}", self.remote_codes);
        println!("Remote streams: {:?}", self.remote_sources);
        Ok(())
    }

    pub fn generate_sdp(&self) -> Result<SdpMedia, String> {
        if self.local_codecs.is_empty() {
            return Err(String::from("No local codes specified."));
        }

        let mut media = SdpMedia::new(SdpMediaLine {
            media: self.media_type.clone(),
            formats: SdpFormatList::Integers(self.local_codecs.iter().map(|e| e.payload_type as u32).collect()),
            port: 9,
            port_count: 0,
            proto: SdpProtocolValue::UdpTlsRtpSavpf
        });

        for codec in self.local_codecs.iter() {
            codec.append_to_sdp(&mut media)
                .map_err(|err| format!("failed to add codec {}: {}", codec.payload_type, err))?;
        }

        media.set_connection(SdpConnection{
            address: ExplicitlyTypedAddress::Ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            ttl: None,
            amount: None
        }).unwrap();
        media.add_attribute(SdpAttribute::RtcpMux).unwrap();

        if self.local_sources.is_empty() {
            media.add_attribute(SdpAttribute::Recvonly).unwrap();
        } else if self.remote_sources.is_empty() {
            media.add_attribute(SdpAttribute::Sendonly).unwrap();
        } else {
            media.add_attribute(SdpAttribute::Sendrecv).unwrap();
        }

        for source in self.local_sources.iter() {
            source.write_sdp(&mut media);
        }

        self.local_extensions.iter().for_each(|ext| media.add_attribute(SdpAttribute::Extmap(ext.clone())).unwrap());

        Ok(media)
    }

    pub fn process_peer_event(&mut self, event: &mut Option<MediaChannelIncomingEvent>) {
        match event.as_ref().unwrap() {
            MediaChannelIncomingEvent::RtpPacketReceived(packet) => {
                if let Some(receiver) = self.receiving_sources.get_mut(&packet.parser.ssrc()) {
                    RefCell::get_mut(&mut receiver.resend_requester).handle_packet_received(SequenceNumber::new(u16::from(packet.parser.sequence_number())));
                    if let MediaChannelIncomingEvent::RtpPacketReceived(packet) = event.take().unwrap() {
                        let _ = self.event_sender.send(MediaChannelRtpBasedEvents::DataReceived(packet));
                    }
                } else {
                    /* the packet is not from interest for use ;) */
                }
            },
            MediaChannelIncomingEvent::RtcpPacketReceived(packet) => {
                let _ = self.event_sender.send(MediaChannelRtpBasedEvents::RtcpPacketReceived(packet.clone()));
                //println!("RTCP Packet: {:?}", packet);
                /*
                match packet.clone() {
                    RtcpPacket::SenderReport(mut sr) => {
                        if let Some(channel) = self.local_sources.first() {
                            let mut buffer = [0u8; 2048];
                            sr.ssrc = channel.id;

                            let size = {
                                let mut writer = Cursor::new(&mut buffer[..]);
                                sr.write(&mut writer).unwrap();
                                writer.position() as usize
                            };
                            self.ice_control.send(PeerICEConnectionControl::SendRtcpMessage(Vec::from(&buffer[0..size])));
                        }
                    }
                    _ => {}
                }
                */
            },
            _ => {}
        }
    }
}

impl Stream for MediaChannelRtpBased {
    type Item = MediaChannelRtpBasedEvents;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        for (_, receiver) in self.receiving_sources.iter() {
            while let Poll::Ready(Some(event)) = RefCell::borrow_mut(&receiver.resend_requester).poll_next_unpin(cx) {
                match event {
                    RtpPacketResendRequesterEvent::ResendPackets(packets) => {
                        let feedback = RtcpPacketTransportFeedback {
                            ssrc: 1,
                            media_ssrc: receiver.id,
                            feedback: RtcpTransportFeedback::create_generic_nack(packets.as_slice())
                        };

                        println!("Resending packets on {} {:?} -> {:?}", receiver.id, packets, &feedback);
                        let _ = self.send_control_data_internal(&RtcpPacket::TransportFeedback(feedback));
                    },
                    RtpPacketResendRequesterEvent::PacketTimedOut(packet) => {
                        return Poll::Ready(Some(MediaChannelRtpBasedEvents::DataLost(packet.iter().map(|e| e.packet_id).collect())))
                    },
                    _ => {
                        eprintln!("Resend event on {}: {:?}", receiver.id, event);
                    }
                }
            }
        }

        if let Poll::Ready(event) = self.event_receiver.poll_next_unpin(cx) {
            Poll::Ready(event)
        } else {
            Poll::Pending
        }
    }
}