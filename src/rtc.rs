#![allow(dead_code)]

use glib::{MainContext, BoolError};
use webrtc_sdp::media_type::{SdpMediaValue, SdpMedia};
use webrtc_sdp::attribute_type::{SdpAttributeType, SdpAttribute, SdpAttributeSetup, SdpAttributeGroup, SdpAttributeGroupSemantic};
use std::fmt::Debug;
use futures::{FutureExt, StreamExt};
use libnice::ice::{Candidate};
use futures::task::{Context, Poll};
use tokio::macros::support::Pin;
use std::rc::Rc;
use std::cell::{RefCell, RefMut};
use std::collections::{HashMap, BTreeMap};
use crate::transport::{RTCTransport, RTCTransportInitializeError, ICECredentials, RTCTransportEvent, RTCTransportICECandidateAddError, RTCTransportState};
use webrtc_sdp::{SdpSession, SdpOrigin, SdpTiming};
use webrtc_sdp::address::ExplicitlyTypedAddress;
use std::net::{IpAddr, Ipv4Addr};
use libnice::ffi::{NiceCompatibility, NiceAgentProperty};
use libnice::sys::NiceAgentOption_NICE_AGENT_OPTION_ICE_TRICKLE;
use crate::application::{ChannelApplication, DataChannel, ApplicationChannelEvent};
use crate::media::{MediaLine, MediaLineParseError, ActiveInternalMediaReceiver, MediaReceiver, InternalMediaSender, InternalMediaTrack, MediaSender, NegotiationState, InternalMediaReceiver};
use crate::utils::rtp::ParsedRtpPacket;
use crate::utils::rtcp::RtcpPacket;
use std::ops::{DerefMut};
use tokio::sync::mpsc;
use crate::utils::RtpPacketResendRequester;
use crate::sctp::message::DataChannelType;

/// The default setup type if the remote peer offers active and passive setup
/// Allowed values are only `SdpAttributeSetup::Passive` and `SdpAttributeSetup::Active`
pub const ACT_PASS_DEFAULT_SETUP_TYPE: SdpAttributeSetup = SdpAttributeSetup::Passive;

pub enum PeerConnectionEvent {
    NegotiationNeeded,
    LocalIceCandidate(Option<Candidate>, u32),

    ReceivedRemoteStream(MediaReceiver),
    ReceivedDataChannel(DataChannel),

    UnassignableRtcpPacket(RtcpPacket),
    UnassignableRtpPacket(ParsedRtpPacket),
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
    Disconnected,
    Failed
}

#[derive(Debug, PartialEq)]
pub enum SignallingState {
    /// Nothing has been negotiated
    None,
    /// A local offer has been created, awaiting the remote answer.
    /// When being in this state, no modifications to the streams are allowed.
    HaveLocalOffer,
    /// Received a remote offer, awaiting the own answer to be generated
    HaveRemoteOffer,
    /// Everything has been negotiated
    Negotiated,
    /// Everything has been negotiated, but something changed
    NegotiationRequired
}

pub struct PeerConnection {
    state: PeerConnectionState,
    signalling_state: SignallingState,

    ice_agent: libnice::ice::Agent,
    transport: BTreeMap<u32, Rc<RefCell<RTCTransport>>>,
    media_lines: Vec<Rc<RefCell<MediaLine>>>,

    stream_receiver: BTreeMap<u32, Box<dyn InternalMediaReceiver>>,
    stream_sender: BTreeMap<u32, InternalMediaSender>,

    application_channel: Box<ChannelApplication>,

    origin_username: String,

    /*
     * TODO: Does this really needs to be a unbound server/receiver or could we better use something else?
     * DeVec for example (but what's about memory growth?)
     */
    local_events: (mpsc::UnboundedSender<PeerConnectionEvent>, mpsc::UnboundedReceiver<PeerConnectionEvent>)
}

#[derive(Debug)]
pub enum RemoteDescriptionApplyError {
    InvalidNegotiationState,

    UnsupportedMode,
    UnsupportedMediaType{ media_index: usize },
    InvalidSdp{ reason: String },
    InternalError{ detail: String },

    /// The remote description contains less media lines than
    /// we're expecting
    MissingMediaLines,

    DuplicatedApplicationChannel,
    MissingAttribute{ media_index: usize, attribute: String },
    FailedToAddIceStream{ error: BoolError },

    MediaLineParseError{ media_line: u32, error: MediaLineParseError },

    MediaChannelConfigure{ error: String },
    IceInitializeError{ result: RTCTransportInitializeError, media_id: u32 },
    MixedIceSetupStates{ },
    IceSetupUnsupported{ media_index: usize },
    MissMatchingIceSettings{ media_index: u32 },
}

#[derive(Debug)]
pub enum CreateAnswerError {
    MediaLineError{ error: String, media_id: u32 },
    DescribeError(u32),

    MissingTransportChannel(u32),
    InternalError(String),

    InvalidNegotiationState
}

macro_rules! get_attribute_value {
    ($media:expr, $index:ident, $type:ident) => {
        $media.get_attribute(SdpAttributeType::$type)
            .and_then(|attr| if let SdpAttribute::$type(value) = attr { Some(value) } else { None })
            .ok_or(RemoteDescriptionApplyError::MissingAttribute { media_index: $index, attribute: SdpAttributeType::$type.to_string() })
    }
}

impl PeerConnection {
    pub fn new(event_loop: MainContext) -> Self {
        let mut connection = PeerConnection{
            state: PeerConnectionState::New,
            signalling_state: SignallingState::None,

            /* "-" indicates no username */
            origin_username: String::from("-"),

            ice_agent: libnice::ice::Agent::new_full(event_loop, NiceCompatibility::RFC5245, NiceAgentOption_NICE_AGENT_OPTION_ICE_TRICKLE),
            transport: BTreeMap::new(),

            stream_receiver: BTreeMap::new(),
            stream_sender: BTreeMap::new(),

            application_channel: Box::new(ChannelApplication::new().expect("failed to allocate new application channel")),

            media_lines: Vec::new(),
            local_events: mpsc::unbounded_channel()
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

    pub fn media_lines(&self) -> &Vec<Rc<RefCell<MediaLine>>> {
        &self.media_lines
    }

    pub fn set_remote_description(&mut self, description: &webrtc_sdp::SdpSession, mode: &RtcDescriptionType) -> Result<(), RemoteDescriptionApplyError> {
        if mode == &RtcDescriptionType::Offer {
            if !matches!(&self.signalling_state, &SignallingState::None | &SignallingState::Negotiated) {
                return Err(RemoteDescriptionApplyError::InvalidNegotiationState);
            }
        } else {
            if !matches!(&self.signalling_state, &SignallingState::HaveLocalOffer) {
                return Err(RemoteDescriptionApplyError::InvalidNegotiationState);
            }
        }

        if self.media_lines.len() > description.media.len() {
            return Err(RemoteDescriptionApplyError::MissingMediaLines);
        }

        /* copy the origin username and send it back, required for mozilla for example */
        self.origin_username = description.origin.username.clone();

        for media_line in 0..description.media.len() {
            let media = &description.media[media_line as usize];

            let credentials = ICECredentials {
                username: get_attribute_value!(media, media_line, IceUfrag)?.clone(),
                password: get_attribute_value!(media, media_line, IcePwd)?.clone()
            };

            let setup = get_attribute_value!(media, media_line, Setup)?;
            let local_setup = {
                match setup {
                    SdpAttributeSetup::Passive => Ok(SdpAttributeSetup::Active),
                    SdpAttributeSetup::Active => Ok(SdpAttributeSetup::Passive),
                    SdpAttributeSetup::Actpass => Ok(ACT_PASS_DEFAULT_SETUP_TYPE),
                    _ => Err(RemoteDescriptionApplyError::IceSetupUnsupported { media_index: media_line })
                }
            }?;

            if let Some(line) = self.media_lines.get(media_line) {
                /* we've to update the line */
                let line = line.clone();
                let mut line = RefCell::borrow_mut(&line);

                let transport = self.transport.get(&line.transport_id);
                if transport.is_none() {
                    return Err(RemoteDescriptionApplyError::InternalError { detail: String::from("missing transport for media line") });
                }
                let transport = Rc::clone(&transport.unwrap());
                let mut transport = RefCell::borrow_mut(&transport);

                if transport.remote_credentials != credentials {
                    return Err(RemoteDescriptionApplyError::MissMatchingIceSettings{ media_index: media_line as u32 });
                }

                if format!("{}", &transport.setup) != format!("{}", &local_setup) {
                    return Err(RemoteDescriptionApplyError::MissMatchingIceSettings{ media_index: media_line as u32 });
                }

                line.update_from_sdp(media)
                    .map_err(|err| RemoteDescriptionApplyError::MediaLineParseError { media_line: media_line as u32, error: err })?;

                if line.media_type == SdpMediaValue::Application {
                    self.application_channel.set_remote_description(media)
                        .map_err(|err| RemoteDescriptionApplyError::MediaChannelConfigure { error: err })?;
                }

                self.update_media_line_streams(media, line.deref_mut(), transport.deref_mut());
            } else {
                let ice_channel = self.create_ice_channel(&credentials, media_line as u32, local_setup)?;
                let mut ice_channel_mut = RefCell::borrow_mut(&ice_channel);

                let mut line = MediaLine::new_from_sdp(media_line as u32, media)
                    .map_err(|err| RemoteDescriptionApplyError::MediaLineParseError { media_line: media_line as u32, error: err })?;

                if ice_channel_mut.owning_media_line == media_line as u32 {
                    /* yeah it's our channel */
                    for candidate in media.get_attributes_of_type(SdpAttributeType::Candidate)
                        .iter()
                        .map(|attribute| if let SdpAttribute::Candidate(c) = attribute { c } else { panic!("expected a candidate") }) {

                        ice_channel_mut.add_remote_candidate(Some(candidate))
                            .map_err(|err| RemoteDescriptionApplyError::InternalError { detail: String::from(format!("failed to add candidate: {:?}", err)) })?;
                    }

                    /*
                    /* Firefox does not sends this stuff */
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

                line.transport_id = ice_channel_mut.transport_id;
                ice_channel_mut.media_lines.push(line.index);

                if line.media_type == SdpMediaValue::Application {
                    self.application_channel.set_remote_description(media)
                        .map_err(|err| RemoteDescriptionApplyError::InternalError { detail: String::from(format!("failed to set remote description: {:?}", err)) })?;
                    self.application_channel.transport = Some(ice_channel_mut.control_sender.clone());
                    /* TODO: Trigger the handle_transport_initialized event if the transport has already been initialized */
                    //self.application_channel.handle_transport_initialized();
                }

                self.update_media_line_streams(media, &mut line, ice_channel_mut.deref_mut());
                self.media_lines.push(Rc::new(RefCell::new(line)));
            }
        }

        match &self.signalling_state {
            &SignallingState::HaveLocalOffer => {
                for line in self.media_lines.iter() {
                    let mut line = RefCell::borrow_mut(line);
                    if line.negotiation_state == NegotiationState::Propagated {
                        line.negotiation_state = NegotiationState::Negotiated;
                    }
                }
                for sender in self.stream_sender.values_mut() {
                    sender.promote_negotiation(|s| matches!(s, NegotiationState::Propagated), NegotiationState::Negotiated);
                }

                self.signalling_state = SignallingState::Negotiated;
            },
            &SignallingState::None |
            &SignallingState::Negotiated => {
                self.signalling_state = SignallingState::HaveRemoteOffer;
            },
            _ => panic!()
        }

        Ok(())
    }

    fn update_media_line_streams(&mut self, media: &SdpMedia, media_line: &mut MediaLine, transport: &mut RTCTransport) {
        self.stream_receiver.drain_filter(|id, receiver| {
            if receiver.track().media_line != media_line.index {
                return false;
            }

            !media_line.remote_streams.contains(id)
        }).count();

        for receiver_id in media_line.remote_streams.iter() {
            if self.stream_receiver.contains_key(&receiver_id) {
                /* TODO: Check if codec or formats have changed! */
                continue;
            }

            println!("RTP Stream got new {} on {}", receiver_id, media_line.index);
            let (tx, rx) = mpsc::unbounded_channel();

            let (ctx, crx) = mpsc::unbounded_channel();
            let mut internal_receiver = ActiveInternalMediaReceiver {
                track: InternalMediaTrack {
                    id: *receiver_id,
                    media_line: media_line.index,

                    transport_id: media_line.transport_id
                },

                event_sender: tx,
                control_receiver: crx,

                properties: HashMap::new(),

                resend_requester: RtpPacketResendRequester::new(),
                rtcp_sender: transport.create_rtcp_sender()
            };
            internal_receiver.parse_properties_from_sdp(media);

            let receiver = MediaReceiver {
                id: *receiver_id,
                media_line: media_line.index,

                events: rx,
                control: ctx
            };

            let _ = self.local_events.0.send(PeerConnectionEvent::ReceivedRemoteStream(receiver));
            self.stream_receiver.insert(*receiver_id, Box::new(internal_receiver));
        }
    }

    pub fn create_local_description(&mut self) -> Result<SdpSession, CreateAnswerError> {
        if matches!(&self.signalling_state, &SignallingState::HaveLocalOffer | &SignallingState::Negotiated) {
            return Err(CreateAnswerError::InvalidNegotiationState);
        }

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
            tags: self.media_lines.iter().map(|e| RefCell::borrow(e).id.clone()).collect::<Vec<String>>()
        })).unwrap();

        /* flush all pending stream modifications */
        self.flush_transceiver_control();
        for media_line in self.media_lines.iter() {
            let media_line = RefCell::borrow(media_line);
            let mut media = {
                if media_line.media_type == SdpMediaValue::Application {
                    let mut media = self.application_channel.generate_local_description()
                        .map_err(|err| CreateAnswerError::InternalError(err))?;

                    media.add_attribute(SdpAttribute::Mid(media_line.id.clone()))
                        .unwrap();
                    media
                } else {
                    let mut media = media_line.generate_local_description()
                        .ok_or(CreateAnswerError::DescribeError(media_line.index))?;

                    for sender in self.stream_sender.values_mut()
                        .filter(|sender| sender.track.media_line == media_line.index) {
                        sender.write_sdp(&mut media);
                    }

                    media
                }
            };

            let ice_channel = self.transport.get(&media_line.transport_id);
            if ice_channel.is_none() {
                return Err(CreateAnswerError::MissingTransportChannel(media_line.index));
            }
            let ice_channel = RefCell::borrow(ice_channel.unwrap());

            media.add_attribute(SdpAttribute::IceUfrag(ice_channel.local_credentials.username.clone())).unwrap();
            media.add_attribute(SdpAttribute::IcePwd(ice_channel.local_credentials.password.clone())).unwrap();
            media.add_attribute(SdpAttribute::IceOptions(vec![String::from("trickle")])).unwrap();
            media.add_attribute(SdpAttribute::Fingerprint(ice_channel.fingerprint.clone())).unwrap();
            media.add_attribute(SdpAttribute::Setup(ice_channel.setup.clone())).unwrap();

            answer.media.push(media);
        }

        match &self.signalling_state {
            &SignallingState::None |
            &SignallingState::NegotiationRequired => {
                for line in self.media_lines.iter() {
                    let mut line = RefCell::borrow_mut(line);
                    if line.negotiation_state == NegotiationState::Changed ||
                        line.negotiation_state == NegotiationState::None {
                        line.negotiation_state = NegotiationState::Propagated;
                    }
                }
                for sender in self.stream_sender.values_mut() {
                    sender.promote_negotiation(|s| matches!(s, NegotiationState::Changed | NegotiationState::None), NegotiationState::Propagated);
                }
                self.signalling_state = SignallingState::HaveLocalOffer;
            },
            &SignallingState::HaveRemoteOffer => {
                for line in self.media_lines.iter() {
                    let mut line = RefCell::borrow_mut(line);
                    line.negotiation_state = NegotiationState::Negotiated;
                }
                for sender in self.stream_sender.values_mut() {
                    sender.promote_negotiation(|_| true, NegotiationState::Negotiated);
                }
                self.signalling_state = SignallingState::Negotiated;
            }
            _ => panic!() /* this other cases should never happen, they're already caught in the first few lines */
        }

        Ok(answer)
    }

    fn allocate_sender_media_line(&mut self, media_type: SdpMediaValue) -> Rc<RefCell<MediaLine>> {
        for line in self.media_lines.iter() {
            let ref_line = RefCell::borrow(line);
            if ref_line.media_type != media_type { continue; }
            if !ref_line.local_streams.is_empty() { continue; }
            return line.clone();
        }

        let mut line = MediaLine::new(self.media_lines.len() as u32,String::from(format!("{}", self.media_lines.len())), media_type);

        /* FIXME: If no transport exists create one! */
        let transport = RefCell::borrow(self.transport.values().find(|_| true).unwrap());
        line.transport_id = transport.transport_id;

        let line = Rc::new(RefCell::new(line));
        self.media_lines.push(line.clone());
        return line;
    }

    pub fn add_media_sender(&mut self, media_type: SdpMediaValue) -> MediaSender {
        /* find or create a free media line */
        let media_line = self.allocate_sender_media_line(media_type);
        let mut media_line = RefCell::borrow_mut(&media_line);

        let mut transport = RefCell::borrow_mut(self.transport.get(&media_line.transport_id).expect("missing transport"));
        let (internal_sender, sender) = InternalMediaSender::new(
            InternalMediaTrack {
                /* TODO: Ensure this hasn't been used already */
                id: rand::random::<u32>(),
                media_line: media_line.index,

                transport_id: media_line.transport_id
            },
            (transport.create_rtp_sender(), transport.create_rtcp_sender())
        );

        media_line.local_streams.push(internal_sender.track.id);
        self.stream_sender.insert(internal_sender.track.id, internal_sender);
        sender
    }

    pub fn create_data_channel(&mut self, channel_type: DataChannelType, label: String, protocol: Option<String>, priority: u16) -> Result<DataChannel, String> {
        /* TODO: Trigger renegotiation if we're not yet having a application media line */
        self.application_channel.create_data_channel(channel_type, label, protocol, priority)
    }

    /// Adding a remote ice candidate.
    /// To signal a no more candidates event just add `None`
    pub fn add_remote_ice_candidate(&mut self, media_line_index: u32, candidate: Option<&Candidate>) -> Result<(), RTCTransportICECandidateAddError> {
        if let Some(channel) = self.find_ice_channel_by_media_fragment(media_line_index) {
            /* in theory the channel should not be borrowed elsewhere */
            let mut channel = RefCell::borrow_mut(channel);

            if channel.owning_media_line == media_line_index {
                channel.add_remote_candidate(candidate)
            } else {
                Ok(())
            }
        } else {
            Err(RTCTransportICECandidateAddError::UnknownMediaChannel)
        }
    }

    fn create_ice_channel(&mut self, credentials: &ICECredentials, media_line: u32, setup: SdpAttributeSetup) -> Result<Rc<RefCell<RTCTransport>>, RemoteDescriptionApplyError> {
        if let Some((_, channel)) = self.transport.iter().find(|entry| {
            let entry = RefCell::borrow(entry.1);
            &entry.remote_credentials == credentials
        }) {
            if RefCell::borrow(channel).setup.to_string() != setup.to_string() {
                Err(RemoteDescriptionApplyError::MixedIceSetupStates {})
            } else {
                Ok(channel.clone())
            }
        } else {
            println!("Creating a new transport channel");
            /* register a new channel */
            let stream = libnice::ice::Agent::stream_builder(&mut self.ice_agent, 1).build()
                .map_err(|error| RemoteDescriptionApplyError::FailedToAddIceStream { error })?;

            #[allow(unused_mut)]
            let mut connection = RTCTransport::new(stream, credentials.clone(), media_line, setup)
                .map_err(|err| RemoteDescriptionApplyError::IceInitializeError { result: err, media_id: media_line })?;

            /* FIXME! */
            #[cfg(feature = "simulated-loss")]
            {
                //connection.set_simulated_loss(10);
            }

            let id = connection.transport_id;
            let connection = Rc::new(RefCell::new(connection));
            self.transport.insert(id, connection.clone());

            Ok(connection)
        }
    }

    fn find_ice_channel_by_media_fragment(&self, media_line: u32) -> Option<&Rc<RefCell<RTCTransport>>> {
        self.transport.iter().find(|channel| RefCell::borrow(channel.1).media_lines.iter().find(|media| **media == media_line).is_some())
            .map(|e| e.1)
    }

    fn handle_ice_event(&mut self, ice: &mut RefMut<RTCTransport>, event: RTCTransportEvent) -> Option<PeerConnectionEvent> {
        match event {
            RTCTransportEvent::LocalIceCandidate(candidate) => {
                return Some(PeerConnectionEvent::LocalIceCandidate(Some(candidate.into()), ice.owning_media_line));
            },
            RTCTransportEvent::LocalIceGatheringFinished() => {
                return Some(PeerConnectionEvent::LocalIceCandidate(None, ice.owning_media_line));
            },
            RTCTransportEvent::TransportStateChanged => {
                /* TODO: Propagate state changes from Connected to any thing else to the streams */
                println!("Transport state change to {:?}", ice.state());
                match ice.state() {
                    &RTCTransportState::Connected => {
                        self.application_channel.handle_transport_connected();
                    },
                    _ => {}
                }
                None
            },
            RTCTransportEvent::MessageReceivedDtls(message) => {
                /* TODO: Test if the ice transport we're receiving it from matches our application channel */
                self.application_channel.handle_data(message);
                None
            },
            RTCTransportEvent::MessageReceivedRtcp(message) => {
                let mut packets = [&[0u8][..]; 128];
                let packet_count = RtcpPacket::split_up_packets(message.as_slice(), &mut packets[..]);
                if let Err(error) = packet_count {
                    eprintln!("Received invalid merged packet: {:?}", error);
                    return None;
                }

                for index in 0..packet_count.unwrap() {
                    match RtcpPacket::parse(packets[index]) {
                        Ok(packet) => {
                            /*
                            let mut buffer = [0u8; 2038];
                            match packet.write(&mut buffer) {
                                Err(error) => {
                                    eprintln!("Failed to write received RTCP packet: {:?}", error);
                                },
                                Ok(length) => {
                                    if packets[index] != &buffer[0..length] {
                                        /*
                                            FF Pads the SourceDescription elements invalid (https://bugzilla.mozilla.org/show_bug.cgi?id=1671169).
                                            Example: The CName is 38 characters long adding two (one byte for the description type, the other for the length) results in a length of 40.
                                            40 has no need to be padded (already on a 32bit boundary). For some reason FF padds the message with two bytes. This results later on in the padding of four zero bytes and result in an over all invalid packet
                                            Parsed packet: SourceDescription(RtcpPacketSourceDescription { descriptions: [(1308369285, CName("{4b1d1d86-d4bc-44a2-a92d-6e47ee1ed6a3}"))] })
                                            Created packet is different than source packet:
                                            Source:  [129, 202, 0, 12, 77, 252, 33, 133, 1, 38, 123, 52, 98, 49, 100, 49, 100, 56, 54, 45, 100, 52, 98, 99, 45, 52, 52, 97, 50, 45, 97, 57, 50, 100, 45, 54, 101, 52, 55, 101, 101, 49, 101, 100, 54, 97, 51, 125, 0, 0, 0, 0]
                                            Created: [129, 202, 0, 12, 77, 252, 33, 133, 1, 38, 123, 52, 98, 49, 100, 49, 100, 56, 54, 45, 100, 52, 98, 99, 45, 52, 52, 97, 50, 45, 97, 57, 50, 100, 45, 54, 101, 52, 55, 101, 101, 49, 101, 100, 54, 97, 51, 125]
                                         */
                                        eprintln!("Parsed packet: {:?}", packet);
                                        eprintln!("Created packet is different than source packet:\nSource:  {:?}\nCreated: {:?}", &packets[index], &buffer[0..length]);
                                    }
                                }
                            }
                            */

                            match packet {
                                RtcpPacket::ReceiverReport(mut rr) => {
                                    let app_data = rr.profile_data.take();
                                    for (id, report) in rr.reports {
                                        if let Some(sender) = self.stream_sender.get_mut(&id) {
                                            sender.handle_receiver_report(report, &app_data);
                                        }
                                    }
                                },
                                RtcpPacket::SenderReport(sr) => {
                                    if let Some(receiver) = self.stream_receiver.get_mut(&sr.ssrc) {
                                        receiver.handle_sender_report(sr);
                                    } else {
                                        let _ = self.local_events.0.send(PeerConnectionEvent::UnassignableRtcpPacket(RtcpPacket::SenderReport(sr)));
                                    }
                                },
                                RtcpPacket::SourceDescription(sd) => {
                                    for (id, description) in sd.descriptions.iter() {
                                        if let Some(receiver) = self.stream_receiver.get_mut(&id) {
                                            receiver.handle_source_description(description);
                                        }
                                    }
                                },
                                RtcpPacket::Bye(bye) => {
                                    eprintln!("Received bye packet: {:?}", bye);
                                    /* TODO: Remove remote stream(s) without nego */
                                },
                                RtcpPacket::TransportFeedback(fb) => {
                                    if let Some(sender) = self.stream_sender.get_mut(&fb.media_ssrc) {
                                        sender.handle_transport_feedback(fb.feedback);
                                    } else {
                                        let _ = self.local_events.0.send(PeerConnectionEvent::UnassignableRtcpPacket(RtcpPacket::TransportFeedback(fb)));
                                    }
                                },
                                RtcpPacket::PayloadFeedback(pfb) => {
                                    if let Some(sender) = self.stream_sender.get_mut(&pfb.media_ssrc) {
                                        sender.handle_payload_feedback(pfb.feedback);
                                    } else {
                                        let _ = self.local_events.0.send(PeerConnectionEvent::UnassignableRtcpPacket(RtcpPacket::PayloadFeedback(pfb)));
                                    }
                                },
                                RtcpPacket::ExtendedReport(xr) => {
                                    /* TODO: What to do here? We can't really assign the report to any media sender/receiver... */
                                    /*
                                    if let Some(sender) = self.stream_sender.get_mut(&xr.ssrc) {
                                        sender.handle_extended_report(xr);
                                    } else if let Some(receiver) = self.stream_receiver.get_mut(&xr.ssrc) {
                                        receiver.handle_extended_report(xr);
                                    } else {
                                        let _ = self.local_events.0.send(PeerConnectionEvent::UnassignableRtcpPacket(RtcpPacket::ExtendedReport(xr)));
                                    }
                                    */
                                    let _ = self.local_events.0.send(PeerConnectionEvent::UnassignableRtcpPacket(RtcpPacket::ExtendedReport(xr)));
                                },
                                RtcpPacket::Unknown(data) => {
                                    self.stream_receiver.iter_mut().for_each(|receiver|
                                        receiver.1.handle_unknown_rtcp(&data)
                                    );

                                    self.stream_sender.iter_mut().for_each(|sender|
                                        sender.1.handle_unknown_rtcp(&data)
                                    );
                                }
                            }
                        },
                        Err(error) => {
                            eprintln!("Failed to decode RTCP packet: {:?}", error);
                        }
                    }
                }
                None
            },
            RTCTransportEvent::MessageReceivedRtp(message) => {
                match ParsedRtpPacket::new(message) {
                    Ok(reader) => {
                        if let Some(receiver) = self.stream_receiver.get_mut(&reader.ssrc()) {
                            if receiver.track().transport_id != ice.transport_id {
                                eprintln!("Received RTP message for receiver, but receiver isn't registered to that transport");
                            } else {
                                receiver.handle_rtp_packet(reader);
                            }
                        } else {
                            let _ = self.local_events.0.send(PeerConnectionEvent::UnassignableRtpPacket(reader));
                        }
                    },
                    Err((error, _)) => {
                        /* TODO: Don't log spam here */
                        eprintln!("Failed to decode RTP packet: {:?}", error);
                    }
                }
                None
            },
            RTCTransportEvent::MessageDropped(message) => {
                println!("Dropping received ICE message of length {}", message.len());
                None
            },
            _ => {
                None
            }
        }

    }

    fn poll_transceiver_control(&mut self, cx: &mut Context<'_>) {
        let removed = self.stream_receiver.drain_filter(|_, receiver| {
            if let Poll::Ready(_) = receiver.poll_unpin(cx) {
                true
            } else {
                false
            }
        }).collect::<Vec<_>>();

        for (id, rc) in removed {
            self.stream_receiver.insert(id, rc.into_void());
            println!("Media stream {} receiver has no end point. Voiding it.", id);
        }

        let removed = self.stream_sender.drain_filter(|_, sender| {
            if let Poll::Ready(_) = sender.poll_unpin(cx) {
                true
            } else {
                false
            }
        }).collect::<Vec<_>>();

        for (_, sender) in removed {
            let media_line = self.media_lines.iter()
                .find(|line| RefCell::borrow(line).index == sender.track.media_line);
            if media_line.is_some() {
                let mut media_line = RefCell::borrow_mut(media_line.unwrap());
                media_line.local_streams.retain(|e| *e != sender.track.id);
                if media_line.negotiation_state != NegotiationState::None {
                    media_line.negotiation_state = NegotiationState::Changed;
                }
            }
        }
    }

    fn flush_transceiver_control(&mut self) {
        for receiver in self.stream_receiver.values_mut() {
            receiver.flush_control();
        }

        for sender in self.stream_sender.values_mut() {
            sender.flush_control();
        }
    }
}

unsafe impl Send for PeerConnection {}

impl futures::stream::Stream for PeerConnection {
    type Item = PeerConnectionEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Poll::Ready(message) = self.local_events.1.poll_next_unpin(cx) {
            return Poll::Ready(Some(message.expect("unexpected local event stream close")));
        }

        self.poll_transceiver_control(cx);

        while let Poll::Ready(event) = self.application_channel.poll_next_unpin(cx) {
            let event = event.expect("unexpected stream end");
            match event {
                ApplicationChannelEvent::DataChannelReceived(channel) => {
                    return Poll::Ready(Some(PeerConnectionEvent::ReceivedDataChannel(channel)));
                },
                ApplicationChannelEvent::StateChanged { new_state: _ } => {
                    /* TODO: Track the application channel state */
                }
            }
        }

        let streams = self.transport.clone();
        for (_, stream) in streams.iter() {
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

        if self.signalling_state == SignallingState::Negotiated {
            let mut negotiation_required = false;
            let changed_mline = self.media_lines.iter()
                .find(|e| matches!(RefCell::borrow(e).negotiation_state(), NegotiationState::None | NegotiationState::Changed));
            if changed_mline.is_some() {
                negotiation_required = true;
            }
            if !negotiation_required && self.stream_sender.values()
                .find(|e| e.negotiation_needed()).is_some() {
                negotiation_required = true;
            }
            if negotiation_required {
                self.signalling_state = SignallingState::NegotiationRequired;
                return Poll::Ready(Some(PeerConnectionEvent::NegotiationNeeded));
            }
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod test {
    use crate::utils::rtcp::RtcpPacket;

    const FIREFOX_PACKET: [u8; 52] = [129, 202, 0, 12, 77, 252, 33, 133, 1, 38, 123, 52, 98, 49, 100, 49, 100, 56, 54, 45, 100, 52, 98, 99, 45, 52, 52, 97, 50, 45, 97, 57, 50, 100, 45, 54, 101, 52, 55, 101, 101, 49, 101, 100, 54, 97, 51, 125, 0, 0, 0, 0];

    #[test]
    fn test_packet_split_up() {
        let mut packets = [&[0u8][..]; 128];
        let packet_count = RtcpPacket::split_up_packets(&FIREFOX_PACKET[..], &mut packets[..]);
        assert_eq!(packet_count.unwrap(), 1usize);
    }

    #[test]
    fn test_packet_parse() {
        let parsed = RtcpPacket::parse(&FIREFOX_PACKET[..]).expect("failed to decode valid packet");
        println!("{:?}", parsed);
    }
}