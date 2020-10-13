#![feature(once_cell)]
#![feature(drain_filter)]
#![feature(try_trait)]

use std::net::{SocketAddr};
use futures::{StreamExt, SinkExt, FutureExt};
use futures::future::{Either};
use tokio_tungstenite::tungstenite::{Message};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use serde::{Deserialize, Serialize};
use webrtc_sdp::parse_sdp;
use webrtc_sdp::attribute_type::{SdpAttribute, SdpAttributeCandidateTransport, SdpAttributeExtmap};
use std::sync::{Arc, Mutex};
use std::ops::DerefMut;
use std::str::FromStr;
use futures::task::{Poll, Context};
use tokio::sync::mpsc;
use web_test::{rtc, initialize_webrtc};
use web_test::srtp2::srtp2_global_init;
use web_test::media::application::{MediaChannelApplication, MediaChannelApplicationEvent};
use web_test::sctp::message::DataChannelType;
use web_test::media::audio::MediaChannelAudio;
use web_test::media::video::{MediaChannelVideo, MediaChannelVideoEvents};
use web_test::rtc::{PeerConnection, PeerConnectionEvent, RtcDescriptionType, MediaIdFragment};
use web_test::media::{TypedMediaChannel, MediaChannel};
use tokio_tungstenite::tungstenite::protocol::Role::Server;
use std::cell::Cell;
use crate::shared::gio::MAIN_GIO_EVENT_LOOP;
use crate::shared::ws::{WebCommand, Client, ClientEvents};

mod shared;

struct ClientData {
    event_loop: Option<glib::MainLoop>,
    peer: Option<Arc<Mutex<rtc::PeerConnection>>>,
    command_pipe: (mpsc::UnboundedSender<WebCommand>, mpsc::UnboundedReceiver<WebCommand>)
}

impl Default for ClientData {
    fn default() -> Self {
        ClientData {
            event_loop: Some(MAIN_GIO_EVENT_LOOP.lock().unwrap().event_loop()),
            peer: None,
            command_pipe: mpsc::unbounded_channel()
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

fn poll_application_channel(channel: &mut MediaChannelApplication, cx: &mut Context) {
    while let Poll::Ready(event) = channel.poll_next_unpin(cx) {
        if !event.is_some() {
            /* TODO: Never poll that media channel again */
            break;
        }

        match event.unwrap() {
            MediaChannelApplicationEvent::DataChannelReceived { label, channel_id, channel_type } => {
                println!("Received channel {:?} ({:?})", label, channel_type);
                let mut data_channel = channel.take_data_channel(channel_id).expect("missing just created channel");
                data_channel.send_text_message(Some(String::from("Hey newly created channel"))).unwrap();
                //data_channel.close();
                tokio::spawn(async move {
                    while let Some(event) = data_channel.next().await {
                        println!("DC - Event {:?}", event);
                    }
                    println!("DC - Closed");
                });

                let _channel = channel.create_data_channel(DataChannelType::Reliable, String::from(format!("response - {}", label)), None, 0)
                    .expect("failed to create channel");
            },
            MediaChannelApplicationEvent::StateChanged { new_state } => {
                println!("Application channel state changed to {:?}", new_state);
            }
        }
    }

    for channel in channel.data_channels_mut() {
        while let Poll::Ready(event) = channel.poll_next_unpin(cx) {
            println!("Having a data channel event for {}: {:?}", channel.label(), event);
        }
    }
}

fn poll_audio_channel(channel: &mut MediaChannelAudio, cx: &mut Context) {
    while let Poll::Ready(event) = channel.poll_next_unpin(cx) {
        if !event.is_some() {
            /* TODO: Never poll that media channel again */
            break;
        }

        println!("Audio channel event: {:?}", event.unwrap());
    }
}

fn poll_video_channel(channel: &mut MediaChannelVideo, cx: &mut Context) {
    while let Poll::Ready(event) = channel.poll_next_unpin(cx) {
        if !event.is_some() {
            /* TODO: Never poll that media channel again */
            break;
        }

        match event.as_ref().unwrap() {
            MediaChannelVideoEvents::DataReceived(packet) => {
                let local_channel = channel.local_sources().first();
                if let Some(local_channel) = local_channel {
                    let buffer = packet.create_builder()
                        .ssrc(local_channel.id())
                        .add_csrc(packet.ssrc())
                        .build();

                    if rand::random::<u8>() < 50 && false {
                        println!("Dropping video frame");
                    } else {
                        if let Ok(buffer) = buffer {
                            channel.send_data(buffer);
                        } else {
                            eprintln!("Failed to replay video RTP packet");
                        }
                    }
                }
            },
            _ => {
                println!("Video channel event: {:?}", event.unwrap());
            }
        }
    }
}

fn spawn_client_peer(client: &mut Client<ClientData>) {
    assert!(client.data.peer.is_none());

    println!("Creating a new peer");
    let peer = Arc::new(Mutex::new(rtc::PeerConnection::new(client.data.event_loop.clone().unwrap().get_context())));
    client.data.peer = Some(peer.clone());

    let tx = client.data.command_pipe.0.clone();
    tokio::spawn(async move {
        /* This locks all other access */
        loop {
            let event = futures::future::poll_fn(|cx| {
                let mut peer = peer.lock().unwrap();
                let peer = peer.deref_mut();

                for channel in peer.media_channels_mut().iter_mut() {
                    let mut channel = channel.lock().unwrap();

                    match channel.as_typed() {
                        TypedMediaChannel::Application(channel) => poll_application_channel(channel, cx),
                        TypedMediaChannel::Audio(channel) => poll_audio_channel(channel, cx),
                        TypedMediaChannel::Video(channel) => poll_video_channel(channel, cx)
                    }
                }

                PeerConnection::poll_next_unpin(peer, cx)
            }).await;

            if event.is_none() {
                break;
            }

            match event.unwrap() {
                PeerConnectionEvent::LocalIceCandidate(candidate, media_id) => {
                    if let Some(candidate) = candidate {
                        if candidate.transport == SdpAttributeCandidateTransport::Tcp {
                            tx.send(WebCommand::RtcAddIceCandidate {
                                candidate: String::from("candidate:") + &candidate.to_string(),
                                media_id: media_id.1,
                                media_index: media_id.0 as u32
                            }).expect("failed to send local candidate add");
                        }
                    } else {
                        tx.send(WebCommand::RtcAddIceCandidate {
                            candidate: String::from(""),
                            media_id: media_id.1,
                            media_index: media_id.0 as u32
                        }); //.expect("failed to send local candidate add");
                        //FIXME: This crashes on FF for some reason
                    }
                }
            }
        }

        println!("Peer poll exited");
    });
}

fn configure_media_channel(channel: &mut dyn MediaChannel) -> std::result::Result<(), String> {
    match channel.as_typed() {
        TypedMediaChannel::Audio(channel) => {
            let opus_codec = channel.remote_codecs()
                .iter()
                .find(|e| e.codec_name == "opus");
            if opus_codec.is_none() {
                return Err(String::from("Missing the opus codec"));
            }

            println!("Remote offered opus, we're accepting it: {:?}", &opus_codec);
            let mut codec = opus_codec.unwrap().clone();
            //codec.parameters = None;
            //codec.feedback = None;
            channel.local_codecs_mut().push(codec);

            for ext in channel.remote_extensions().iter().map(|e| e.clone()).collect::<Vec<SdpAttributeExtmap>>() {
                channel.local_extensions_mut().push(ext);
            }

            let source = channel.register_local_source();

            /*
             * Since we sadly can't name tracks we've to name the underlying media stream.
             * But we can't name the media stream without having the cname attribute, so we're setting
             * it to "-" as and now we can name the media stream however we want to name it.
             */
            let stream_name = "WolverinDEV_Is_Nice";
            source.properties_mut().insert(String::from("cname"), Some(String::from("-")));
            source.properties_mut().insert(String::from("msid"), Some(String::from(format!("{} -", stream_name))));
        },
        TypedMediaChannel::Video(channel) => {
            let vp8_codec = channel.remote_codecs()
                .iter()
                .find(|e| e.codec_name == "VP8");
            if vp8_codec.is_none() {
                return Err(String::from("Missing the opus codec"));
            }

            println!("Remote offered VP8, we're accepting it: {:?}", &vp8_codec);
            let mut codec = vp8_codec.unwrap().clone();
            //codec.parameters = None;
            //codec.feedback = None;
            channel.local_codecs_mut().push(codec);

            let stream_name = "WolverinDEV_Is_Nice-Video";
            let source = channel.register_local_source();
            source.properties_mut().insert(String::from("cname"), Some(String::from("-")));
            source.properties_mut().insert(String::from("msid"), Some(String::from(format!("{} -", stream_name))));
        }
        _ => {}
    }

    Ok(())
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

            /* configure the media channels */
            for channel in locked_peer.media_channels_mut().iter() {
                configure_media_channel(channel.lock().unwrap().deref_mut())?;
            }

            let answer = locked_peer.create_answer().map_err(|err| format!("{:?}", err))?;
            println!("Answer: {:?}", answer.to_string());
            let _ = client.data.command_pipe.0.send(WebCommand::RtcSetRemoteDescription { sdp: answer.to_string(), mode: String::from("answer") });
        },
        WebCommand::RtcAddIceCandidate { candidate, media_id, media_index } => {
            let peer = client.data.peer.as_ref().ok_or(String::from("no peer initialized"))?;

            let candidate = SdpAttribute::from_str(candidate.as_str()).map_err(|err| format!("failed to parse candidate: {:?}", err))?;
            let candidate = { if let SdpAttribute::Candidate(c) = candidate { Ok(c) } else { Err(String::from("internal candidate cast error")) } }?;

            let mut peer = peer.lock().unwrap();
            let _ = peer.add_remove_ice_candidate(&MediaIdFragment::Full((*media_index as usize, media_id.clone())), Some(&candidate)).map_err(|err| format!("{:?}", err))
                .map_err(|err| println!("Failed to add remote ice candidate: {:?}", err));

        },
        WebCommand::RtcFinishedIceCandidates {} => {
            let mut peer = client.data.peer.as_ref().ok_or(String::from("no peer initialized"))?
                .lock().unwrap();
            peer.finish_remote_ice_candidates();
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