use webrtc_sdp::media_type::{SdpMediaValue, SdpMedia, SdpFormatList, SdpMediaLine, SdpProtocolValue};
use crate::media::{InternalMediaTrack, Codec, CodecFeedback};
use webrtc_sdp::attribute_type::{SdpAttributeExtmap, SdpAttribute, SdpAttributeType, SdpAttributeRtcpFb, SdpAttributeRtcpFbType};
use webrtc_sdp::SdpConnection;
use webrtc_sdp::address::ExplicitlyTypedAddress;
use std::net::{IpAddr, Ipv4Addr};

pub struct MediaLine {
    pub id: String,
    pub index: u32,

    pub media_type: SdpMediaValue,

    /* if zero than currently no transport */
    pub(crate) transport_id: u32,
    pub(crate) requires_negotiation: bool,

    pub(crate) remote_streams: Vec<u32>,
    pub(crate) local_streams: Vec<u32>,

    pub(crate) remote_codecs: Vec<Codec>,
    pub(crate) local_codecs: Vec<Codec>,

    pub(crate) remote_extensions: Vec<SdpAttributeExtmap>,
    pub(crate) local_extensions: Vec<SdpAttributeExtmap>,
}

#[derive(Debug)]
pub enum MediaLineParseError {
    MissingMediaId,
    InvalidFormatList,
    MissingCodecDescription(u32),
    InvalidSendRecvStatus
}

impl MediaLine {
    pub(crate) fn new(index: u32, id: String, media_type: SdpMediaValue) -> Self {
        MediaLine{
            id,
            index,

            media_type,

            transport_id: 0,
            requires_negotiation: false,

            remote_streams: Vec::new(),
            local_streams: Vec::new(),

            remote_codecs: Vec::new(),
            local_codecs: Vec::new(),

            remote_extensions: Vec::new(),
            local_extensions: Vec::new(),
        }
    }

    pub fn remote_codecs(&self) -> &Vec<Codec> {
        &self.remote_codecs
    }

    pub fn local_codecs(&self) -> &Vec<Codec> {
        &self.local_codecs
    }

    pub fn register_local_codec(&mut self, codec: Codec) {
        self.unregister_local_codec(codec.payload_type);
        self.local_codecs.push(codec);
        self.requires_negotiation = true;
    }

    pub fn unregister_local_codec(&mut self, payload_type: u8) {
        self.local_codecs.retain(|e| e.payload_type != payload_type);
        self.requires_negotiation = true;
    }

    pub fn remote_extensions(&self) -> &Vec<SdpAttributeExtmap> {
        &self.remote_extensions
    }

    pub fn local_extensions(&self) -> &Vec<SdpAttributeExtmap> {
        &self.local_extensions
    }

    pub fn register_local_extension(&mut self, extension: SdpAttributeExtmap) {
        self.unregister_local_extension(extension.id);
        self.local_extensions.push(extension);
        self.requires_negotiation = true;
    }

    pub fn unregister_local_extension(&mut self, extension_id: u16) {
        self.local_extensions.retain(|e| e.id == extension_id);
        self.requires_negotiation = true;
    }

    pub(crate) fn new_from_sdp(index: u32, media: &SdpMedia) -> Result<Self, MediaLineParseError> {
        let id = if let Some(SdpAttribute::Mid(id)) = media.get_attribute(SdpAttributeType::Mid) { Some(id.clone()) } else { None };
        if id.is_none() {
            return Err(MediaLineParseError::MissingMediaId);
        }

        let mut result = MediaLine::new(index, id.unwrap(), media.get_type().clone());
        if *media.get_type() == SdpMediaValue::Application {
            if media.get_attribute(SdpAttributeType::Recvonly).is_some() ||
                media.get_attribute(SdpAttributeType::Sendonly).is_some() {
                return Err(MediaLineParseError::InvalidSendRecvStatus);
            }
        } else {
            if let SdpFormatList::Integers(formats) = media.get_formats() {
                result.remote_codecs.reserve(formats.len());
                for format in formats.iter() {
                    let codec = Codec::from_sdp(*format as u8, media);
                    if codec.is_none() {
                        return Err(MediaLineParseError::MissingCodecDescription(*format));
                    }

                    result.remote_codecs.push(codec.unwrap());
                }
            } else {
                return Err(MediaLineParseError::InvalidFormatList);
            }
        }

        result.update_from_sdp(media)?;

        result.remote_extensions = media.get_attributes_of_type(SdpAttributeType::Extmap)
            .iter()
            .map(|e| if let SdpAttribute::Extmap(data) = e { data.clone() } else { panic!() })
            .collect();

        println!("Remote codecs: {:?}", result.remote_codecs);
        println!("Remote streams: {:?}", result.remote_streams);
        Ok(result)
    }

    pub(crate) fn update_from_sdp(&mut self, media: &SdpMedia) -> Result<(), MediaLineParseError> {
        self.remote_streams.clear();
        if media.get_attribute(SdpAttributeType::Recvonly).is_none() {
            for source in media.get_attributes_of_type(SdpAttributeType::Ssrc)
                .iter()
                .map(|e| if let SdpAttribute::Ssrc(data) = e { data } else { panic!() })
            {
                if self.remote_streams.iter().find(|e| **e == source.id).is_some() {
                    continue;
                }

                self.remote_streams.push(source.id);
            }
        }

        Ok(())
    }

    /// Attention: Only use this function if the media type is NOT application!
    pub(crate) fn generate_local_description(&self) -> Option<SdpMedia> {
        if self.media_type == SdpMediaValue::Application {
            return None;
        }

        if self.local_codecs.is_empty() {
            return None;
        }

        let mut media = SdpMedia::new(SdpMediaLine {
            media: self.media_type.clone(),
            formats: SdpFormatList::Integers(self.local_codecs.iter().map(|e| e.payload_type as u32).collect()),
            port: 9,
            port_count: 0,
            proto: SdpProtocolValue::UdpTlsRtpSavpf
        });

        media.set_connection(SdpConnection{
            address: ExplicitlyTypedAddress::Ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            ttl: None,
            amount: None
        }).unwrap();
        media.add_attribute(SdpAttribute::RtcpMux).unwrap();
        media.add_attribute(SdpAttribute::Mid(self.id.clone())).unwrap();

        let no_local_streams = self.local_streams.is_empty();
        let no_remote_streams = self.remote_streams.is_empty() /* TODO: Test if we're doing an offer or answer (If we're doing an offer, set this to false)! */;

        if no_local_streams && no_remote_streams {
            media.add_attribute(SdpAttribute::Inactive).unwrap();
        } else if no_local_streams {
            media.add_attribute(SdpAttribute::Recvonly).unwrap();
        } else if no_remote_streams {
            media.add_attribute(SdpAttribute::Sendonly).unwrap();
        } else {
            media.add_attribute(SdpAttribute::Sendrecv).unwrap();
        }

        self.local_extensions.iter().for_each(|ext|
            media.add_attribute(SdpAttribute::Extmap(ext.clone())).unwrap()
        );

        for codec in self.local_codecs.iter() {
            codec.append_to_sdp(&mut media)
                .expect("failed to write codec SDP");
        }

        Some(media)
    }
}