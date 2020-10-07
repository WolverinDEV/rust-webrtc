use libusrsctp_sys as ffi;
use crate::sctp::{UsrSCTPSocket, AF_CONN, UsrSctpConnectResult};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::task::{Waker, Context, Poll};
use std::pin::Pin;
use std::future::Future;

pub struct UsrSctpConnect {
    socket: Arc<UsrSCTPSocket>,
    local_port: u16,
    remote_port: u16,
    inner: Arc<Mutex<UsrSctpConnectInner>>
}

enum UsrSctpConnectState {
    Uninitialized,
    Connecting(JoinHandle<()>),
    Finished(UsrSctpConnectResult),
    Destroyed
}

struct UsrSctpConnectInner {
    state: UsrSctpConnectState,
    waker: Option<Waker>,
}

impl UsrSctpConnect {
    pub fn new(local_port: u16, remote_port: u16, socket: Arc<UsrSCTPSocket>) -> Self {
        UsrSctpConnect {
            socket,
            local_port,
            remote_port,
            inner: Arc::new(Mutex::new(UsrSctpConnectInner{
                state: UsrSctpConnectState::Uninitialized,
                waker: None
            }))
        }
    }

    pub fn start_connect(&mut self) {
        let mut inner_locked = self.inner.lock().unwrap();

        if !matches!(inner_locked.state, UsrSctpConnectState::Uninitialized) {
            panic!("Start connect called, but sctp connector not in uninitialized state any more");
        }

        let local_port = self.local_port;
        let remote_port = self.remote_port;
        let socket = self.socket.clone();
        let inner = self.inner.clone();
        inner_locked.state = UsrSctpConnectState::Connecting(std::thread::spawn(move || {
            let result = unsafe {
                let mut addr: ffi::sockaddr_conn = std::mem::MaybeUninit::zeroed().assume_init();
                addr.sconn_family = AF_CONN as u16;
                addr.sconn_port = local_port.to_be();
                addr.sconn_addr = socket.address_void_ptr();

                let result = ffi::usrsctp_bind(socket.socket, &mut addr as *mut ffi::sockaddr_conn as *mut libc::sockaddr, std::mem::size_of::<ffi::sockaddr_conn>() as u32);
                if result != 0 {
                    UsrSctpConnectResult::BindFailed(result)
                } else {
                    addr.sconn_port = remote_port.to_be();

                    /* the connect call sadly blocks... */
                    let result = ffi::usrsctp_connect(socket.socket, &mut addr as *mut ffi::sockaddr_conn as *mut libc::sockaddr, std::mem::size_of::<ffi::sockaddr_conn>() as u32);
                    if result != 0 {
                        UsrSctpConnectResult::ConnectFailed(result)
                    } else {
                        UsrSctpConnectResult::Success
                    }
                }
            };

            let mut inner = inner.lock().unwrap();
            inner.state = UsrSctpConnectState::Finished(result);
            if let Some(waker) = &inner.waker { waker.wake_by_ref(); }
        }));
    }

    pub fn abort_connect(&mut self) { /* not possible */ }
}

impl Future for UsrSctpConnect {
    type Output = UsrSctpConnectResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.inner.lock().unwrap();
        inner.waker = Some(cx.waker().clone());
        match &inner.state {
            UsrSctpConnectState::Uninitialized => {
                return Poll::Pending;
            },
            UsrSctpConnectState::Connecting(_) => {
                return Poll::Pending;
            },
            UsrSctpConnectState::Finished(_) => {
                let state = std::mem::replace(&mut inner.state, UsrSctpConnectState::Destroyed);
                if let UsrSctpConnectState::Finished(result) = state {
                    return Poll::Ready(result);
                } else {
                    panic!();
                }
            },
            UsrSctpConnectState::Destroyed => panic!("this future has been polled after a result")
        }
    }
}