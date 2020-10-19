#![allow(dead_code)]

use tokio::sync::mpsc;
use crate::rtc::{ACT_PASS_DEFAULT_SETUP_TYPE};
use webrtc_sdp::media_type::{SdpMedia, SdpMediaLine, SdpMediaValue, SdpFormatList, SdpProtocolValue};
use crate::transport::{RTCTransportControl};
use webrtc_sdp::attribute_type::{SdpAttribute, SdpAttributeSetup, SdpAttributeSctpmap, SdpAttributeType};
use webrtc_sdp::SdpConnection;
use webrtc_sdp::address::ExplicitlyTypedAddress;
use std::net::{IpAddr, Ipv4Addr};
use crate::sctp::{UsrSctpSession, UsrSctpSessionEvent, SctpSendInfo};
use futures::task::{Context, Poll, Waker};
use tokio::macros::support::Pin;
use futures::{StreamExt, Stream};
use std::io::{Read, Write};
use std::cell::RefCell;
use std::rc::Rc;
use std::ops::{DerefMut, Deref};
use std::collections::{VecDeque, BTreeMap};
use futures::io::ErrorKind;
use crate::sctp::notification::{SctpNotificationType, SctpNotification, SctpNotificationStreamReset, SctpNotificationAssocChange, SctpSacState};
use crate::sctp::message::{DataChannelMessage as RawDataChannelMessage, DataChannelType, DataChannelControlMessage, DataChannelControlMessageOpenAck, DataChannelControlMessageOpen};
use crate::sctp::sctp_macros::{SCTP_ALL_ASSOC, SCTP_STREAM_RESET_DENIED, SCTP_STREAM_RESET_FAILED, SCTP_STREAM_RESET_INCOMING_SSN, SPP_PMTUD_ENABLE, SPP_PMTUD_DISABLE, SCTP_EOR, SCTP_UNORDERED};

/// Events which are emitted by the `ApplicationChannel`.
pub enum ApplicationChannelEvent {
    DataChannelReceived(DataChannel),
    StateChanged{ new_state: ApplicationChannelState }
}

#[derive(Debug, PartialEq)]
pub enum ApplicationChannelState {
    Disconnected,
    Connecting,
    Connected
}

struct RemoteConfig {
    port: u16
}

struct InternalDataChannel {
    state: DataChannelState,
    channel_type: DataChannelType,
    priority: u16,
    label: String,
    protocol: String,

    internal_channel_id: u16,
    channel_id: u32,
    event_sender: mpsc::UnboundedSender<DataChannelEvent>
}

/// The implementation for the application media channel aka Data Channels
pub(crate) struct ChannelApplication {
    state: ApplicationChannelState,

    max_incoming_channel: u16, /* Don't modify once initialized */
    max_outgoing_channel: u16, /* Don't modify once initialized */

    remote_config: Option<RemoteConfig>,
    poll_waker: Option<Waker>,

    sctp_session: UsrSctpSession<SctpStream>,
    stream: Rc<RefCell<SctpStreamInner>>,
    pending_stream_resets: Vec<u16>,

    dtls_role_client: bool,

    pub transport_id: u32,
    pub transport: Option<mpsc::UnboundedSender<RTCTransportControl>>,

    internal_channels: BTreeMap<u16, Rc<RefCell<InternalDataChannel>>>,
    channel_id_index: u32,

    channel_communication_receiver: mpsc::UnboundedReceiver<DataChannelAction>,
    channel_communication_sender: mpsc::UnboundedSender<DataChannelAction>,
}

impl ChannelApplication {
    pub fn new() -> Option<Self> {
        let stream = Rc::new(RefCell::new(SctpStreamInner {
            read_queue: VecDeque::new(),
            read_index: 0,
            write_queue: VecDeque::new(),
            write_waker: None
        }));

        let mut sctp_session = UsrSctpSession::new(SctpStream{ inner: stream.clone() }, 5000);
        if sctp_session.is_none() { return None; }

        let _ = sctp_session.as_mut().unwrap().set_linger(0)
            .map_err(|err| eprintln!("failed to disable linger: {}", err));

        let (tx, rx) = mpsc::unbounded_channel();
        Some(ChannelApplication {
            state: ApplicationChannelState::Disconnected,

            max_incoming_channel: 1024,
            max_outgoing_channel: 1024,

            stream,
            sctp_session: sctp_session.unwrap(),
            pending_stream_resets: Vec::new(),

            transport_id: 0,
            transport: None,

            dtls_role_client: true, /* initially assume we're the client */
            poll_waker: None,
            remote_config: None,

            internal_channels: BTreeMap::new(),
            channel_id_index: 0,

            channel_communication_receiver: rx,
            channel_communication_sender: tx
        })
    }

    pub fn create_data_channel(&mut self, channel_type: DataChannelType, label: String, protocol: Option<String>, priority: u16) -> Result<DataChannel, String> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut internal_channel = InternalDataChannel {
            channel_id: self.channel_id_index,
            internal_channel_id: 0,

            channel_type,

            label,
            protocol: if protocol.is_some() { protocol.unwrap() } else { String::new() },
            priority,

            state: DataChannelState::Closed,
            event_sender: tx
        };

        let mut internal_channel_id = if self.dtls_role_client { 0 } else { 1 };
        while self.internal_channels.contains_key(&internal_channel_id) {
            let (id, overflow) = internal_channel_id.overflowing_add(2);
            internal_channel_id = id;
            if overflow {
                return Err(String::from("max amount of hardware channels reached"));
            }
        }
        if internal_channel_id as u32 >= self.max_outgoing_channel as u32 * 2 {
            return Err(String::from("max amount of channels reached"));
        }
        internal_channel.internal_channel_id = internal_channel_id;

        if self.state == ApplicationChannelState::Connected {
            internal_channel.state = DataChannelState::Connecting;
            self.send_open_request(&internal_channel);
            self.send_message(&internal_channel, &RawDataChannelMessage::String(String::from("HEY!")), false);
        }

        let channel = DataChannel {
            channel_id: internal_channel.channel_id,
            internal_channel_id: internal_channel.internal_channel_id,

            channel_type: internal_channel.channel_type,
            label: internal_channel.label.clone(),
            protocol: internal_channel.protocol.clone(),

            state: internal_channel.state,
            events: rx,
            channel_sender: self.channel_communication_sender.clone()
        };

        self.channel_id_index = self.channel_id_index.wrapping_add(1);
        self.internal_channels.insert(internal_channel_id, Rc::new(RefCell::new(internal_channel)));
        Ok(channel)
    }

    fn configure_sctp(&mut self) -> Result<(), std::io::Error> {
        /* throws constantly an error right now */
        //self.sctp_session.set_send_buffer(1024 * 1024)?;
        //self.sctp_session.set_receive_buffer(1024 * 1024)?;

        self.sctp_session.toggle_non_block(true)?;
        self.sctp_session.toggle_assoc_resets(SCTP_ALL_ASSOC, true)?;
        self.sctp_session.toggle_no_delay(true)?;
        self.sctp_session.set_init_parameters(self.max_outgoing_channel, self.max_incoming_channel, 0, 0)?;

        self.sctp_session.toggle_notifications(SctpNotificationType::StreamResetEvent, true)?;
        self.sctp_session.toggle_notifications(SctpNotificationType::AssocChange, true)?;
        //self.sctp_session.toggle_notifications(SctpNotificationType::StreamChangeEvent, true)?; /* TODO! */
        Ok(())
    }

    fn process_sctp_message(&mut self, channel_id: u16, message: RawDataChannelMessage) -> Option<ApplicationChannelEvent> {
        match message {
            RawDataChannelMessage::Control(message) => self.process_control_message(channel_id, message),
            RawDataChannelMessage::Binary(_) |
            RawDataChannelMessage::BinaryEmpty() |
            RawDataChannelMessage::String(_) |
            RawDataChannelMessage::StringEmpty() => {
                if let Some(channel) = self.internal_channels.get(&channel_id) {
                    let channel = RefCell::borrow(channel);
                    if matches!(channel.state, DataChannelState::Open | DataChannelState::Connecting) {
                        let event = {
                            DataChannelEvent::MessageReceived(match message {
                                RawDataChannelMessage::BinaryEmpty() => DataChannelMessage::Binary(None),
                                RawDataChannelMessage::Binary(buffer) => DataChannelMessage::Binary(Some(buffer)),
                                RawDataChannelMessage::StringEmpty() => DataChannelMessage::String(None),
                                RawDataChannelMessage::String(text) => DataChannelMessage::String(Some(text)),
                                _ => panic!()
                            })
                        };
                        let _ = channel.event_sender.send(event);
                    }
                } else {
                    eprintln!("Received SCTP message for invalid channel {}.", channel_id);
                }
                None
            }
        }
    }

    fn process_control_message(&mut self, internal_channel_id: u16, message: DataChannelControlMessage) -> Option<ApplicationChannelEvent> {
        match message {
            DataChannelControlMessage::Open(request) => {
                /*
                   If one side wants to open a Data Channel, it chooses a Stream
                   Identifier for which the corresponding incoming and outgoing Streams
                   are free.  If the side is the DTLS client, it MUST choose an even
                   Stream Identifier, if the side is the DTLS server, it MUST choose an
                   odd one.  It fills in the parameters of the DATA_CHANNEL_OPEN message
                   and sends it on the chosen Stream.
                 */
                if (internal_channel_id & 1) == if self.dtls_role_client { 0 } else { 1 } {
                    /* Error handling described in https://tools.ietf.org/html/draft-ietf-rtcweb-data-protocol-08#section-6 */
                    eprintln!("Remote peer tried to open a data channel on an invalid stream id");
                    self.pending_stream_resets.push(internal_channel_id);
                    self.send_outgoing_stream_resets();
                    return None;
                }

                if self.internal_channels.contains_key(&internal_channel_id) {
                    /* Error handling described in https://tools.ietf.org/html/draft-ietf-rtcweb-data-protocol-08#section-6 */
                    println!("Remote peer tried to open a data channel on a stream which already contains a channel. Closing data channel.");
                    let channel = self.internal_channels.get(&internal_channel_id).unwrap().clone();
                    let mut channel = RefCell::borrow_mut(&channel);
                    self.close_datachannel_locally(channel.deref_mut());
                    return None;
                }

                let channel_id = self.channel_id_index;

                let (tx, rx) = mpsc::unbounded_channel();
                let internal_channel = InternalDataChannel {
                    channel_id,
                    internal_channel_id,

                    channel_type: request.channel_type,
                    priority: request.priority,

                    label: request.label.clone(),
                    protocol: request.protocol.clone(),

                    /* directly set state to open since we're only the side who have to acknowledge the channel */
                    state: DataChannelState::Open,
                    event_sender: tx
                };

                let channel = DataChannel {
                    channel_id,
                    internal_channel_id,

                    channel_type: request.channel_type,
                    state: DataChannelState::Open,

                    label: request.label.clone(),
                    protocol: request.protocol.clone(),

                    events: rx,
                    channel_sender: self.channel_communication_sender.clone()
                };

                self.channel_id_index = self.channel_id_index.wrapping_add(1);
                self.send_open_acknowledge(&internal_channel);
                self.internal_channels.insert(internal_channel_id, Rc::new(RefCell::new(internal_channel)));

                return Some(ApplicationChannelEvent::DataChannelReceived(channel))
            },
            DataChannelControlMessage::OpenAck(_) => {
                if let Some(channel) = self.internal_channels.get(&internal_channel_id) {
                    let mut channel = RefCell::borrow_mut(channel);
                    if channel.state != DataChannelState::Connecting {
                        eprintln!("Received OpenAck for channel which isn't in connection state any more");
                    } else {
                        channel.state = DataChannelState::Open;
                        let _ = channel.event_sender.send(DataChannelEvent::StateChanged(DataChannelState::Open));
                    }
                } else {
                    eprintln!("Received OpenAck for unknown channel {:?}", internal_channel_id);
                }
            }
        }
        None
    }

    fn process_sctp_notification(&mut self, notification: &SctpNotification) -> Option<ApplicationChannelEvent> {
        match notification {
            SctpNotification::StreamReset(event) => self.process_sctp_notification_stream_reset(event),
            SctpNotification::AssocChange(event) => self.process_sctp_notification_assoc_change(event),
            _ => {
                println!("Received unexpected SCTP notification: {:?}", notification);
                None
            }
        }
    }

    fn process_sctp_notification_stream_reset(&mut self, notification: &SctpNotificationStreamReset) -> Option<ApplicationChannelEvent> {
        if (notification.flags & SCTP_STREAM_RESET_DENIED as u16) == 0 &&
            (notification.flags & SCTP_STREAM_RESET_FAILED as u16) == 0 &&
            (notification.flags & SCTP_STREAM_RESET_INCOMING_SSN as u16) != 0 {
            for channel_id in notification.streams.iter() {
                if let Some(channel) = self.internal_channels.get(channel_id) {
                    let channel = RefCell::borrow_mut(channel);
                    match channel.state {
                        DataChannelState::Connecting |
                        DataChannelState::Open |
                        DataChannelState::Closing => {
                            let _ = channel.event_sender.send(DataChannelEvent::StateChanged(DataChannelState::Closed));
                        },
                        DataChannelState::Closed => {}
                    }
                } else {
                    println!("Received remote stream reset notification for an invalid channel");
                }

                self.internal_channels.remove(channel_id);
            }
        }

        self.send_outgoing_stream_resets();
        None
    }

    fn process_sctp_notification_assoc_change(&mut self, notification: &SctpNotificationAssocChange) -> Option<ApplicationChannelEvent> {
        println!("Assoc change: {:?}", notification);
        match notification.state {
            SctpSacState::CommUp => {
                self.state = ApplicationChannelState::Connected;
                return Some(ApplicationChannelEvent::StateChanged { new_state: ApplicationChannelState::Connected });
            },
            _ => {}
        }
        None
    }

    fn send_open_acknowledge(&mut self, channel: &InternalDataChannel) {
        let payload = DataChannelControlMessage::OpenAck(DataChannelControlMessageOpenAck{ });
        self.send_message(channel, &RawDataChannelMessage::Control(payload), true);
    }

    fn send_open_request(&mut self, channel: &InternalDataChannel) {
        let payload = DataChannelControlMessageOpen{
            label: channel.label.clone(),
            priority: channel.priority,
            protocol: channel.protocol.clone(),
            channel_type: channel.channel_type
        };
        let control_payload = DataChannelControlMessage::Open(payload);
        self.send_message(channel, &RawDataChannelMessage::Control(control_payload), true);
    }

    fn close_datachannel_locally(&mut self, channel: &mut InternalDataChannel) {
        if !matches!(channel.state, DataChannelState::Connecting | DataChannelState::Open) { return; }
        channel.state = DataChannelState::Closing;
        let _ = channel.event_sender.send(DataChannelEvent::StateChanged(DataChannelState::Closing));

        self.pending_stream_resets.push(channel.internal_channel_id);
        self.send_outgoing_stream_resets();
    }

    fn send_message(&mut self, channel: &InternalDataChannel, message: &RawDataChannelMessage, enforce_ordered: bool) {
        let mut info = SctpSendInfo::new();
        info.snd_sid = channel.internal_channel_id;
        info.snd_ppid = message.message_type().value();
        info.snd_flags = SCTP_EOR;
        if !enforce_ordered && channel.state == DataChannelState::Open && !channel.channel_type.is_ordered() {
            info.snd_flags |= SCTP_UNORDERED;
        }

        let expected_length = message.expected_length();
        let result = {
            if expected_length < 8000 {
                let mut buffer = [0u8; 8000];
                let size = message.write(&mut buffer)
                    .expect("failed to encode open request message");
                self.sctp_session.send(&buffer[0..size], &info)
            } else {
                let mut buffer = Vec::new();
                buffer.resize(expected_length, 0);
                let size = message.write(&mut buffer)
                    .expect("failed to encode open request message");
                self.sctp_session.send(&buffer[0..size], &info)
            }
        };

        if let Err(error) = result {
            /* TODO: Improve error handling */
            println!("failed to send sctp message: {:?}", error);
        }
    }

    fn send_outgoing_stream_resets(&mut self) {
        if self.pending_stream_resets.is_empty() { return; }

        let result = self.sctp_session.reset_streams(self.pending_stream_resets.as_slice());
        if let Err(error) = result {
            println!("Reset result: {}", error);
        } else {
            self.pending_stream_resets.clear();
        }
    }
}

impl ChannelApplication {
    pub fn set_remote_description(&mut self, media: &SdpMedia) -> Result<(), String> {
        println!("Configuring application channel");

        let dtls_role_client = match media.get_attribute(SdpAttributeType::Setup) {
            Some(SdpAttribute::Setup(SdpAttributeSetup::Active)) => Ok(false),
            Some(SdpAttribute::Setup(SdpAttributeSetup::Passive)) => Ok(true),
            Some(SdpAttribute::Setup(SdpAttributeSetup::Actpass)) => {
                match ACT_PASS_DEFAULT_SETUP_TYPE {
                    SdpAttributeSetup::Active => Ok(true),
                    SdpAttributeSetup::Passive => Ok(false),
                    _ => panic!("this branch should never be reached")
                }
            },
            _ => Err(String::from("missing/invalid setup type"))
        }?;

        let mut remote_port = 5000u16; /* 5000 is the default */
        if let Some(SdpAttribute::Sctpmap(map)) = media.get_attribute(SdpAttributeType::Sctpmap) {
            remote_port = map.port;
        }

        if let Some(remote_config) = &self.remote_config {
            if self.dtls_role_client != dtls_role_client {
                return Err(String::from("client/server role miss match to previous description"));
            }

            if remote_config.port != remote_port {
                return Err(String::from("remote port miss match to previous description"));
            }
        } else {
            self.dtls_role_client = dtls_role_client;

            /* TODO: Adjust already existing data channel ids */
            self.configure_sctp().map_err(|e| format!("sctp configure failed: {}", e))?;
            self.remote_config = Some(RemoteConfig{
                port: remote_port
            });
        }

        Ok(())
    }

    pub fn generate_local_description(&self) -> Result<SdpMedia, String> {
        let mut media = SdpMedia::new(SdpMediaLine {
            media: SdpMediaValue::Application,
            formats: SdpFormatList::Integers(vec![self.sctp_session.local_port as u32]),
            port: 9,
            port_count: 0,
            proto: SdpProtocolValue::DtlsSctp
        });

        media.set_connection(SdpConnection{
            address: ExplicitlyTypedAddress::Ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            ttl: None,
            amount: None
        }).unwrap();

        media.add_attribute(SdpAttribute::Sctpmap(SdpAttributeSctpmap{
            port: self.sctp_session.local_port,
            channels: self.max_outgoing_channel as u32
        })).unwrap();
        Ok(media)
    }

    pub fn handle_transport_connected(&mut self) {
        if self.transport.is_none() {
            return;
        }

        if self.state != ApplicationChannelState::Disconnected {
            return;
        }

        self.state = ApplicationChannelState::Connecting;
        self.sctp_session.connect(self.remote_config.as_ref().expect("missing remote config").port);

        let _result = self.sctp_session.change_peer_addr_params(|params| {
            params.spp_flags &= !SPP_PMTUD_ENABLE;
            params.spp_flags |= SPP_PMTUD_DISABLE;

            /* 1280 for IPv6 and 1200 for IPv4, so 1200 fits for all */
            params.spp_pathmtu = 1200;
        });
        /* The call failes on windows anyways
        if let Err(error) = result {
            eprintln!("Failed to configure the sctp peer: {}", error);
        }
        */
    }

    pub fn handle_data(&mut self, message: Vec<u8>) {
        if self.transport.is_none() {
            eprintln!("Received DTLS data for SCTP without having a transport.");
            return;
        }
        let mut sctp_buffer = RefCell::borrow_mut(&self.stream);
        sctp_buffer.read_queue.push_back(message);
        if let Some(waker) = &self.poll_waker { waker.wake_by_ref(); }
    }
}

impl Stream for ChannelApplication {
    type Item = ApplicationChannelEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.poll_waker = None;
        RefCell::borrow_mut(&self.stream).write_waker = None;

        while let Poll::Ready(event) = self.sctp_session.poll_next_unpin(cx) {
            if let Some(event) = event {
                match event {
                    UsrSctpSessionEvent::MessageReceived { buffer, info } => {
                        match RawDataChannelMessage::parse(&buffer, info.rcv_ppid) {
                            Ok(message) => {
                                if let Some(event) = self.process_sctp_message(info.rcv_sid, message) {
                                    return Poll::Ready(Some(event));
                                }
                            },
                            Err(error) => {
                                println!("Failed to parse SCTP message: {:?}", error);
                            }
                        }
                    },
                    UsrSctpSessionEvent::EventReceived { notification } => {
                        if let Some(event) = self.process_sctp_notification(&notification) {
                            return Poll::Ready(Some(event));
                        }
                    }
                }
            } else {
                panic!("unexpected sctp event poll end");
            }
        }

        /* write all pending packets */
        while let Some(buffer) = RefCell::borrow_mut(&self.stream).write_queue.pop_front() {
            if let Some(ice_control) = &self.transport {
                let _ = ice_control.send(RTCTransportControl::SendMessage(buffer));
            } else {
                eprintln!("Tried to send SCTP packet without having a valid ice connection");
            }
        }

        while let Poll::Ready(message) = self.channel_communication_receiver.poll_next_unpin(cx) {
            match message.expect("unexpected channel close") {
                DataChannelAction::SendMessage(channel, message) => {
                    if let Some(internal_channel) = self.internal_channels.get(&channel) {
                        let internal_channel = internal_channel.clone();
                        self.send_message(RefCell::borrow(&internal_channel).deref(), &message, false);
                    } else {
                        println!("Data channel tried to send a message to an unknown channel.");
                    }
                },
                DataChannelAction::Close(channel) => {
                    if let Some(internal_channel) = self.internal_channels.get(&channel) {
                        let internal_channel = internal_channel.clone();
                        self.close_datachannel_locally(RefCell::borrow_mut(&internal_channel).deref_mut());
                    }
                }
            }

        }

        RefCell::borrow_mut(&self.stream).write_waker = Some(cx.waker().clone());
        self.poll_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

struct SctpStreamInner {
    write_queue: VecDeque<Vec<u8>>,
    write_waker: Option<Waker>,

    read_queue: VecDeque<Vec<u8>>,
    read_index: usize
}

struct SctpStream {
    inner: Rc<RefCell<SctpStreamInner>>
}

impl Read for SctpStream {
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        let mut inner = RefCell::borrow_mut(&self.inner);
        if let Some(buffer) = inner.read_queue.front() {
            assert!(buffer.len() > inner.read_index);
            let written = buf.write(&buffer[inner.read_index..])?;
            if written + inner.read_index == buffer.len() {
                inner.read_queue.pop_front();
                inner.read_index = 0;
            } else {
                inner.read_index += written;
            }

            Ok(written)
        } else {
            Err(std::io::Error::new(ErrorKind::WouldBlock, ""))
        }
    }
}

impl Write for SctpStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut inner = RefCell::borrow_mut(&self.inner);
        inner.write_queue.push_back(Vec::from(buf));
        if let Some(waker) = &inner.write_waker { waker.wake_by_ref(); }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(PartialEq, Debug)]
pub enum DataChannelMessage {
    String(Option<String>),
    Binary(Option<Vec<u8>>)
}

#[derive(PartialEq, Debug)]
pub enum DataChannelEvent {
    MessageReceived(DataChannelMessage),
    StateChanged(DataChannelState)
}

/// The state of a data channel.
#[derive(PartialOrd, PartialEq, Debug, Copy, Clone)]
pub enum DataChannelState {
    /// The data channel isn't yet established.
    /// Messages cannot be send or received.
    Connecting,

    /// The data channel has been established.
    /// Messages can be send and received.
    Open,

    /// The data channel is closing.
    /// Messages may be received but cannot be send.
    Closing,

    /// The data channel has been closed.
    /// Messages cannot be send or received.
    Closed
}

#[derive(PartialEq, Debug)]
enum DataChannelAction {
    SendMessage(u16, RawDataChannelMessage),
    Close(u16)
}

pub struct DataChannel {
    channel_id: u32,
    internal_channel_id: u16,

    label: String,
    protocol: String,
    state: DataChannelState,
    channel_type: DataChannelType,

    events: mpsc::UnboundedReceiver<DataChannelEvent>,
    channel_sender: mpsc::UnboundedSender<DataChannelAction>
}

impl DataChannel {
    /// A unique id for this channel, which will be used for all
    /// Application channel events
    pub fn unique_id(&self) -> u32 { self.channel_id }

    /// The name of the Data Channel as a UTF-8 encoded string.
    /// This may be an empty string.
    pub fn label(&self) -> &String { &self.label }

    /// Returns the protocol of the data channel.
    /// If this is an empty string the protocol is unspecified.
    /// If it is a non-empty string, it specifies a protocol registered in the
    /// 'WebSocket Subprotocol Name Registry' created in [RFC6455](https://tools.ietf.org/html/rfc6455).
    pub fn protocol(&self) -> &String { &self.protocol }

    /// Returns the current data channel state
    pub fn state(&self) -> &DataChannelState { &self.state }

    /// Get the underlying data channel transport type
    pub fn channel_type(&self) -> &DataChannelType { &self.channel_type }

    /// Send a string message to the remote peer.
    /// The text must be UTF-8 encoded.
    pub fn send_text_message(&mut self, message: Option<String>) -> Result<(), String> {
        if self.state != DataChannelState::Open {
            return Err(String::from("channel isn't open"));
        }

        if let Some(message) = message {
            self.channel_sender.send(DataChannelAction::SendMessage(self.internal_channel_id, RawDataChannelMessage::String(message)))
                .expect("unexpected channel close");
        } else {
            self.channel_sender.send(DataChannelAction::SendMessage(self.internal_channel_id, RawDataChannelMessage::StringEmpty()))
                .expect("unexpected channel close");
        }
        Ok(())
    }

    /// Send a binary message to the remote peer.
    pub fn send_binary_message(&mut self, message: Option<Vec<u8>>) -> Result<(), String> {
        if self.state != DataChannelState::Open {
            return Err(String::from("channel isn't open"));
        }

        if let Some(message) = message {
            self.channel_sender.send(DataChannelAction::SendMessage(self.internal_channel_id, RawDataChannelMessage::Binary(message)))
                .expect("unexpected channel close");
        } else {
            self.channel_sender.send(DataChannelAction::SendMessage(self.internal_channel_id, RawDataChannelMessage::BinaryEmpty()))
                .expect("unexpected channel close");
        }
        Ok(())
    }

    pub fn close(&mut self) {
        if matches!(self.state, DataChannelState::Closed | DataChannelState::Closing) {
            return;
        }

        let _ = self.channel_sender.send(DataChannelAction::Close(self.internal_channel_id));
    }
}

impl Stream for DataChannel {
    type Item = DataChannelEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.poll_next_unpin(cx)
    }
}

impl Drop for DataChannel {
    fn drop(&mut self) {
        if matches!(self.state, DataChannelState::Connecting | DataChannelState::Open) {
            let _ = self.channel_sender.send(DataChannelAction::Close(self.internal_channel_id));
        }
    }
}