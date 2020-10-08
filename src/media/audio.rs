#![allow(dead_code)]

use crate::media::{MediaChannel, TypedMediaChannel};
use crate::ice::{PeerICEConnectionEvent, PeerICEConnectionControl};
use webrtc_sdp::media_type::{SdpMedia, SdpMediaLine, SdpMediaValue, SdpFormatList, SdpProtocolValue};
use crate::rtc::MediaId;
use webrtc_sdp::SdpConnection;
use webrtc_sdp::address::ExplicitlyTypedAddress;
use std::net::{IpAddr, Ipv4Addr};
use tokio::sync::mpsc;
use futures::Stream;
use futures::task::{Context, Poll};
use tokio::macros::support::Pin;
use webrtc_sdp::attribute_type::{SdpAttribute, SdpAttributeFmtpParameters, SdpAttributeRtcpFbType, SdpAttributeRtpmap, SdpAttributeFmtp, SdpAttributeDirection, SdpAttributeRtcpFb, SdpAttributePayloadType, SdpAttributeType, SdpAttributeSsrc, SdpAttributeExtmap, SdpAttributeMsid};
use webrtc_sdp::error::SdpParserInternalError;
use std::fmt::Debug;
use serde::export::Formatter;
use crate::utils::rtcp::RtcpPacket;
use std::collections::HashMap;
use std::io::Cursor;

/* TODO: When looking at extensions https://github.com/zxcpoiu/webrtc/blob/ea3dddf1d0880e89d84a7e502f65c65993d4169d/modules/rtp_rtcp/source/rtp_packet_received.cc#L50 */

#[derive(Debug, PartialEq)]
pub enum MediaChannelAudioEvents {
    DataReceived()
}

#[derive(Debug)]
pub struct MediaSource {
    id: u32,
    properties: HashMap<String, Option<String>>
}

impl MediaSource {
    fn parse_sdp(id: u32, media: &SdpMedia) -> Self {
        let mut result = MediaSource{
            id,
            properties: HashMap::new()
        };

        media.get_attributes_of_type(SdpAttributeType::Ssrc)
            .iter()
            .map(|e| if let SdpAttribute::Ssrc(data) = e { data } else { panic!() })
            .filter(|e| e.id == id && e.attribute.is_some())
            .for_each(|attr| {
                result.properties.insert(attr.attribute.as_ref().unwrap().clone(), attr.value.clone());
            });

        result
    }

    fn write_sdp(&self, media: &mut SdpMedia) {
        if self.properties.is_empty() {
            media.add_attribute(SdpAttribute::Ssrc(SdpAttributeSsrc {
                id: self.id,
                value: None,
                attribute: None
            })).expect("failed to add ssrc");
        } else {
            for (key, value) in self.properties.iter() {
                media.add_attribute(SdpAttribute::Ssrc(SdpAttributeSsrc {
                    id: self.id,
                    value: value.clone(),
                    attribute: Some(key.clone())
                })).expect("failed to add ssrc");
            }
        }
    }

    pub fn properties(&self) -> &HashMap<String, Option<String>> {
        &self.properties
    }

    pub fn properties_mut(&mut self) -> &mut HashMap<String, Option<String>> {
        &mut self.properties
    }

    pub fn id(&self) -> u32 {
        self.id
    }
}

pub struct MediaChannelAudio {
    media_id: MediaId,
    direction: SdpAttributeDirection,

    ice_control: mpsc::UnboundedSender<PeerICEConnectionControl>,

    remote_codes: Vec<Codec>,
    local_codecs: Vec<Codec>,

    remote_sources: Vec<MediaSource>,
    local_sources: Vec<MediaSource>,

    remote_extensions: Vec<SdpAttributeExtmap>,
    local_extensions: Vec<SdpAttributeExtmap>
}

impl MediaChannelAudio {
    pub fn new(media_id: MediaId, ice_control: mpsc::UnboundedSender<PeerICEConnectionControl>) -> Self {
        MediaChannelAudio {
            media_id,
            direction: SdpAttributeDirection::Sendrecv,

            ice_control,

            remote_codes: Vec::new(),
            local_codecs: Vec::new(),

            remote_sources: Vec::new(),
            local_sources: Vec::new(),

            remote_extensions: Vec::new(),
            local_extensions: Vec::new()
        }
    }

    pub fn remote_codecs(&self) -> &Vec<Codec> {
        &self.remote_codes
    }

    pub fn local_codecs(&self) -> &Vec<Codec> {
        &self.local_codecs
    }

    pub fn local_codecs_mut(&mut self) -> &mut Vec<Codec> {
        &mut self.local_codecs
    }

    pub fn remote_sources(&self) -> &Vec<MediaSource> {
        &self.remote_sources
    }

    pub fn local_sources(&self) -> &Vec<MediaSource> {
        &self.local_sources
    }

    pub fn local_sources_mut(&mut self) -> Vec<&mut MediaSource> {
        self.local_sources.iter_mut().collect()
    }

    pub fn register_local_source(&mut self) -> &mut MediaSource {
        let source = MediaSource {
            id: rand::random::<u32>(),
            properties: HashMap::new()
        };
        self.local_sources.push(source);
        self.local_sources.last_mut().unwrap()
    }

    pub fn remote_extensions(&self) -> &Vec<SdpAttributeExtmap> {
        &self.remote_extensions
    }

    pub fn local_extensions(&self) -> &Vec<SdpAttributeExtmap> {
        &self.local_extensions
    }

    pub fn local_extensions_mut(&mut self) -> &mut Vec<SdpAttributeExtmap> {
        &mut self.local_extensions
    }
}

impl MediaChannel for MediaChannelAudio {
    fn as_typed(&mut self) -> TypedMediaChannel {
        TypedMediaChannel::Audio(self)
    }

    fn media_id(&self) -> &MediaId {
        &self.media_id
    }

    fn configure(&mut self, media: &SdpMedia) -> Result<(), String> {
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

        for source in media.get_attributes_of_type(SdpAttributeType::Ssrc)
            .iter()
            .map(|e| if let SdpAttribute::Ssrc(data) = e { data } else { panic!() })
        {
            if self.remote_sources.iter().find(|e| e.id == source.id).is_some() {
                continue;
            }

            self.remote_sources.push(MediaSource::parse_sdp(source.id, media));
        }

        self.remote_extensions = media.get_attributes_of_type(SdpAttributeType::Extmap)
            .iter()
            .map(|e| if let SdpAttribute::Extmap(data) = e { data.clone() } else { panic!() })
            .collect();

        println!("Remote codecs: {:?}", self.remote_codes);
        println!("Remote streams: {:?}", self.remote_sources);
        Ok(())
    }

    fn generate_sdp(&self) -> Result<SdpMedia, String> {
        if self.local_codecs.is_empty() {
            return Err(String::from("No local codes specified."));
        }

        let mut media = SdpMedia::new(SdpMediaLine {
            media: SdpMediaValue::Audio,
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

    fn process_peer_event(&mut self, event: &PeerICEConnectionEvent) {
        match event {
            PeerICEConnectionEvent::MessageReceivedRtp(buffer) => {
                let packet = rtp_rs::RtpReader::new(&buffer);

                if let Ok(packet) = packet {
                    if let Some(channel) = self.local_sources.first() {
                        let mut payload = packet.payload().to_vec();

                        let mut buffer = packet.create_builder()
                            .ssrc(channel.id)
                            .add_csrc(packet.ssrc())
                            .sequence(packet.sequence_number() + 100)
                            .timestamp(packet.timestamp().wrapping_add(100000))
                            .payload(&payload);

                        let buffer = buffer.build().unwrap();
                        self.ice_control.send(PeerICEConnectionControl::SendRtpMessage(buffer));
                    }
                } else if let Err(err) = packet {
                    println!("Failed to parse RTP packet: {:?}", err);
                } else {
                    panic!();
                }
            },
            PeerICEConnectionEvent::MessageReceivedRtcp(buffer) => {
                match RtcpPacket::parse(buffer) {
                    Ok(packet) => {
                        println!("RTCP Packet: {:?}", packet);
                        match packet {
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
                    },
                    Err(err) => {
                        println!("Failed to parse RTCP packet: {:?} Data: {:?}", err, buffer);
                        //self.ice_control.send(PeerICEConnectionControl::SendRtcpMessage(buffer.clone()));
                    }
                }
            },
            _ => {}
        }
    }
}

impl Stream for MediaChannelAudio {
    type Item = MediaChannelAudioEvents;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Pending
    }
}

#[derive(Clone)]
pub struct CodecFeedback {
    pub feedback_type: SdpAttributeRtcpFbType,
    pub parameter: String,
    pub extra: String,
}

impl Debug for CodecFeedback {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodecFeedback")
            .field("feedback_type", &self.feedback_type.to_string())
            .field("parameter", &self.parameter)
            .field("extra", &self.extra)
            .finish()
    }
}

#[derive(Clone)]
pub struct Codec {
    pub payload_type: u8,
    pub codec_name: String,
    pub frequency: u32,
    pub channels: Option<u32>,

    pub parameters: Option<SdpAttributeFmtpParameters>,
    pub feedback: Option<CodecFeedback>
}

impl Debug for Codec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Codec")
            .field("payload_type", &self.payload_type)
            .field("codec_name", &self.codec_name)
            .field("frequency", &self.frequency)
            .field("channels", &self.channels)
            .field("parameters", &if let Some(params) = &self.parameters { params.to_string() } else { String::from("None") })
            .field("feedback", &self.feedback)
            .finish()
    }
}

impl Codec {
    fn append_to_sdp(&self, target: &mut SdpMedia) -> Result<(), SdpParserInternalError> {
        target.add_attribute(SdpAttribute::Rtpmap(SdpAttributeRtpmap{
            channels: self.channels.clone(),
            codec_name: self.codec_name.clone(),
            frequency: self.frequency,
            payload_type: self.payload_type
        }))?;

        if let Some(parameters) = &self.parameters {
            target.add_attribute(SdpAttribute::Fmtp(SdpAttributeFmtp{
                parameters: parameters.clone(),
                payload_type: self.payload_type
            }))?;
        }

        if let Some(feedback) = &self.feedback {
            target.add_attribute(SdpAttribute::Rtcpfb(SdpAttributeRtcpFb{
                payload_type: SdpAttributePayloadType::PayloadType(self.payload_type),
                extra: feedback.extra.clone(),
                parameter: feedback.parameter.clone(),
                feedback_type: feedback.feedback_type.clone()
            }))?;
        }

        Ok(())
    }

    fn from_sdp(payload_type: u8, sdp: &SdpMedia) -> Option<Codec> {
        let map = sdp.get_attributes_of_type(SdpAttributeType::Rtpmap)
            .iter()
            .map(|e| if let SdpAttribute::Rtpmap(map) = e { map } else { panic!() })
            .find(|e| e.payload_type == payload_type);

        if map.is_none() {
            return None
        }

        let map = map.unwrap();
        let mut codec = Codec {
            payload_type,
            frequency: map.frequency,
            codec_name: map.codec_name.clone(),
            channels: map.channels.clone(),
            feedback: None,
            parameters: None
        };

        let feedback = sdp.get_attributes_of_type(SdpAttributeType::Rtcpfb)
            .iter()
            .map(|e| if let SdpAttribute::Rtcpfb(map) = e { map } else { panic!() })
            .find(|e| if let SdpAttributePayloadType::PayloadType(payload) = e.payload_type { payload == payload_type } else { true });
        if let Some(feedback) = feedback {
            codec.feedback = Some(CodecFeedback{
                feedback_type: feedback.feedback_type.clone(),
                parameter: feedback.parameter.clone(),
                extra: feedback.extra.clone()
            });
        }

        let parameters = sdp.get_attributes_of_type(SdpAttributeType::Fmtp)
            .iter()
            .map(|e| if let SdpAttribute::Fmtp(map) = e { map } else { panic!() })
            .find(|e| e.payload_type == payload_type);
        if let Some(parameters) = parameters {
            codec.parameters = Some(parameters.parameters.clone());
        }

        Some(codec)
    }
}