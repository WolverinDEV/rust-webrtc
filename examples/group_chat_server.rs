#![feature(once_cell)]
#![feature(drain_filter)]
#![feature(try_trait)]

use futures::{StreamExt};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

extern crate webrtc_lib;
use webrtc_sdp::{parse_sdp, SdpBandwidth};
use webrtc_sdp::attribute_type::{SdpAttribute, SdpAttributeRtcpFbType, SdpAttributeFmtpParameters};
use std::sync::{Arc, Mutex};
use std::ops::{DerefMut};
use std::str::FromStr;
use futures::task::{Poll, Waker};
use tokio::sync::mpsc;
use crate::shared::gio::MAIN_GIO_EVENT_LOOP;
use crate::shared::ws::{WebCommand, Client, ClientEvents};
use webrtc_sdp::media_type::SdpMediaValue;
use std::cell::RefCell;
use futures::future::{Abortable, AbortHandle};
use crate::shared::execute_example;
use std::collections::{LinkedList, HashMap};
use lazy_static::lazy_static;
use std::rc::Rc;
use std::borrow::BorrowMut;
#[allow(unused_imports)]
use std::mem::forget;
use webrtc_lib::media::{Codec, MediaReceiver, MediaSender, MediaSenderEvent, MediaReceiverEvent, CodecFeedback};
use webrtc_lib::rtc::{PeerConnection, PeerConnectionEvent, RtcDescriptionType};
use webrtc_lib::utils::rtcp::packets::{RtcpPayloadFeedback, RtcpPacketPayloadFeedback};
use webrtc_lib::utils::rtcp::RtcpPacket;
use webrtc_lib::{initialize_webrtc, rtc};
use slog::{ o, Drain };
use std::io::{ Cursor };
use byteorder::{WriteBytesExt, BigEndian};

mod shared;
mod video;

struct Server {
    clients: HashMap<u32, Arc<Mutex<Client<ClientData>>>>,
    client_id_index: u32,
}

lazy_static! {
    static ref SERVER: Mutex<Server> = Mutex::new(Server {
        clients: HashMap::new(),
        client_id_index: 0
    });
}

struct ClientData {
    client_id: u32,

    peer: rtc::PeerConnection,
    peer_abort: Option<futures::future::AbortHandle>,

    media_broadcast_abort: Option<futures::future::AbortHandle>,
    broadcast_waker: Option<Waker>,
    enforce_pli: bool,

    video_receiver: Option<Rc<RefCell<MediaReceiver>>>,
    audio_receiver: Option<Rc<RefCell<MediaReceiver>>>,

    video_senders: LinkedList<Option<MediaSender>>,
    audio_senders: LinkedList<Option<MediaSender>>
}

/* FIXME: This is not true, but it's required for lazy_static!, even though the whole client, including the client data has been wrapped in a Mutex */
unsafe impl Sync for ClientData {}

/* FIXME: This is not true, but it's required for lazy_static!, even though the whole client, including the client data has been wrapped in a Mutex */
unsafe impl Send for ClientData {}

impl Default for ClientData {
    fn default() -> Self {
        let event_loop = MAIN_GIO_EVENT_LOOP.lock().unwrap().event_loop();
        let event_context = event_loop.get_context();

        let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
        let drain = slog_term::FullFormat::new(decorator).build().fuse();
        /* FIXME: Generate a client id */
        let logger = slog::Logger::root(drain, o!("client-id" => 0));

        let peer = PeerConnection::builder()
            .logger(logger)
            .event_loop(event_context)
            .dispatch_all_packets(true)
            //.ice_udp(false)
            .create().expect("failed to create peer connection");

        ClientData {
            client_id: 0,
            peer,
            peer_abort: None,

            media_broadcast_abort: None,
            broadcast_waker: None,
            enforce_pli: false,

            video_receiver: None,
            audio_receiver: None,

            video_senders: LinkedList::new(),
            audio_senders: LinkedList::new()
        }
    }
}

impl Drop for ClientData {
    fn drop(&mut self) {
        println!("Client deleted");
    }
}

async fn execute_client(client: Client<ClientData>){
    let client = Arc::new(Mutex::new(client));

    {
        let mut server = SERVER.lock().unwrap();
        let mut locked_client = client.lock().unwrap();
        locked_client.data.client_id = server.client_id_index;
        server.client_id_index =  server.client_id_index.wrapping_add(1);
        server.clients.insert(locked_client.data.client_id, client.clone());
    }

    /* required from the beginning in order to pull the senders streams */
    broadcast_client_media(client.clone(), client.lock().unwrap().deref_mut());

    let client_clone = client.clone();
    futures::future::poll_fn(move |cx| {
        /* process client events */
        while let Poll::Ready(message) = { let mut locked_client = client.lock().unwrap(); locked_client.poll_next_unpin(cx) } {
            if let Some(message) = message {
                match message {
                    ClientEvents::Connected => {
                        println!("Remote client connected");
                    },
                    ClientEvents::Disconnected => {
                        println!("Remote client disconnected (event)");
                    },
                    ClientEvents::CommandReceived(command) => {
                        if let Err(error) = handle_command(client.clone(), &command) {
                            println!("Failed to handle a command: {:?}", error);
                            client.lock().unwrap().close(Some(CloseFrame{ code: CloseCode::Invalid, reason: "command handling failed".into() }));
                        }
                    }
                }
            } else {
                println!("client connection gone");
                return Poll::Ready(());
            }
        }

        Poll::Pending
    }).await;

    {
        let client_id = { client_clone.lock().unwrap().data.client_id };
        let mut server = SERVER.lock().unwrap();
        server.clients.remove(&client_id);
    }

    let client = client_clone.lock().unwrap();
    if let Some(abort) = client.data.peer_abort.as_ref() {
        abort.abort();
    }

    if let Some(abort) = client.data.media_broadcast_abort.as_ref() {
        abort.abort();
    }
}

fn broadcast_client_media(client: Arc<Mutex<Client<ClientData>>>, locked_client: &mut Client<ClientData>) {
    let (abort_handle, abort_registration) = AbortHandle::new_pair();
    locked_client.data.media_broadcast_abort = Some(abort_handle);

    tokio::spawn(Abortable::new(futures::future::poll_fn(move |cx| {
        let mut locked_client = client.lock().unwrap();
        locked_client.data.broadcast_waker = Some(cx.waker().clone());

        for opt_sender in locked_client.data.audio_senders.iter_mut() {
            if opt_sender.is_none() { continue; }

            while let Poll::Ready(event) = opt_sender.as_mut().unwrap().poll_next_unpin(cx) {
                if event.is_none() {
                    println!("Unregistering audio sender from client");
                    *opt_sender = None;
                    break;
                }

                println!("Audio sender received event {:?}", event.unwrap());
            }
        }
        locked_client.data.audio_senders.drain_filter(|e| e.is_none());


        let mut request_pli = locked_client.data.enforce_pli;
        for opt_sender in locked_client.data.video_senders.iter_mut() {
            if opt_sender.is_none() { continue; }

            while let Poll::Ready(event) = opt_sender.as_mut().unwrap().poll_next_unpin(cx) {
                if event.is_none() {
                    println!("Unregistering video sender from client");
                    *opt_sender = None;
                    break;
                }

                match event.as_ref().unwrap() {
                    MediaSenderEvent::PayloadFeedbackReceived(fb) => {
                        if *fb == RtcpPayloadFeedback::PictureLossIndication {
                            println!("Video sender channel got pli");
                            request_pli = true;
                        } else {
                            println!("Video sender channel PayloadFeedbackReceived: {:?}", fb);
                        }
                    },
                    MediaSenderEvent::TransportFeedbackReceived(feedback) => {
                        /* will already be handled */
                    },
                    MediaSenderEvent::ReceiverReportReceived(rr) => {
                        /* will already be handled */
                    },
                    _ => {
                        println!("Video sender channel event: {:?}", event);
                    }
                }
            }
        }
        locked_client.data.video_senders.drain_filter(|e| e.is_none());

        if let Some(receiver) = locked_client.data.audio_receiver.as_ref().map(|e| e.clone()) {
            let mut receiver = RefCell::borrow_mut(&receiver);
            while let Poll::Ready(event) = receiver.poll_next_unpin(cx) {
                if event.is_none() {
                    println!("Audio stream ended");
                    locked_client.data.audio_receiver = None;
                    break;
                }

                match event.unwrap() {
                    MediaReceiverEvent::DataReceived(data) => {
                        for sender in locked_client.data.audio_senders.iter_mut() {
                            if let Some(sender) = sender.as_mut() {
                                *sender.payload_type_mut() = data.payload_type(); /* TODO: Don't do this. Since we're only accepting VP8/opus it should have already been set */
                                sender.send_seq(data.payload(), data.sequence_number().into(), data.mark(), data.timestamp(), None);
                            }
                        }
                    },
                    MediaReceiverEvent::DataLost(ids) => {
                        println!("Audio data lost: {:?}", ids);
                    },
                    MediaReceiverEvent::ByeSignalReceived(reason) => {
                        println!("Received bye signal for audio stream with reason {:?}", reason);
                    },
                    MediaReceiverEvent::ReceiverActivated => {},
                    MediaReceiverEvent::BandwidthLimitViolation(_) => {}
                }
            }
        }

        if let Some(receiver) = locked_client.data.video_receiver.as_ref().map(|e| e.clone()) {
            let mut receiver = RefCell::borrow_mut(&receiver);
            if request_pli {
                receiver.reset_pending_resends();
                let id = receiver.ssrc();
                let _ = receiver.send_control(&RtcpPacket::PayloadFeedback(RtcpPacketPayloadFeedback{
                    ssrc: 1,
                    media_ssrc: id,
                    feedback: RtcpPayloadFeedback::PictureLossIndication
                }));
            }
            while let Poll::Ready(event) = receiver.poll_next_unpin(cx) {
                if event.is_none() {
                    println!("Video stream ended");
                    locked_client.data.video_receiver = None;
                    break;
                }

                match event.unwrap() {
                    MediaReceiverEvent::DataReceived(data) => {
                        for sender in locked_client.data.video_senders.iter_mut() {
                            if let Some(sender) = sender.as_mut() {
                                *sender.payload_type_mut() = data.payload_type(); /* TODO: Don't do this. Since we're only accepting VP8/opus it should have already been set */
                                sender.send_seq(data.payload(), data.sequence_number().into(), data.mark(), data.timestamp(), None);
                            }
                        }
                    },
                    MediaReceiverEvent::DataLost(ids) => {
                        println!("Video data lost: {:?}", ids);
                    },
                    MediaReceiverEvent::ByeSignalReceived(reason) => {
                        println!("Received bye signal for video stream with reason {:?}", reason);
                    },
                    MediaReceiverEvent::ReceiverActivated => {},
                    MediaReceiverEvent::BandwidthLimitViolation(_) => {}
                }
            }
        }

        /* required else Rust can not defer the return type */
        if false {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }), abort_registration));
}

fn main() {
    let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let logger = slog::Logger::root(drain, o!("global-logger" => 1));
    initialize_webrtc(logger);

    execute_example(execute_client);
}

fn execute_client_peer(client: Arc<Mutex<Client<ClientData>>>, locked_client: &mut Client<ClientData>) {
    println!("Creating a new peer");

    let mut command_pipe = locked_client.command_sender.clone();

    let (abort_handle, abort_registration) = AbortHandle::new_pair();
    locked_client.data.peer_abort = Some(abort_handle);

    tokio::spawn(Abortable::new(async move {
        /* This locks all other access */
        loop {
            let event = futures::future::poll_fn(|cx| {
                let mut locked_client = client.lock().unwrap();

                PeerConnection::poll_next_unpin(&mut locked_client.data.peer, cx)
            }).await;

            if event.is_none() {
                break;
            }

            match event.unwrap() {
                PeerConnectionEvent::LocalIceCandidate(candidate, media_id) => {
                    if let Some(candidate) = candidate {
                        let _ = command_pipe.send(WebCommand::RtcAddIceCandidate {
                            candidate: String::from("candidate:") + &candidate.to_string(),
                            media_index: media_id
                        });
                    } else {
                        let _ = command_pipe.send(WebCommand::RtcAddIceCandidate {
                            candidate: String::from(""),
                            media_index: media_id
                        });
                    }
                },
                PeerConnectionEvent::ReceivedRemoteStream(mut receiver) => {
                    let mut locked_client = client.lock().unwrap();
                    let peer = &mut locked_client.data.peer;
                    let media_line = peer.media_lines().iter()
                        .find(|line| RefCell::borrow(line).unique_id() == receiver.media_line())
                        .map(|e| e.clone()).unwrap();
                    let media_line = RefCell::borrow(&media_line);

                    println!("Received remote {} stream {}", media_line.media_type, receiver.ssrc());
                    let mut wake_broadcast = false;
                    match &media_line.media_type {
                        SdpMediaValue::Video => {
                            if locked_client.data.video_receiver.is_none() {
                                let resend_requester = receiver.resend_requester_mut();
                                resend_requester.set_resend_interval(50);
                                resend_requester.set_nack_delay(10);
                                resend_requester.set_frame_size(1024);
                                receiver.set_bandwidth_limit(Some(4_000_000));

                                locked_client.data.video_receiver = Some(Rc::new(RefCell::new(receiver)));
                                wake_broadcast = true;
                            }
                        },
                        SdpMediaValue::Audio => {
                            if locked_client.data.audio_receiver.is_none() {
                                locked_client.data.audio_receiver = Some(Rc::new(RefCell::new(receiver)));
                                wake_broadcast = true;
                            }
                        },
                        _ => {}
                    }

                    if wake_broadcast {
                        if let Some(waker) = &locked_client.data.broadcast_waker {
                            waker.wake_by_ref();
                        }
                    }
                },
                PeerConnectionEvent::ReceivedDataChannel(channel) => {
                    println!("Received remote data channel: {}", channel.label());
                },
                PeerConnectionEvent::UnassignableRtpPacket(_packet) => { },
                PeerConnectionEvent::UnassignableRtcpPacket(_packet) => {
                    //eprintln!("Unassignable RTCP packet: {:?}", packet);
                },
                PeerConnectionEvent::NegotiationNeeded => {
                    eprintln!("Negotiation needed");

                    let mut locked_client = client.lock().unwrap();
                    if let Err(err) = send_local_description(&mut command_pipe, &mut locked_client.data.peer, String::from("offer")) {
                        eprintln!("Failed to send local description: {}", err);
                    }
                },
                _ => {}
            }
        }

        println!("Peer poll exited");
    }, abort_registration));
}

const DEFAULT_FMTP_PARAMETERS: SdpAttributeFmtpParameters = SdpAttributeFmtpParameters{
    packetization_mode: 0,
    level_asymmetry_allowed: false,
    profile_level_id: 0x0042_0010,
    max_fs: 0,
    max_cpb: 0,
    max_dpb: 0,
    max_br: 0,
    max_mbps: 0,
    usedtx: false,
    stereo: false,
    useinbandfec: false,
    cbr: false,
    max_fr: 0,
    maxplaybackrate: 48000,
    maxaveragebitrate: 0,
    ptime: 0,
    minptime: 0,
    maxptime: 0,
    encodings: Vec::new(),
    dtmf_tones: String::new(),
    rtx: None,
    unknown_tokens: Vec::new(),
};

fn send_local_description(command_pipe: &mut mpsc::UnboundedSender<WebCommand>, peer: &mut PeerConnection, mode: String) -> std::result::Result<(), String> {
    for line in peer.media_lines() {
        let mut line = RefCell::borrow_mut(line);
        if line.local_codecs().is_empty() {
            match line.media_type {
                SdpMediaValue::Application => {},
                SdpMediaValue::Video => {
                    line.register_local_codec(Codec{
                        payload_type: 105,
                        frequency: 90_000,
                        codec_name: String::from("H264"),
                        feedback: vec![
                            CodecFeedback{ feedback_type: SdpAttributeRtcpFbType::Nack, parameter: String::new(), extra: String::new() },
                            CodecFeedback{ feedback_type: SdpAttributeRtcpFbType::Nack, parameter: String::from("pli"), extra: String::new() },
                            CodecFeedback{ feedback_type: SdpAttributeRtcpFbType::Remb, parameter: String::new(), extra: String::new() },
                        ],
                        channels: None,
                        parameters: None
                    }).expect("failed to register local codec");
                },
                SdpMediaValue::Audio => {
                    line.register_local_codec(Codec{
                        payload_type: 111,
                        frequency: 48_000,
                        codec_name: String::from("opus"),
                        feedback: vec![ ],
                        channels: Some(2),
                        parameters: None
                    }).expect("failed to register local codec");
                }
            }
        }
    }

    let mut answer = peer.create_local_description().map_err(|err| format!("{:?}", err))?;
    for line in answer.media.iter_mut() {
        if line.get_type() == &SdpMediaValue::Video {
            line.add_bandwidth(SdpBandwidth::Tias(50_000_000));
        }
    }

    println!("Sending {}: {:?}", mode, answer.to_string());
    let _ = command_pipe.send(WebCommand::RtcSetRemoteDescription { sdp: answer.to_string(), mode });
    Ok(())
}

fn handle_command(client: Arc<Mutex<Client<ClientData>>>, command: &WebCommand) -> std::result::Result<(), String> {
    let mut locked_client = client.lock().unwrap();

    println!("Received web command: {:?}", command);
    match command {
        WebCommand::RtcSetRemoteDescription{ mode, sdp } => {
            let sdp = parse_sdp(sdp.as_str(), false)
                .map_err(|err| format!("failed to parse sdp: {:?}", err))?;
            println!("Offer/Answer contains {} media streams", sdp.media.len());

            if locked_client.data.peer_abort.is_none() {
                execute_client_peer(client.clone(), locked_client.deref_mut());
            }

            let mode = {
                if mode == "offer" {
                    RtcDescriptionType::Offer
                } else {
                    RtcDescriptionType::Answer
                }
            };

            /*
            /* Testing media sender adding before the peer has been initialized */
            if mode == RtcDescriptionType::Offer {
                let mut stream = locked_client.data.peer.create_media_sender(SdpMediaValue::Video);
                stream.register_property(String::from("msid"), Some(String::from("PreICETest -")));
                //forget(stream);
                tokio::spawn(tokio::future::poll_fn(move |cx| {
                    while let Poll::Ready(event) = stream.poll_next_unpin(cx) {
                        if event.is_none() {
                            return Poll::Ready(());
                        }

                        println!("XXXXXXXXXX - Event: {:?}", event);
                    }

                    Poll::Pending
                }));
            }
            */

            locked_client.data.peer.set_remote_description(&sdp, &mode).map_err(|err| format!("{:?}", err))?;

            if mode == RtcDescriptionType::Offer {
                /*
                let mut video_sender = client.data.video_sender.lock().unwrap();
                if video_sender.is_none() && peer.media_lines().iter().find(|e| RefCell::borrow(e).media_type != SdpMediaValue::Application).is_some() {
                    let mut channel = peer.create_media_sender(SdpMediaValue::Video).unwrap();
                    channel.register_property(String::from("msid"), Some(String::from(format!("{} -", "VideoReplayChannel"))));
                    println!("Props: {:?}", channel.properties().deref());
                }
                */

                let locked_client = locked_client.deref_mut();
                send_local_description(locked_client.command_sender.borrow_mut(), locked_client.data.peer.borrow_mut(), String::from("answer"))?;
            }

            if locked_client.data.audio_senders.is_empty() && locked_client.data.video_senders.is_empty() {
                {
                    let locked_client = locked_client.deref_mut();
                    //locked_client.data.audio_senders.push_back(Some(locked_client.data.peer.create_media_sender(SdpMediaValue::Audio)));
                    locked_client.data.video_senders.push_back(Some(locked_client.data.peer.create_media_sender(SdpMediaValue::Video)));
                }

                SERVER.lock().unwrap().clients.iter().for_each(|(target_client_id, target)| {
                    if *target_client_id == locked_client.data.client_id {
                        return;
                    }

                    let mut target = target.lock().unwrap();
                    target.data.audio_senders.push_back(Some(locked_client.data.peer.create_media_sender(SdpMediaValue::Audio)));
                    target.data.video_senders.push_back(Some(locked_client.data.peer.create_media_sender(SdpMediaValue::Video)));
                    target.data.enforce_pli = true;

                    locked_client.data.audio_senders.push_back(Some(target.data.peer.create_media_sender(SdpMediaValue::Audio)));
                    locked_client.data.video_senders.push_back(Some(target.data.peer.create_media_sender(SdpMediaValue::Video)));
                    locked_client.data.enforce_pli = true;

                    println!("Connected {} with {}", locked_client.data.client_id, target.data.client_id);
                });

                if let Some(waker) = &locked_client.data.broadcast_waker {
                    waker.wake_by_ref();
                }
            }
        },
        WebCommand::RtcAddIceCandidate { candidate, media_index, .. } => {
            let candidate = SdpAttribute::from_str(candidate.as_str()).map_err(|err| format!("failed to parse candidate: {:?}", err))?;
            let candidate = { if let SdpAttribute::Candidate(c) = candidate { Ok(c) } else { Err(String::from("internal candidate cast error")) } }?;

            let _ = locked_client.data.peer.add_remote_ice_candidate(*media_index, Some(&candidate)).map_err(|err| format!("{:?}", err))
                .map_err(|err| println!("Failed to add remote ice candidate: {:?}", err));

        },
        WebCommand::RtcFinishedIceCandidates {} => {
            let sdp_lines = locked_client.data.peer.media_lines().iter()
                .map(|e| RefCell::borrow(e).sdp_index().clone())
                .filter_map(|e| e)
                .collect::<Vec<_>>();

            for line in sdp_lines {
                if let Err(err) = locked_client.data.peer.add_remote_ice_candidate(line, None) {
                    eprintln!("Failed to signal ICE finished: {:?}", err);
                }
            }
        },
        WebCommand::RtcChangeVideoBandwidth { bitrate } => {
            if let Some(receiver) = locked_client.data.video_receiver.as_ref() {
                let mut receiver = RefCell::borrow_mut(receiver);

                receiver.set_bandwidth_limit(Some(*bitrate));
                let stats = receiver.statistics();
                //println!("Stats: {:?}", stats);
                println!("Payload: Second: {}, Minute: {}; Header: Second: {}, Minute: {}", stats.bandwidth_payload_second(), stats.bandwidth_payload_minute(), stats.bandwidth_header_second(), stats.bandwidth_header_minute());
            }
        }
    }

    Ok(())
}
