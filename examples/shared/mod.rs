use crate::shared::ws::{Client, Server};
use std::future::Future;
use futures::StreamExt;

pub mod gio;
pub mod ws;

pub fn execute_example<T, D, F>(user_callback: T)
    where T: Fn(Client<D>) -> F,
          F: Future<Output = ()> + Send + 'static,
          D: Default + Unpin
{
    let mut runtime = tokio::runtime::Builder::new()
        .threaded_scheduler()
        .enable_all()
        .core_threads(1)
        .max_threads(1)
        .build().unwrap();

    runtime.block_on(async move {
        let mut server = Server::new(String::from("127.0.0.1:1234"));
        loop {
            let (client, server_) = server.into_future().await;
            server = server_;
            if client.is_none() {
                /* server has been closed */
                break;
            }

            let socket = client.unwrap();
            println!("Received new client: {:?}", &socket.peer_addr().unwrap());

            tokio::spawn(user_callback(Client::<D>::new(socket)));
        }
    });
}