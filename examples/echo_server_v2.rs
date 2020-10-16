#![feature(once_cell)]
#![feature(drain_filter)]
#![feature(try_trait)]

use futures::{StreamExt};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use webrtc_sdp::parse_sdp;
use webrtc_sdp::attribute_type::{SdpAttribute};
use std::sync::{Arc, Mutex};
use std::ops::DerefMut;
use std::str::FromStr;
use futures::task::{Poll};
use tokio::sync::mpsc;
use web_test::{rtc, initialize_webrtc};
use web_test::rtc::{PeerConnection, PeerConnectionEvent, RtcDescriptionType};
use web_test::media::{MediaSender, MediaReceiverEvent};
use crate::shared::gio::MAIN_GIO_EVENT_LOOP;
use crate::shared::ws::{WebCommand, Client, ClientEvents};
use rtp_rs::Seq;
use webrtc_sdp::media_type::SdpMediaValue;
use std::cell::RefCell;
use web_test::application::{DataChannelEvent, DataChannelMessage};
use web_test::sctp::message::DataChannelType;

mod shared;
mod video;

struct ClientData {
    event_loop: Option<glib::MainLoop>,
    peer: Option<Arc<Mutex<rtc::PeerConnection>>>,
    command_pipe: (mpsc::UnboundedSender<WebCommand>, mpsc::UnboundedReceiver<WebCommand>),

    video_sender: Arc<Mutex<Option<MediaSender>>>
}

impl Default for ClientData {
    fn default() -> Self {
        ClientData {
            event_loop: Some(MAIN_GIO_EVENT_LOOP.lock().unwrap().event_loop()),
            peer: None,
            command_pipe: mpsc::unbounded_channel(),
            video_sender: Arc::new(Mutex::new(None))
        }
    }
}

fn main() {
    initialize_webrtc();

    let mut runtime = tokio::runtime::Builder::new()
        .threaded_scheduler()
        .enable_all()
        .core_threads(1)
        .max_threads(1)
        .build().unwrap();

    runtime.block_on(async move {
        let mut server = shared::ws::Server::<ClientData>::new(String::from("127.0.0.1:1234"));
        loop {
            let (client, server_) = server.into_future().await;
            server = server_;
            if client.is_none() {
                /* server has been closed */
                break;
            }

            let client = client.unwrap();
            println!("Received new client: {:?}", &client.address);
            execute_client(client);
        }
    });
}

static mut PACKET_ID: u16 = 1000;
fn spawn_client_peer(client: &mut Client<ClientData>) {
    assert!(client.data.peer.is_none());

    println!("Creating a new peer");
    let peer = Arc::new(Mutex::new(rtc::PeerConnection::new(client.data.event_loop.clone().unwrap().get_context())));
    client.data.peer = Some(peer.clone());

    let tx = client.data.command_pipe.0.clone();
    let video_stream = client.data.video_sender.clone();
    tokio::spawn(async move {
        /* This locks all other access */
        loop {
            let event = futures::future::poll_fn(|cx| {
                let mut peer = peer.lock().unwrap();
                let peer = peer.deref_mut();

                PeerConnection::poll_next_unpin(peer, cx)
            }).await;

            if event.is_none() {
                break;
            }

            match event.unwrap() {
                PeerConnectionEvent::LocalIceCandidate(candidate, media_id) => {
                    if let Some(candidate) = candidate {
                        let _ = tx.send(WebCommand::RtcAddIceCandidate {
                            candidate: String::from("candidate:") + &candidate.to_string(),
                            media_index: media_id
                        });
                    } else {
                        let _ = tx.send(WebCommand::RtcAddIceCandidate {
                            candidate: String::from(""),
                            media_index: media_id
                        });
                    }
                },
                PeerConnectionEvent::ReceivedRemoteStream(receiver) => {
                    println!("Received remote stream {}", receiver.id);

                    let video_stream = video_stream.clone();
                    tokio::spawn(async move {
                        let mut receiver = receiver;
                        loop {
                            let (message, receiver_) = receiver.into_future().await;
                            receiver = receiver_;

                            if message.is_none() {
                                println!("Remote stream {} closed", receiver.id);
                                break;
                            }

                            let message = message.unwrap();
                            match message {
                                MediaReceiverEvent::DataReceived(packet) => {
                                    println!("Remote stream {} received RTP data {}", receiver.id, packet.payload().len());
                                    if let Some(stream) = video_stream.lock().unwrap().deref_mut() {
                                        let builder = packet.create_builder()
                                            .ssrc(stream.id)
                                            .sequence(Seq::from(unsafe { PACKET_ID = PACKET_ID.wrapping_add(1); PACKET_ID }));

                                        stream.send_data(builder.build().unwrap());
                                    }
                                },
                                _ => {
                                    println!("Remote stream {} received: {:?}", receiver.id, message);
                                }
                            }
                        }
                    });
                },
                PeerConnectionEvent::ReceivedDataChannel(channel) => {
                    println!("Received remote data channel: {}", channel.label());

                    let channel = peer.lock().unwrap().create_data_channel(DataChannelType::Reliable, String::from(format!("reply - {}", channel.label())), None, 0).unwrap();

                    tokio::spawn(async move {
                        let mut channel = channel;
                        loop {
                            let (message, channel_) = channel.into_future().await;
                            channel = channel_;

                            if message.is_none() {
                                println!("Data channel {} closed", channel.label());
                                break;
                            }

                            match message.unwrap() {
                                DataChannelEvent::MessageReceived(message) => {
                                    println!("Received dc message on {}: {:?}", channel.label(), message);
                                    if let DataChannelMessage::String(Some(text)) = message {
                                        channel.send_text_message(Some(text));
                                    }
                                },
                                DataChannelEvent::StateChanged(state) => {
                                    println!("Data channel state for {} changed to {:?}", channel.label(), state);
                                }
                            }
                        }
                    });
                },
                PeerConnectionEvent::UnassignableRtpPacket(_packet) => { },
                PeerConnectionEvent::UnassignableRtcpPacket(_packet) => { }
            }
        }

        println!("Peer poll exited");
    });
}

fn handle_command(client: &mut Client<ClientData>, command: &WebCommand) -> std::result::Result<(), String> {
    println!("Received web command: {:?}", command);
    match command {
        WebCommand::RtcSetRemoteDescription{ mode, sdp } => {
            if mode != "offer" {
                return Err(String::from("We only support rtp offers"));
            }

            let sdp = parse_sdp(sdp.as_str(), false)
                .map_err(|err| format!("failed to parse sdp: {:?}", err))?;
            println!("Offer contains {} media streams", sdp.media.len());

            if client.data.peer.is_none() {
                spawn_client_peer(client);
            }

            let peer = client.data.peer.as_ref().expect("expected a peer");
            let mut locked_peer = peer.lock().unwrap();
            locked_peer.set_remote_description(&sdp, RtcDescriptionType::Offer).map_err(|err| format!("{:?}", err))?;

            for line in locked_peer.media_lines() {
                let mut line = RefCell::borrow_mut(line);
                if line.local_codecs().is_empty() && line.media_type != SdpMediaValue::Application {
                    let codec = line.remote_codecs().first().unwrap().clone();
                    line.register_local_codec(codec);
                }
            }

            let mut video_sender = client.data.video_sender.lock().unwrap();
            if video_sender.is_none() && locked_peer.media_lines().iter().find(|e| RefCell::borrow(e).media_type != SdpMediaValue::Application).is_some() {
                *video_sender = Some(locked_peer.add_media_sender(SdpMediaValue::Video).unwrap());
            }

            let answer = locked_peer.create_local_description().map_err(|err| format!("{:?}", err))?;
            println!("Answer: {:?}", answer.to_string());
            let _ = client.data.command_pipe.0.send(WebCommand::RtcSetRemoteDescription { sdp: answer.to_string(), mode: String::from("answer") });
        },
        WebCommand::RtcAddIceCandidate { candidate, media_index, .. } => {
            let peer = client.data.peer.as_ref().ok_or(String::from("no peer initialized"))?;

            let candidate = SdpAttribute::from_str(candidate.as_str()).map_err(|err| format!("failed to parse candidate: {:?}", err))?;
            let candidate = { if let SdpAttribute::Candidate(c) = candidate { Ok(c) } else { Err(String::from("internal candidate cast error")) } }?;

            let mut peer = peer.lock().unwrap();
            let _ = peer.add_remote_ice_candidate(*media_index, Some(&candidate)).map_err(|err| format!("{:?}", err))
                .map_err(|err| println!("Failed to add remote ice candidate: {:?}", err));

        },
        WebCommand::RtcFinishedIceCandidates {} => {
            let mut peer = client.data.peer.as_ref().ok_or(String::from("no peer initialized"))?
                .lock().unwrap();

            for line in peer.media_lines().iter().map(|e| RefCell::borrow_mut(e).index).collect::<Vec<u32>>() {
                peer.add_remote_ice_candidate(line, None);
            }
        }
    }

    Ok(())
}

fn execute_client(mut client: Client<ClientData>) {
    tokio::spawn(futures::future::poll_fn(move |cx| {
        while let Poll::Ready(message) = client.poll_next_unpin(cx) {
            if let Some(message) = message {
                match message {
                    ClientEvents::Connected => {
                        println!("Remote client connected");
                    },
                    ClientEvents::Disconnected => {
                        println!("Remote client disconnected (event)");
                    },
                    ClientEvents::CommandReceived(command) => {
                        if let Err(error) = handle_command(&mut client, &command) {
                            println!("Failed to handle a command: {:?}", error);
                            client.close(Some(CloseFrame{ code: CloseCode::Invalid, reason: "command handling failed".into() }));
                        }
                    }
                }
            } else {
                println!("client connection gone");
                return Poll::Ready(());
            }
        }

        while let Poll::Ready(message) = client.data.command_pipe.1.poll_next_unpin(cx) {
            let message = message.expect("unexpected channel close");
            client.send_message(&message);
        }

        Poll::Pending
    }));
}