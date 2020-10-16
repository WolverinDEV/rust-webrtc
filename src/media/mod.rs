use crate::application::ChannelApplication;
use std::collections::HashMap;
use webrtc_sdp::attribute_type::{SdpAttributeType, SdpAttribute, SdpAttributeSsrc, SdpAttributeRtcpFbType, SdpAttributeFmtpParameters, SdpAttributeRtpmap, SdpAttributePayloadType, SdpAttributeRtcpFb, SdpAttributeFmtp};
use std::fmt::{Formatter, Debug};
use webrtc_sdp::error::SdpParserInternalError;
use crate::utils::rtp::ParsedRtpPacket;
use crate::utils::rtcp::RtcpPacket;
use webrtc_sdp::media_type::SdpMedia;

mod media_line;
pub use media_line::*;

mod receiver;
pub use receiver::*;

mod sender;
pub use sender::*;
use tokio::sync::mpsc;
use crate::transport::RTCTransportControl;
use std::io::Error;

pub enum ControlDataSendError {
    BuildFailed(Error),
    SendFailed
}

#[derive(Debug)]
pub(crate) struct InternalMediaTrack {
    pub id: u32,
    pub media_line: u32,
    pub properties: HashMap<String, Option<String>>,

    pub transport_id: u32,
    pub transport: mpsc::UnboundedSender<RTCTransportControl>,
}

impl InternalMediaTrack {
    pub fn parse_properties_from_sdp(&mut self, media: &SdpMedia) {
        /* TODO: Test if the properties have changed */
        let id = self.id;
        self.properties.clear();
        for attribute in media.get_attributes_of_type(SdpAttributeType::Ssrc)
            .iter()
            .map(|e| if let SdpAttribute::Ssrc(data) = e { data } else { panic!() })
            .filter(|e| e.id == id && e.attribute.is_some()) {
            self.properties.insert(attribute.attribute.as_ref().unwrap().clone(), attribute.value.clone());
        }
    }

    pub fn write_sdp(&self, media: &mut SdpMedia) {
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
    pub feedback: Vec<CodecFeedback>
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
    pub(crate) fn append_to_sdp(&self, target: &mut SdpMedia) -> Result<(), SdpParserInternalError> {
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

        for feedback in self.feedback.iter() {
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
            feedback: Vec::new(),
            parameters: None
        };

        for feedback in sdp.get_attributes_of_type(SdpAttributeType::Rtcpfb)
            .iter()
            .map(|e| if let SdpAttribute::Rtcpfb(map) = e { map } else { panic!() })
            .filter(|e| if let SdpAttributePayloadType::PayloadType(payload) = e.payload_type { payload == payload_type } else { true }) {
            codec.feedback.push(CodecFeedback{
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