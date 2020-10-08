#![feature(once_cell)]
#![feature(drain_filter)]
#![feature(try_trait)]

use tokio::net::{TcpListener, TcpStream};
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
use crate::rtc::{RtcDescriptionType, MediaIdFragment, PeerConnection, PeerConnectionEvent};
use std::sync::{Arc};
use futures::lock::{Mutex, MutexLockFuture};
use std::ops::DerefMut;
use std::str::FromStr;
use futures::task::{Poll, Context};
use tokio::sync::mpsc;
use crate::media::{TypedMediaChannel, MediaChannel};
use crate::media::application::{MediaChannelApplicationEvent, MediaChannelApplication};
use crate::sctp::message::DataChannelType;
use crate::media::audio::MediaChannelAudio;
use crate::srtp2::srtp2_global_init;

mod rtc;
mod media;
mod ice;
mod sctp;
mod srtp2;
mod utils;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", content = "payload")]
enum WebCommand {
    RtcSetRemoteDescription { sdp: String, mode: String },
    RtcAddIceCandidate { media_index: u32, media_id: String, candidate: String },
    RtcFinishedIceCandidates { }
}


struct WebClient {
    _address: SocketAddr,
    socket: Option<Arc<WebSocketStream<TcpStream>>>,
    peer: Option<Arc<Mutex<rtc::PeerConnection>>>,
    event_loop: Option<glib::MainLoop>,

    command_sender: mpsc::UnboundedSender<WebCommand>
}

fn main() {
    srtp2_global_init().expect("srtp2 init failed");
    openssl::init();
    // unsafe { libnice::sys::nice_debug_enable(1); }
    let mut runtime = tokio::runtime::Builder::new()
        .threaded_scheduler()
        .enable_all()
        .core_threads(1)
        .max_threads(1)
        .build().unwrap();

    runtime.block_on(async move {
        let mut socket = TcpListener::bind("127.0.0.1:1234").await.map_err(|_| "failed to bind socket").unwrap();
        while let Ok((stream, _)) = socket.accept().await {
            println!("Having a new TCP connection");

            tokio::spawn(async move {
                let address = stream.peer_addr().unwrap().clone();
                match tokio_tungstenite::accept_async(stream).await {
                    Err(error) => {
                        println!("Failed to accept client {:?}: {:?}. Closing connection.", address, error);
                    },
                    Ok(socket) => {
                        let (tx, rx) = mpsc::unbounded_channel::<WebCommand>();
                        execute_client(WebClient{
                            _address: socket.get_ref().peer_addr().unwrap().clone(),
                            socket: Some(Arc::new(socket)),
                            peer: None,
                            event_loop: None,
                            command_sender: tx
                        }, rx).await
                    }
                }
            });
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

fn spawn_client_peer(client: &mut WebClient) {
    assert!(client.peer.is_none());

    println!("Creating a new peer");
    let peer = Arc::new(Mutex::new(rtc::PeerConnection::new(client.event_loop.clone().unwrap().get_context())));
    client.peer = Some(peer.clone());

    let tx = client.command_sender.clone();
    tokio::spawn(async move {
        /* This locks all other access */
        let mut lock_future: Option<MutexLockFuture<PeerConnection>> = None;

        loop {
            let event = futures::future::poll_fn(|cx| {
                if lock_future.is_none() {
                    lock_future = Some(peer.lock());
                }

                let future = lock_future.as_mut().unwrap();
                if let Poll::Ready(mut peer) = future.poll_unpin(cx) {
                    lock_future = None;
                    let peer = peer.deref_mut();

                    for channel in peer.media_channels_mut().iter_mut() {
                        let mut channel = channel.lock().unwrap();

                        match channel.as_typed() {
                            TypedMediaChannel::Application(channel) => poll_application_channel(channel, cx),
                            TypedMediaChannel::Audio(channel) => poll_audio_channel(channel, cx)
                        }
                    }

                    PeerConnection::poll_next_unpin(peer, cx)
                } else {
                    Poll::Pending
                }
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
        _ => {}
    }

    Ok(())
}

async fn handle_command(client: &mut WebClient, command: &WebCommand) -> std::result::Result<(), String> {
    println!("Received web command: {:?}", command);
    match command {
        WebCommand::RtcSetRemoteDescription{ mode, sdp } => {
            if mode != "offer" {
                return Err(String::from("We only support rtp offers"));
            }

            let sdp = parse_sdp(sdp.as_str(), false).map_err(|err| format!("failed to parse sdp: {:?}", err))?;
            println!("Offer contains {} media streams", sdp.media.len());

            if client.event_loop.is_none() {
                let context = glib::MainContext::new();
                let instance = glib::MainLoop::new(Some(&context), false);
                client.event_loop = Some(instance.clone());

                /* loop will be terminated as soon the client disconnects */
                std::thread::spawn(move || {
                    if !instance.get_context().acquire() {
                        panic!("failed to acquire event loop context");
                    }
                    instance.run();
                });
            }

            if client.peer.is_none() {
                spawn_client_peer(client);
            }

            let peer = client.peer.as_ref().expect("expected a peer");
            let mut locked_peer = peer.lock().await;
            locked_peer.set_remote_description(&sdp, RtcDescriptionType::Offer).map_err(|err| format!("{:?}", err))?;

            /* configure the media channels */
            for channel in locked_peer.media_channels_mut().iter() {
                configure_media_channel(channel.lock().unwrap().deref_mut())?;
            }

            let answer = locked_peer.create_answer().map_err(|err| format!("{:?}", err))?;
            println!("Answer: {:?}", answer.to_string());
            let _ = client.command_sender.send(WebCommand::RtcSetRemoteDescription { sdp: answer.to_string(), mode: String::from("answer") });
        },
        WebCommand::RtcAddIceCandidate { candidate, media_id, media_index } => {
            let peer = client.peer.as_ref().ok_or(String::from("no peer initialized"))?;

            let candidate = SdpAttribute::from_str(candidate.as_str()).map_err(|err| format!("failed to parse candidate: {:?}", err))?;
            let candidate = { if let SdpAttribute::Candidate(c) = candidate { Ok(c) } else { Err(String::from("internal candidate cast error")) } }?;

            let _ = peer.lock().await.add_remove_ice_candidate(&MediaIdFragment::Full((*media_index as usize, media_id.clone())), Some(&candidate)).map_err(|err| format!("{:?}", err))
                .map_err(|err| println!("Failed to add remote ice candidate: {:?}", err));

        },
        WebCommand::RtcFinishedIceCandidates {} => {
            let peer = client.peer.as_ref().ok_or(String::from("no peer initialized"))?;
            peer.lock().await.finish_remote_ice_candidates();
        }
    }

    Ok(())
}

async fn execute_client(client: WebClient, mut rx: mpsc::UnboundedReceiver<WebCommand>) {
    println!("Client connected");

    let mut client = client;
    loop {
        let read_future = futures::future::poll_fn(|cx| Arc::get_mut(client.socket.as_mut().expect("missing socket")).unwrap().poll_next_unpin(cx));
        let write_future = futures::future::poll_fn(|cx| rx.poll_next_unpin(cx));

        match futures::future::select(read_future, write_future).await {
            Either::Left((read, _write)) => {
                if let Some(message) = read {
                    match message {
                        Ok(Message::Text(message)) => {
                            match serde_json::from_str::<WebCommand>(message.as_str()) {
                                Ok(command) => {
                                    if let Err(error) = handle_command(&mut client, &command).await {
                                        println!("Failed to handle a command: {:?}", error);
                                        let _ = Arc::get_mut(client.socket.as_mut().unwrap()).unwrap().close(Some(CloseFrame{ code: CloseCode::Invalid, reason: "command handling failed".into() })).await;
                                        break;
                                    }
                                }
                                Err(error) => {
                                    println!("Failed to parse json: {:?}", error);
                                }
                            }
                        },
                        Ok(Message::Close(_)) => {
                            println!("client disconnected");
                            break;
                        }
                        Ok(message) => {
                            println!("Unknown - Message: {:?}", message);

                            let _ = Arc::get_mut(client.socket.as_mut().unwrap()).unwrap().close(Some(CloseFrame{ code: CloseCode::Invalid, reason: "text messages are only supported".into() })).await;
                            break;
                        }
                        Err(error) => {
                            println!("error: {:?}", error);
                        }
                    }
                } else {
                    println!("client disconnected");
                    break;
                }
            },
            Either::Right((message, _read)) => {
                let message = message.expect("unexpected channel close");
                Arc::get_mut(client.socket.as_mut().unwrap()).unwrap().send(Message::Text(serde_json::to_string(&message).expect("failed to stingify message"))).await.expect("failed to send message");
            }
        }
    }

    if let Some(ev_loop) = client.event_loop {
        ev_loop.quit();
    }
}