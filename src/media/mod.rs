use crate::media::application::MediaChannelApplication;
use crate::rtc::MediaId;
use webrtc_sdp::media_type::SdpMedia;
use crate::ice::PeerICEConnectionEvent;
use crate::media::audio::{MediaChannelAudio};
use crate::media::video::MediaChannelVideo;
use std::collections::HashMap;
use webrtc_sdp::attribute_type::{SdpAttributeType, SdpAttribute, SdpAttributeSsrc, SdpAttributeRtcpFbType, SdpAttributeFmtpParameters, SdpAttributeRtpmap, SdpAttributePayloadType, SdpAttributeRtcpFb, SdpAttributeFmtp};
use std::fmt::{Formatter, Debug};
use webrtc_sdp::error::SdpParserInternalError;
use crate::utils::rtp::ParsedRtpPacket;
use std::cell::{Cell, RefCell};
use crate::utils::rtcp::RtcpPacket;

pub mod application;
pub mod rtp_based;
pub mod audio;
pub mod video;

pub enum TypedMediaChannel<'a> {
    Application(&'a mut MediaChannelApplication),
    Audio(&'a mut MediaChannelAudio),
    Video(&'a mut MediaChannelVideo),
}

/* TODO: Send this in a ref cell and allow the handler to move the value.
 *       A move would indicate that the event has been consumed and we've no longer to dispatch it.
 */
pub enum MediaChannelIncomingEvent {
    TransportInitialized,
    DtlsDataReceived(Vec<u8>),
    RtpPacketReceived(ParsedRtpPacket),
    RtcpPacketReceived(RtcpPacket)
}

pub trait MediaChannel {
    fn as_typed(&mut self) -> TypedMediaChannel;

    fn media_id(&self) -> &MediaId;

    fn configure(&mut self, media: &SdpMedia) -> Result<(), String>;
    fn generate_sdp(&self) -> Result<SdpMedia, String>;

    fn process_peer_event(&mut self, event: &mut Option<MediaChannelIncomingEvent>);
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