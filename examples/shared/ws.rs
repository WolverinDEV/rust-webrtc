use futures::{Stream, StreamExt, Future, FutureExt, SinkExt};
use futures::task::{Context, Poll};
use tokio::macros::support::Pin;
use std::net::SocketAddr;
use tokio_tungstenite::{WebSocketStream, tungstenite};
use tokio::net::{TcpStream, TcpListener};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use std::ops::{DerefMut};
use serde::export::PhantomData;

enum ServerState {
    Unset(),
    Binding(Pin<Box<dyn Future<Output=tokio::io::Result<TcpListener>>>>),
    Listening(TcpListener)
}

pub struct Server<ClientData: Default + Unpin> {
    pub address: String,
    state: ServerState,
    __phantom: PhantomData<ClientData>
}

impl<ClientData: Default + Unpin> Server<ClientData> {
    pub fn new(address: String) -> Self {
        Server {
            address,
            state: ServerState::Unset(),
            __phantom: Default::default()
        }
    }
}

impl<ClientData: Default + Unpin> Stream for Server<ClientData> {
    type Item = Client<ClientData>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match &mut Pin::deref_mut(&mut self).state {
            ServerState::Unset() => {
                self.state = ServerState::Binding(Box::pin(TcpListener::bind("127.0.0.1:1234")));
                cx.waker().clone().wake();
            },
            ServerState::Binding(future) => {
                match Pin::as_mut(future).poll(cx) {
                    Poll::Ready(Ok(socket)) => {
                        self.state = ServerState::Listening(socket);
                        cx.waker().clone().wake();
                    },
                    Poll::Ready(Err(err)) => {
                        eprintln!("Failed to bind server: {:?}", err);
                        return Poll::Ready(None);
                    },
                    Poll::Pending => { return Poll::Pending; }
                }
            },
            ServerState::Listening(listener) => {
                while let Poll::Ready(client) = listener.poll_next_unpin(cx) {
                    if client.is_none() {
                        println!("Server closed");
                        return Poll::Ready(None);
                    }

                    match client.unwrap() {
                        Ok(stream) => {
                            return Poll::Ready(Some(Client::new(stream)));
                        },
                        Err(error) => {
                            eprintln!("Failed to accept new client: {:?}", error);
                        }
                    }
                }
            }
        }

        Poll::Pending
    }
}

pub enum ClientEvents {
    Connected,
    CommandReceived(WebCommand),
    Disconnected
}

type WsFuture<Result> = Pin<Box<dyn Future<Output=Result> + Send>>;
enum ClientSocket {
    Accepting(WsFuture<Result<WebSocketStream<TcpStream>, tungstenite::Error>>),
    Connected(Option<WebSocketStream<TcpStream>>),
    Closing(Option<WebSocketStream<TcpStream>>),
    Closed()
}

pub struct Client<ClientData: Default + Unpin> {
    pub address: SocketAddr,
    pub data: ClientData,
    socket: ClientSocket,
}

impl<ClientData: Default + Unpin> Client<ClientData> {
    fn new(socket: TcpStream) -> Self {
        let address = socket.peer_addr().expect("missing peer address").clone();
        Client {
            address,
            socket: ClientSocket::Accepting(Box::pin(tokio_tungstenite::accept_async(socket))),
            data: Default::default()
        }
    }
    pub fn send_message(&mut self, message: &WebCommand) {
        if let ClientSocket::Connected(stream) = &mut self.socket {
            let stream = stream.as_mut().unwrap();
            if let Err(error) = stream.start_send_unpin(Message::Text(serde_json::to_string(message)
                .expect("failed to encode WS message"))) {
                eprintln!("Failed to send WS message: {:?}", error);
            }
        } else {
            eprintln!("Tried to send a message to a not connected client");
        }
    }

    pub fn close(&mut self, reason: Option<CloseFrame>) {
        if let ClientSocket::Connected(stream) = &mut self.socket {
            let _ = stream.as_mut().unwrap().start_send_unpin(Message::Close(reason.map(|msg| msg.into_owned())));
            self.socket = ClientSocket::Closing(stream.take());
        } else {
            eprintln!("Tried to send a message to a not connected client");
        }
    }

    fn close_connection(&mut self) {
        self.socket = ClientSocket::Closed();
        /* TODO: Call the waker, the stream has ended */
    }
}

impl<ClientData: Default + Unpin> Stream for Client<ClientData> {
    type Item = ClientEvents;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match &mut self.socket {
            ClientSocket::Accepting(future) => {
                return match future.poll_unpin(cx) {
                    Poll::Ready(Ok(socket)) => {
                        self.socket = ClientSocket::Connected(Some(socket));
                        Poll::Ready(Some(ClientEvents::Connected))
                    },
                    Poll::Ready(Err(error)) => {
                        eprintln!("Failed to accept new client: {:?}", error);
                        Poll::Ready(None)
                    },
                    Poll::Pending => {
                        Poll::Pending
                    }
                }
            },
            ClientSocket::Connected(stream) => {
                while let Poll::Ready(message) = stream.as_mut().unwrap().poll_next_unpin(cx) {
                    match message {
                        None => {
                            println!("Remote client closed the connection");
                            self.close_connection();
                            return Poll::Pending;
                        },
                        Some(Err(error)) => {
                            eprintln!("Received WB error. Closing connection: {:?}", error);
                            self.close(None);
                            return Poll::Pending;
                        },
                        Some(Ok(message)) => {
                            match message {
                                Message::Text(message) => {
                                    match serde_json::from_str::<WebCommand>(message.as_str()) {
                                        Ok(command) => {
                                            return Poll::Ready(Some(ClientEvents::CommandReceived(command)));
                                        }
                                        Err(error) => {
                                            println!("Failed to parse json: {:?}", error);
                                        }
                                    }
                                },
                                Message::Close(_) => {
                                    println!("client disconnected");
                                    self.close_connection();
                                    return Poll::Ready(Some(ClientEvents::Disconnected));
                                }
                                message => {
                                    println!("Unknown - Message: {:?}", message);

                                    self.close(Some(CloseFrame{ code: CloseCode::Invalid, reason: "text messages are only supported".into() }));
                                    return Poll::Pending;
                                }
                            }
                        }
                    }
                }
            },
            ClientSocket::Closing(socket) => {
                return match socket.as_mut().unwrap().poll_close_unpin(cx) {
                    Poll::Ready(Ok(..)) => {
                        /* Stream closed */
                        self.socket = ClientSocket::Closed();
                        Poll::Ready(Some(ClientEvents::Disconnected))
                    },
                    Poll::Ready(Err(error)) => {
                        eprintln!("Failed to send close message: {:?}", error);
                        self.socket = ClientSocket::Closed();
                        Poll::Ready(Some(ClientEvents::Disconnected))
                    },
                    Poll::Pending => {
                        Poll::Pending
                    }
                }
            },
            ClientSocket::Closed() => {
                return Poll::Ready(None);
            }
        }

        Poll::Pending
    }
}


#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", content = "payload")]
pub enum WebCommand {
    RtcSetRemoteDescription { sdp: String, mode: String },
    RtcAddIceCandidate { media_index: u32, candidate: String },
    RtcFinishedIceCandidates { }
}