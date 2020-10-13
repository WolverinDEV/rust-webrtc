#![allow(dead_code)]

use glib::{MainContext, BoolError};
use webrtc_sdp::media_type::{SdpMediaValue};
use std::sync::{Arc, Mutex};
use webrtc_sdp::attribute_type::{SdpAttributeType, SdpAttribute, SdpAttributeSetup, SdpAttributeMsidSemantic, SdpAttributeGroup, SdpAttributeGroupSemantic};
use std::fmt::Debug;
use futures::{FutureExt, StreamExt};
use libnice::ice::{Candidate};
use futures::task::{Context, Poll};
use tokio::macros::support::Pin;
use std::rc::Rc;
use std::cell::{RefCell, RefMut};
use crate::media;
use std::collections::HashMap;
use crate::ice::{PeerICEConnection, ICEConnectionInitializeError, ICECredentials, PeerICEConnectionEvent, ICECandidateAddError};
use webrtc_sdp::{SdpSession, SdpOrigin, SdpTiming};
use webrtc_sdp::address::ExplicitlyTypedAddress;
use std::net::{IpAddr, Ipv4Addr};
use libnice::ffi::{NiceCompatibility, NiceAgentProperty};
use libnice::sys::NiceAgentOption_NICE_AGENT_OPTION_ICE_TRICKLE;
use crate::media::application::MediaChannelApplication;
use crate::media::audio::MediaChannelAudio;
use crate::media::video::MediaChannelVideo;
use crate::media::MediaChannelIncomingEvent;
use crate::utils::rtp::ParsedRtpPacket;
use crate::utils::rtcp::RtcpPacket;

/// The default setup type if the remote peer offers active and passive setup
/// Allowed values are only `SdpAttributeSetup::Passive` and `SdpAttributeSetup::Active`
pub const ACT_PASS_DEFAULT_SETUP_TYPE: SdpAttributeSetup = SdpAttributeSetup::Passive;

/// The first element contains the media line index.
/// The second element contains the media identifier
pub type MediaId = (usize, String);

pub enum MediaIdFragment {
    /// Only the media line index is known
    LineIndex(usize),

    /// Only the media identifier is known
    Identifier(String),

    /// The full media id is known
    Full(MediaId)
}

impl MediaIdFragment {
    fn matches(&self, target: &MediaId) -> bool {
        match self {
            MediaIdFragment::Full(media) => {
                target == media
            },
            MediaIdFragment::Identifier(identifier) => {
                &target.1 == identifier
            },
            MediaIdFragment::LineIndex(index) => {
                &target.0 == index
            }
        }
    }
}

pub enum PeerConnectionEvent {
    LocalIceCandidate(Option<Candidate>, MediaId)
}

#[derive(Debug, PartialOrd, PartialEq, Clone)]
pub enum RtcDescriptionType {
    Offer,
    Answer
}

#[derive(Debug, PartialOrd, PartialEq, Clone)]
pub enum PeerConnectionState {
    New,
    Connecting,
    Connected,
    Disconnecting,
    Closed,
    Failed
}

pub struct PeerConnection {
    state: PeerConnectionState,

    ice_agent: libnice::ice::Agent,
    ice_channels: Vec<Rc<RefCell<PeerICEConnection>>>,

    origin_username: String,
    media_channel: HashMap<MediaId, Arc<Mutex<dyn media::MediaChannel>>>
}

#[derive(Debug)]
pub enum RemoteDescriptionApplyError {
    UnsupportedMode,
    UnsupportedMediaType{ media_index: usize },
    InvalidSdp{ reason: String },
    InternalError{ detail: String },

    DuplicatedApplicationChannel,
    MissingAttribute{ media_index: usize, attribute: String },
    FailedToAddIceStream{ error: BoolError },

    MediaChannelConfigure{ error: String },
    IceInitializeError{ result: ICEConnectionInitializeError, media_id: MediaId },
    MixedIceSetupStates{ },
    IceSetupUnsupported{ media_index: usize }
}

#[derive(Debug)]
pub enum CreateAnswerError {
    MediaLineError{ error: String, media_id: MediaId }
}

macro_rules! get_attribute_value {
    ($media:expr, $index:ident, $type:ident) => {
        $media.get_attribute(SdpAttributeType::$type)
            .and_then(|attr| if let SdpAttribute::$type(value) = attr { Some(value) } else { None })
            .ok_or(RemoteDescriptionApplyError::MissingAttribute { media_index: $index, attribute: SdpAttributeType::$type.to_string() })
    }
}

/*
v=0
o=- 5007946851132490 2 IN IP4 0.0.0.0
s=-
t=0 0
a=group:BUNDLE 0 1 2
a=msid-semantic: WMS DataPipes

m=audio 9 UDP/TLS/RTP/SAVPF 111
c=IN IP4 0.0.0.0
a=recvonly
a=mid:0
a=rtcp-mux
a=extmap:1 urn:ietf:params:rtp-hdrext:ssrc-audio-level
a=rtpmap:111 opus/48000/2
a=fmtp:111 minptime=10;useinbandfec=1
a=fingerprint:sha-256 F4:00:69:82:EB:B7:E2:08:F7:82:99:9D:AA:C4:95:E8:F9:9D:25:F9:05:84:2C:EA:2D:58:11:1D:22:28:7B:E8
a=setup:active
a=ice-ufrag:jP6v
a=ice-pwd:bVKJaGwHIpBfRVzG/XqBCH
a=ice-options:trickle
m=audio 9 UDP/TLS/RTP/SAVPF 111

c=IN IP4 0.0.0.0
a=recvonly
a=mid:1
a=rtcp-mux
a=extmap:1 urn:ietf:params:rtp-hdrext:ssrc-audio-level
a=rtpmap:111 opus/48000/2
a=fmtp:111 minptime=10;useinbandfec=1
a=fingerprint:sha-256 F4:00:69:82:EB:B7:E2:08:F7:82:99:9D:AA:C4:95:E8:F9:9D:25:F9:05:84:2C:EA:2D:58:11:1D:22:28:7B:E8
a=setup:active
a=ice-ufrag:jP6v
a=ice-pwd:bVKJaGwHIpBfRVzG/XqBCH
a=ice-options:trickle
m=application 9 DTLS/SCTP 5000

c=IN IP4 0.0.0.0
a=mid:2
a=sctpmap:5000 webrtc-datachannel 1024
a=fingerprint:sha-256 F4:00:69:82:EB:B7:E2:08:F7:82:99:9D:AA:C4:95:E8:F9:9D:25:F9:05:84:2C:EA:2D:58:11:1D:22:28:7B:E8
a=setup:active
a=ice-ufrag:jP6v
a=ice-pwd:bVKJaGwHIpBfRVzG/XqBCH
a=ice-options:trickle
 */

impl PeerConnection {
    pub fn new(event_loop: MainContext) -> Self {
        let mut connection = PeerConnection{
            state: PeerConnectionState::New,

            /* "-" indicates no username */
            origin_username: String::from("-"),

            ice_agent: libnice::ice::Agent::new_full(event_loop, NiceCompatibility::RFC5245, NiceAgentOption_NICE_AGENT_OPTION_ICE_TRICKLE),
            ice_channels: Vec::with_capacity(8),

            media_channel: HashMap::default()
        };

        connection.ice_agent.get_ffi_agent().on_selected_pair(|_, _, _, _| println!("Candidate pair has been found")).unwrap();
        //connection.ice_agent.get_ffi_agent().set_nice_property(NiceAgentProperty::StunServer(Some(String::from("74.125.143.127"))));//stun.l.google.com
        //connection.ice_agent.get_ffi_agent().set_nice_property(NiceAgentProperty::StunPort(19302));

        connection.ice_agent.get_ffi_agent().set_nice_property(NiceAgentProperty::IceTcp(false)).unwrap();
        connection.ice_agent.get_ffi_agent().set_nice_property(NiceAgentProperty::IceUdp(true)).unwrap();

        connection.ice_agent.get_ffi_agent().set_nice_property(NiceAgentProperty::Upnp(false)).unwrap();
        connection.ice_agent.get_ffi_agent().set_nice_property(NiceAgentProperty::ControllingMode(false)).unwrap();
        connection.ice_agent.get_ffi_agent().set_nice_property(NiceAgentProperty::IceTrickle(true)).unwrap();

        connection
    }

    pub fn set_remote_description(&mut self, session: &webrtc_sdp::SdpSession, mode: RtcDescriptionType) -> Result<(), RemoteDescriptionApplyError> {
        if mode != RtcDescriptionType::Offer {
            return Err(RemoteDescriptionApplyError::UnsupportedMode);
        }

        /* copy the origin username and send it back, required for mozilla for example */
        self.origin_username = session.origin.username.clone();

        /* TODO: Proper reset everything? */
        self.media_channel.clear();
        self.ice_channels.clear();

        println!("Desk: {:}", session);
        for media_index in 0..session.media.len() {
            let media = &session.media[media_index as usize];
            let media_id: MediaId = (media_index, get_attribute_value!(media, media_index, Mid)?.clone());

            let credentials = ICECredentials {
                username: get_attribute_value!(media, media_index, IceUfrag)?.clone(),
                password: get_attribute_value!(media, media_index, IcePwd)?.clone()
            };

            println!("Remote credentials: {:?}", &credentials);
            let setup = get_attribute_value!(media, media_index, Setup)?;
            let local_setup = {
                match setup {
                    SdpAttributeSetup::Passive => Ok(SdpAttributeSetup::Active),
                    SdpAttributeSetup::Active => Ok(SdpAttributeSetup::Passive),
                    SdpAttributeSetup::Actpass => Ok(ACT_PASS_DEFAULT_SETUP_TYPE),
                    _ => Err(RemoteDescriptionApplyError::IceSetupUnsupported { media_index })
                }
            }?;

            let ice_channel = self.create_ice_channel(&credentials, media_id.clone(), local_setup)?;
            let mut ice_channel_mut = RefCell::borrow_mut(&ice_channel);

            if ice_channel_mut.owning_media_id.0 == media_index {
                /* yeahr it's our channel */
                for candidate in media.get_attributes_of_type(SdpAttributeType::Candidate)
                    .iter()
                    .map(|attribute| if let SdpAttribute::Candidate(c) = attribute { c } else { panic!("expected a candidate") }) {

                    ice_channel_mut.add_remote_candidate(Some(candidate))
                        .map_err(|err| RemoteDescriptionApplyError::InternalError { detail: String::from(format!("failed to add candidate: {:?}", err)) })?;
                }

                /* /* Firefox does not sends this stuff */
                let is_trickle = media.get_attribute(SdpAttributeType::IceOptions)
                    .map(|attribute| if let SdpAttribute::IceOptions(opts) = attribute { opts } else { panic!("expected a ice options") })
                    .map_or(false, |attributes| attributes.iter().find(|attribute| attribute.as_str() == "trickle").is_some());
                if media.get_attribute(SdpAttributeType::EndOfCandidates).is_some() {
                    if is_trickle {
                        return Err(RemoteDescriptionApplyError::InvalidSdp { reason: String::from("found end-of-candidates but the ice mode is expected to be trickle") });
                    }
                } else {
                    if !is_trickle {
                        return Err(RemoteDescriptionApplyError::InvalidSdp { reason: String::from("missing end-of-candidates but the ice mode is trickle") });
                    }
                }
                */
            }
            ice_channel_mut.register_media_channel(&media_id);

            let channel = match *media.get_type() {
                SdpMediaValue::Application => {
                    Ok(Arc::new(Mutex::new(MediaChannelApplication::new(media_id.clone(), ice_channel_mut.control_sender.clone()))) as Arc<Mutex<dyn media::MediaChannel>>)
                },
                SdpMediaValue::Audio => {
                    Ok(Arc::new(Mutex::new(MediaChannelAudio::new(media_id.clone(), ice_channel_mut.control_sender.clone()))) as Arc<Mutex<dyn media::MediaChannel>>)
                },
                SdpMediaValue::Video => {
                    Ok(Arc::new(Mutex::new(MediaChannelVideo::new(media_id.clone(), ice_channel_mut.control_sender.clone()))) as Arc<Mutex<dyn media::MediaChannel>>)
                },
            }?;

            channel.lock().unwrap().configure(media)
                .map_err(|err| RemoteDescriptionApplyError::MediaChannelConfigure{ error: err })?;

            self.media_channel.insert(media_id, channel);
        }

        Ok(())
    }

    pub fn create_answer(&mut self) -> Result<SdpSession, CreateAnswerError> {
        let mut answer = SdpSession::new(0, SdpOrigin {
            session_id: rand::random(),
            session_version: 2,
            unicast_addr: ExplicitlyTypedAddress::Ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            username: self.origin_username.clone()
        }, String::from("-")); /* "-" indicates no session id */
        answer.timing = Some(SdpTiming{ start: 0, stop: 0 }); /* required by WebRTC */

        /* Bundle out media streams */
        answer.add_attribute(SdpAttribute::Group(SdpAttributeGroup{
            semantics: SdpAttributeGroupSemantic::Bundle,
            tags: self.media_channel.keys().into_iter().map(|e| e.1.clone()).collect()
        }));

        let mut keys = self.media_channel.keys().map(|e| e.clone()).collect::<Vec<MediaId>>();
        keys.sort_by_key(|e| e.0);

        for media_id in keys.iter() {
            let channel = self.media_channel.get(media_id).expect("missing expected key").clone();
            let channel = channel.lock().unwrap();

            let mut media = channel.generate_sdp()
                .map_err(|err| CreateAnswerError::MediaLineError{ media_id: (*media_id).clone(), error: err })?;

            let ice_channel = self.find_ice_channel_by_media_fragment(&MediaIdFragment::Full((*media_id).clone()))
                .expect("missing ice channel for media channel");
            let ice_channel = RefCell::borrow(ice_channel);

            media.add_attribute(SdpAttribute::Mid(media_id.1.clone())).unwrap();
            media.add_attribute(SdpAttribute::IceUfrag(ice_channel.local_credentials.username.clone())).unwrap();
            media.add_attribute(SdpAttribute::IcePwd(ice_channel.local_credentials.password.clone())).unwrap();
            media.add_attribute(SdpAttribute::IceOptions(vec![String::from("trickle")])).unwrap();
            media.add_attribute(SdpAttribute::Fingerprint(ice_channel.fingerprint.clone())).unwrap();
            media.add_attribute(SdpAttribute::Setup(ice_channel.setup.clone())).unwrap();

            answer.media.push(media);
        }

        Ok(answer)
    }

    /// Adding a remote ice candidate.
    /// To signal a no more candidates event just add `None`
    pub fn add_remove_ice_candidate(&mut self, media_fragment: &MediaIdFragment, candidate: Option<&Candidate>) -> Result<(), ICECandidateAddError> {
        if let Some(channel) = self.find_ice_channel_by_media_fragment(media_fragment) {
            /* in theory the channel should not be borrowed elsewhere */
            let mut channel = RefCell::borrow_mut(channel);

            if !media_fragment.matches(&channel.owning_media_id) {
                Ok(())
            } else {
                channel.add_remote_candidate(candidate)
            }
        } else {
            Err(ICECandidateAddError::UnknownMediaChannel)
        }
    }

    pub fn finish_remote_ice_candidates(&mut self) {
        /* FIXME: Expose the media streams and let the client call add_remove_ice_candidate */
        self.ice_channels.iter_mut().for_each(|channel| {
            let _ = RefCell::borrow_mut(channel).add_remote_candidate(None).map_err(|error| {
                eprintln!("Failed to add ice candidate finished: {:?}", error);
                ()
            });
        });
    }

    pub fn media_channels_mut(&mut self) -> Vec<Arc<Mutex<dyn media::MediaChannel>>> {
        self.media_channel.iter_mut().map(|e| e.1.clone()).collect()
    }

    fn create_ice_channel(&mut self, credentials: &ICECredentials, media_id: MediaId, setup: SdpAttributeSetup) -> Result<Rc<RefCell<PeerICEConnection>>, RemoteDescriptionApplyError> {
        if let Some(channel) = self.ice_channels.iter_mut().find(|entry| {
            let entry = RefCell::borrow(entry);
            &entry.remote_credentials == credentials
        }) {
            if RefCell::borrow(channel).setup.to_string() != setup.to_string() {
                Err(RemoteDescriptionApplyError::MixedIceSetupStates {})
            } else {
                Ok(channel.clone())
            }
        } else {
            println!("Creating a new channel");
            /* register a new channel */
            let stream = libnice::ice::Agent::stream_builder(&mut self.ice_agent, 1).build().map_err(|error| RemoteDescriptionApplyError::FailedToAddIceStream { error })?;

            let connection = PeerICEConnection::new(stream, credentials.clone(), media_id.clone(), setup)
                .map_err(|err| RemoteDescriptionApplyError::IceInitializeError { result: err, media_id: media_id.clone() })?;

            let connection = Rc::new(RefCell::new(connection));
            self.ice_channels.push(connection);

            Ok(self.ice_channels.last_mut().unwrap().clone())
        }
    }

    fn find_ice_channel_by_media_fragment(&mut self, media_fragment: &MediaIdFragment) -> Option<&Rc<RefCell<PeerICEConnection>>> {
        self.ice_channels.iter().find(|channel| RefCell::borrow(channel).media_ids.iter().find(|media| media_fragment.matches(media)).is_some())
    }

    fn dispatch_media_event(&self, ice: &mut RefMut<PeerICEConnection>, event: MediaChannelIncomingEvent) {
        let mut event = Some(event);
        for stream_id in ice.media_ids.iter() {
            let channel = self.media_channel.get(stream_id)
                .expect("missing media channel, but it's registered on an ice connection");

            let mut channel = channel.lock().unwrap();
            channel.process_peer_event(&mut event);
            if event.is_none() {
                /* event has been consumed */
                break;
            }
        }
    }

    fn handle_ice_event(&mut self, ice: &mut RefMut<PeerICEConnection>, event: PeerICEConnectionEvent) -> Option<PeerConnectionEvent> {
        match event {
            PeerICEConnectionEvent::LocalIceCandidate(candidate) => {
                return Some(PeerConnectionEvent::LocalIceCandidate(Some(candidate.into()), ice.owning_media_id.clone()));
            },
            PeerICEConnectionEvent::LocalIceGatheringFinished() => {
                return Some(PeerConnectionEvent::LocalIceCandidate(None, ice.owning_media_id.clone()));
            },
            PeerICEConnectionEvent::DtlsInitialized() => {
                self.dispatch_media_event(ice, MediaChannelIncomingEvent::TransportInitialized);
                None
            },
            PeerICEConnectionEvent::MessageReceivedDtls(message) => {
                self.dispatch_media_event(ice, MediaChannelIncomingEvent::DtlsDataReceived(message));
                None
            },
            PeerICEConnectionEvent::MessageReceivedRtcp(message) => {
                let mut packets = [&[0u8][..]; 128];
                let packet_count = RtcpPacket::split_up_packets(message.as_slice(), &mut packets[..]);
                if let Err(error) = packet_count {
                    eprintln!("Received invalid merged packet: {:?}", error);
                    return None;
                }

                for index in 0..packet_count.unwrap() {
                    match RtcpPacket::parse(packets[index]) {
                        Ok(packet) => {
                            let mut buffer = [0u8; 2038];
                            match packet.write(&mut buffer) {
                                Err(error) => {
                                    eprintln!("Failed to write received RTCP packet: {:?}", error);
                                },
                                Ok(length) => {
                                    if packets[index] != &buffer[0..length] {
                                        eprintln!("Parsed packet: {:?}", packet);
                                        eprintln!("Created packet is different than source packet:\nSource:  {:?}\nCreated: {:?}", &packets[index], &buffer[0..length]);
                                    }
                                }
                            }
                            self.dispatch_media_event(ice, MediaChannelIncomingEvent::RtcpPacketReceived(packet));
                        },
                        Err(error) => {
                            eprintln!("Failed to decode RTCP packet: {:?}", error);
                        }
                    }
                }
                None
            },
            PeerICEConnectionEvent::MessageReceivedRtp(message) => {
                match ParsedRtpPacket::new(message) {
                    Ok(reader) => {
                        self.dispatch_media_event(ice, MediaChannelIncomingEvent::RtpPacketReceived(reader));
                    },
                    Err((error, _)) => {
                        eprintln!("Failed to decode RTP packet: {:?}", error);
                    }
                }
                None
            },
            _ => {
                None
            }
        }

    }
}

unsafe impl Send for PeerConnection {}

impl futures::stream::Stream for PeerConnection {
    type Item = PeerConnectionEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let streams = self.ice_channels.clone();
        for stream in streams.iter() {
            let mut stream = RefCell::borrow_mut(stream);
            while let Poll::Ready(event) = stream.poll_next_unpin(cx) {
                if let Some(event) = event {
                    if let Some(event) = self.handle_ice_event(&mut stream, event) {
                        return Poll::Ready(Some(event));
                    }
                } else {
                    /* TODO: It's not unexpected if receive some kind of error previously. We need some error handing breforhand */
                    panic!("Unexpected ICE exit");
                }
            }
        }

        let _ = self.ice_agent.poll_unpin(cx);
        Poll::Pending
    }
}