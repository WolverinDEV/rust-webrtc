use libnice::ice::{ComponentState, Candidate, ComponentWriter};
use tokio::sync::mpsc;
use std::pin::Pin;
use futures::prelude::*;
use futures::task::{Poll};
use std::task::Context;
use std::collections::{VecDeque};
use futures::{StreamExt};
use openssl::{ x509, pkey, rsa, ssl };
use openssl::bn::BigNum;
use openssl::error::ErrorStack;
use openssl::asn1::{Asn1Time};
use openssl::hash::MessageDigest;
use webrtc_sdp::attribute_type::{SdpAttributeFingerprintHashType, SdpAttributeFingerprint, SdpAttributeSetup};
use std::ffi::CString;
use webrtc_sdp::address::Address;
use openssl::ssl::{SslMethod};
use std::io::{Read, Write};
use futures::io::ErrorKind;
use std::cell::RefCell;
use std::rc::Rc;
use serde::export::Formatter;
use std::ops::Deref;
use crate::srtp2::{Srtp2, Srtp2ErrorCode};
use crate::utils::rtp::is_rtp_header;
use crate::utils::rtcp::is_rtcp_header;
use std::sync::atomic::{AtomicU32, Ordering};

pub mod packet_history;
mod sender;
pub use sender::*;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct DebugableCandidate {
    inner: Candidate
}

impl std::fmt::Debug for DebugableCandidate {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(format!("{}", &self.inner).as_str())
    }
}

impl Deref for DebugableCandidate {
    type Target = Candidate;

    fn deref(&self) -> &Self::Target { &self.inner }
}

impl Into<Candidate> for DebugableCandidate {
    fn into(self) -> Candidate {
        self.inner
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ICECredentials {
    pub username: String,
    pub password: String
}

#[derive(Clone, Debug, PartialEq)]
pub enum RTCTransportFailReason {
    DtlsInitFailed,
    DtlsError,
    DtlsEof,

    IceFinished,
    IceStreamFinished,
    IceFailure,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RTCTransportState {
    Disconnected,
    Connecting,
    Connected,
    Failed(RTCTransportFailReason)
}

#[derive(Clone, Debug)]
pub enum RTCTransportEvent {
    TransportStateChanged,
    IceStateChanged(ComponentState),

    LocalIceCandidate(DebugableCandidate),
    LocalIceGatheringFinished(),

    MessageReceivedDtls(Vec<u8>),
    MessageReceivedRtp(Vec<u8>),
    MessageReceivedRtcp(Vec<u8>),
    /// We've dropped a message (only when feature `simulated-loss` has been activated)
    MessageDropped(Vec<u8>),
}

#[derive(Clone, Debug)]
pub enum RTCTransportControl {
    SendMessage(Vec<u8>)
}

#[derive(Debug)]
pub enum RTCTransportInitializeError {
    PrivateKeyGenFailed{ stack: ErrorStack },
    CertificateGenFailed{ stack: ErrorStack },
    FingerprintGenFailed{ stack: ErrorStack },
    SslInitFailed { stack: ErrorStack }
}

#[derive(Debug)]
pub enum RTCTransportICECandidateAddError {
    UnknownMediaChannel,
    RemoteCandidatesAlreadyReceived,
    FqdnNotYetSupported,
    InvalidComponentIndex
}

enum DTLSState {
    Uninitialized(ssl::Ssl),
    Handshake(ssl::MidHandshakeSslStream<DtlsStream>),
    Connected(ssl::SslStream<DtlsStream>),
    Failed()
}

pub struct RTCTransport {
    /// A unique id identifying this transport
    pub transport_id: u32,

    /// Index of the "owning" media line
    pub owning_media_line: u32,
    /// Containing all media lines which actively listening to the channel events (bundled channels)
    pub media_lines: Vec<u32>,

    state: RTCTransportState,
    pending_events: VecDeque<RTCTransportEvent>,

    pub local_credentials: ICECredentials,
    pub remote_credentials: ICECredentials,

    //certificate: x509::X509,
    //private_key: pkey::PKey<pkey::Private>,
    pub fingerprint: SdpAttributeFingerprint,

    pub setup: SdpAttributeSetup,

    local_candidates_gathered: bool,
    remote_candidates_gathered: bool,

    control_receiver: mpsc::UnboundedReceiver<RTCTransportControl>,
    pub control_sender: mpsc::UnboundedSender<RTCTransportControl>,

    #[cfg(feature = "simulated-loss")]
    simulated_loss: u8,

    last_ice_state: ComponentState,
    ice_stream: Option<libnice::ice::Stream>,

    /// State containing the current dtls state
    dtls: Option<DTLSState>,
    /// Internal buffer for the dtls stream
    dtls_buffer: Rc<RefCell<DtlsStreamSource>>,

    srtp_in: Option<Srtp2>,
    sender_backend: Arc<Mutex<Option<SenderBackend>>>,
}

static TRANSPORT_ID_INDEX: AtomicU32 = AtomicU32::new(1);
impl RTCTransport {
    fn generate_ssl_components() -> Result<(SdpAttributeFingerprint, ssl::Ssl), RTCTransportInitializeError> {
        let private_key = rsa::Rsa::generate_with_e(4096, &BigNum::from_u32(0x10001u32).unwrap())
            .map_err(|stack| RTCTransportInitializeError::PrivateKeyGenFailed {stack})?;

        let private_key = pkey::PKey::from_rsa(private_key)
            .map_err(|stack| RTCTransportInitializeError::PrivateKeyGenFailed {stack})?;

        let certificate = {
            let subject = {
                let mut builder = x509::X509NameBuilder::new().unwrap();
                builder.append_entry_by_text("CN", "WebRTC - IMM")
                    .map_err(|stack| RTCTransportInitializeError::CertificateGenFailed{ stack })?;
                builder.build()
            };

            let mut cert_builder = x509::X509::builder()
                .map_err(|stack| RTCTransportInitializeError::CertificateGenFailed{ stack })?;

            cert_builder.set_pubkey(&private_key)
                .map_err(|stack| RTCTransportInitializeError::CertificateGenFailed{ stack })?;

            cert_builder.set_version(0)
                .map_err(|stack| RTCTransportInitializeError::CertificateGenFailed{ stack })?;

            cert_builder.set_subject_name(&subject)
                .map_err(|stack| RTCTransportInitializeError::CertificateGenFailed{ stack })?;

            cert_builder.set_issuer_name(&subject)
                .map_err(|stack| RTCTransportInitializeError::CertificateGenFailed{ stack })?;

            cert_builder.set_not_before(&Asn1Time::from_unix(0).unwrap())
                .map_err(|stack| RTCTransportInitializeError::CertificateGenFailed{ stack })?;

            cert_builder.set_not_after(&Asn1Time::days_from_now(14).unwrap())
                .map_err(|stack| RTCTransportInitializeError::CertificateGenFailed{ stack })?;

            cert_builder.sign(&private_key, MessageDigest::sha1())
                .map_err(|stack| RTCTransportInitializeError::CertificateGenFailed{ stack })?;

            cert_builder.build()
        };

        let fingerprint = {
            let fingerprint = certificate.digest(MessageDigest::sha256())
                .map_err(|stack| RTCTransportInitializeError::FingerprintGenFailed{ stack })?;

            SdpAttributeFingerprint{
                fingerprint: fingerprint.to_vec(),
                hash_algorithm: SdpAttributeFingerprintHashType::Sha256
            }
        };

        let ctx = {
            let mut builder = ssl::SslContext::builder(SslMethod::dtls())
                .map_err(|stack| RTCTransportInitializeError::SslInitFailed { stack })?;

            builder.set_tlsext_use_srtp("SRTP_AES128_CM_SHA1_80:SRTP_AES128_CM_SHA1_32")
                .map_err(|stack| RTCTransportInitializeError::SslInitFailed { stack })?;

            builder.set_private_key(&private_key)
                .map_err(|stack| RTCTransportInitializeError::SslInitFailed { stack })?;

            builder.set_certificate(&certificate)
                .map_err(|stack| RTCTransportInitializeError::SslInitFailed { stack })?;

            builder.build()
        };

        let ssl = ssl::Ssl::new(&ctx)
            .map_err(|stack| RTCTransportInitializeError::SslInitFailed { stack })?;

        Ok((fingerprint, ssl))
    }

    pub fn new(mut stream: libnice::ice::Stream,
               remote_credentials: ICECredentials,
               media_line: u32,
               setup: SdpAttributeSetup
    ) -> Result<RTCTransport, RTCTransportInitializeError> {
        assert_eq!(stream.components().len(), 1, "expected only one stream component");


        let (fingerprint, ssl) = RTCTransport::generate_ssl_components()?;
        stream.set_remote_credentials(CString::new(remote_credentials.username.clone()).unwrap(), CString::new(remote_credentials.password.clone()).unwrap());

        let dtls_stream = DtlsStreamSource{
            verbose: false ,
            read_buffer_offset: 0,
            read_buffer: VecDeque::with_capacity(32),
            writer: stream.mut_components()[0].writer()
        };

        let (ctx, crx) = mpsc::unbounded_channel();

        let connection = RTCTransport {
            transport_id: TRANSPORT_ID_INDEX.fetch_add(1, Ordering::Relaxed),

            owning_media_line: media_line,
            media_lines: Vec::with_capacity(3),

            state: RTCTransportState::Disconnected,
            pending_events: VecDeque::new(),

            remote_credentials,
            local_credentials: ICECredentials{
                username: String::from(stream.get_local_ufrag()),
                password: String::from(stream.get_local_pwd())
            },

            //certificate,
            //private_key,
            fingerprint,

            setup,

            local_candidates_gathered: false,
            remote_candidates_gathered: false,

            last_ice_state: ComponentState::Disconnected,

            control_receiver: crx,
            control_sender: ctx,

            #[cfg(feature = "simulated-loss")]
            simulated_loss: 0,

            ice_stream: Some(stream),
            dtls_buffer: Rc::new(RefCell::new(dtls_stream)),

            dtls: Some(DTLSState::Uninitialized(ssl)),
            srtp_in: None,

            sender_backend: Arc::new(Mutex::new(None))
        };

        Ok(connection)
    }

    pub fn state(&self) -> &RTCTransportState {
        &self.state
    }

    pub fn add_remote_candidate(&mut self, candidate: Option<&Candidate>) -> Result<(), RTCTransportICECandidateAddError> {
        if let RTCTransportState::Failed(..) = &self.state {
            /* TODO: Should we really return okey here? */
            return Ok(());
        }

        if self.remote_candidates_gathered {
            Err(RTCTransportICECandidateAddError::RemoteCandidatesAlreadyReceived)
        } else if let Some(candidate) = candidate {
            let candidate = candidate.clone();
            if let Address::Fqdn(_address) = &candidate.address {
                Err(RTCTransportICECandidateAddError::FqdnNotYetSupported)
                /*
                if address.ends_with(".local") {
                    candidate.address = Address::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST));
                }
                */
            } else {
                let ice_stream = self.ice_stream.as_mut().expect("missing ice stream");
                if candidate.component as usize > ice_stream.components().len() {
                    Err(RTCTransportICECandidateAddError::InvalidComponentIndex)
                } else {
                    ice_stream.add_remote_candidate(candidate);
                    Ok(())
                }
            }
        } else {
            self.remote_candidates_gathered = true;
            Ok(())
        }
    }

    pub fn create_rtp_sender(&mut self) -> RtpSender {
        RtpSender::new(self.sender_backend.clone())
    }

    pub fn create_rtcp_sender(&mut self) -> RtcpSender {
        RtcpSender::new(self.sender_backend.clone())
    }

    #[cfg(feature = "simulated-loss")]
    pub fn set_simulated_loss(&mut self, loss: u8) {
        self.simulated_loss = loss;
    }

    fn process_dtls_handshake_result(&mut self, result: Result<ssl::SslStream<DtlsStream>, ssl::HandshakeError<DtlsStream>>) -> Option<RTCTransportEvent> {
        match result {
            Ok(stream) => {
                match Srtp2::from_openssl(stream.ssl()) {
                    Ok(srtp) => {
                        self.srtp_in = Some(srtp.0);

                        let mut backend = self.sender_backend.lock().unwrap();
                        *backend = Some(SenderBackend{
                            srtp: srtp.1,
                            transport: self.ice_stream.as_mut().unwrap().mut_components()[0].writer()
                        });
                    },
                    Err(err) => {
                        eprintln!("Failed to initialize srtp from openssl init: {:?}", err);
                        self.failure_tear_down(RTCTransportFailReason::DtlsInitFailed);
                        return Some(RTCTransportEvent::TransportStateChanged);
                    }
                }

                self.dtls = Some(DTLSState::Connected(stream));
                self.update_state();
                self.pending_events.pop_front()
            },
            Err(error) => {
                match error {
                    ssl::HandshakeError::WouldBlock(handshake) => {
                        self.dtls = Some(DTLSState::Handshake(handshake));
                        None
                    },
                    ssl::HandshakeError::Failure(handshake) => {
                        println!("HS failed: {:?}", handshake.error());
                        self.dtls = Some(DTLSState::Failed());
                        self.failure_tear_down(RTCTransportFailReason::DtlsInitFailed);
                        Some(RTCTransportEvent::TransportStateChanged)
                    },
                    ssl::HandshakeError::SetupFailure(error) => {
                        println!("HS setup error: {:?}", error);
                        self.dtls = Some(DTLSState::Failed());
                        self.failure_tear_down(RTCTransportFailReason::DtlsInitFailed);
                        Some(RTCTransportEvent::TransportStateChanged)
                    }
                }
            }
        }
    }

    fn process_incoming_data(&mut self, mut data: Vec<u8>) -> Option<RTCTransportEvent> {
        #[cfg(feature = "simulated-loss")]
        if rand::random::<u8>() < self.simulated_loss {
            return Some(RTCTransportEvent::MessageDropped(data));
        }

        if DtlsStream::is_ssl_packet(&data) {
            self.dtls_buffer.borrow_mut().read_buffer.push_back(data);
        } else if is_rtp_header(data.as_slice()) {
            if let Some(srtp) = &self.srtp_in {
                match srtp.unprotect(data.as_mut_slice()) {
                    Ok(len) => {
                        data.truncate(len);
                        return Some(RTCTransportEvent::MessageReceivedRtp(data));
                    },
                    Err(err) => {
                        if err == Srtp2ErrorCode::ReplayFail {
                            /* we've probably re-requested the packet twice */
                        } else {
                            eprintln!("Failed to unprotect rtp packet: {:?}", err);
                        }
                    }
                }
            } else {
                eprintln!("Received SRTCP data, but we've not initialized srtp yet.");
            }
        } else if is_rtcp_header(data.as_slice()) {
            if let Some(srtp) = &self.srtp_in {
                match srtp.unprotect_rtcp(data.as_mut_slice()) {
                    Ok(len) => {
                        data.truncate(len);
                        return Some(RTCTransportEvent::MessageReceivedRtcp(data));
                    },
                    Err(err) => {
                        eprintln!("Failed to unprotect rtcp packet: {:?}", err);
                    }
                }
            } else {
                eprintln!("Received SRTP data, but we've not initialized srtp yet.");
            }
        } else {
            eprintln!("Received non DTLS, RTP or RTCP data. Dropping data.");
        }
        None
    }

    fn failure_tear_down(&mut self, reason: RTCTransportFailReason) {
        self.state = RTCTransportState::Failed(reason);

        /* cleanup the resources */
        self.dtls = None;
        self.ice_stream = None;
        *self.sender_backend.lock().unwrap() = None;
        RefCell::borrow_mut(&self.dtls_buffer).flush();
    }

    fn update_state(&mut self) {
        if let RTCTransportState::Failed(..) = self.state {
            return;
        }

        let state = {
            match self.last_ice_state {
                ComponentState::Ready |
                ComponentState::Connected => {
                    if let Some(DTLSState::Connected(..)) = &self.dtls {
                        RTCTransportState::Connected
                    } else {
                        RTCTransportState::Connecting
                    }
                },
                ComponentState::Gathering |
                ComponentState::Connecting => {
                    RTCTransportState::Connecting
                },
                ComponentState::Disconnected => {
                    RTCTransportState::Disconnected
                },
                ComponentState::Failed => {
                    panic!("this should never happen");
                }
            }
        };

        if state != self.state {
            self.state = state;
            self.pending_events.push_back(RTCTransportEvent::TransportStateChanged);
        }
    }
}

impl Stream for RTCTransport {
    type Item = RTCTransportEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if !self.pending_events.is_empty() {
            return Poll::Ready(Some(self.pending_events.pop_front().unwrap()));
        }

        if let RTCTransportState::Failed(..) = self.state {
            /* we can't do anything... */
            return Poll::Pending;
        }

        if !self.local_candidates_gathered {
            if let Poll::Ready(candidate) = self.ice_stream.as_mut().expect("missing ice stream").poll_next_unpin(cx) {
                return if let Some(ice_candidate) = candidate {
                    Poll::Ready(Some(RTCTransportEvent::LocalIceCandidate(DebugableCandidate{ inner: ice_candidate })))
                } else {
                    self.local_candidates_gathered = true;
                    Poll::Ready(Some(RTCTransportEvent::LocalIceGatheringFinished()))
                }
            }
        }

        let ice_state: ComponentState;
        {
            let component = &mut self.ice_stream.as_mut().expect("missing ice stream").mut_components()[0];
            while let Poll::Ready(_) = component.poll_state(cx) {
                self.failure_tear_down(RTCTransportFailReason::IceFinished);
                return Poll::Ready(Some(RTCTransportEvent::TransportStateChanged));
            }

            ice_state = component.get_state();
            if ice_state != self.last_ice_state {
                println!("ICE state change from {:?} to {:?}", self.last_ice_state, ice_state);
                self.last_ice_state = ice_state;
                if ice_state == ComponentState::Failed {
                    self.failure_tear_down(RTCTransportFailReason::IceFailure);
                    return Poll::Ready(Some(RTCTransportEvent::TransportStateChanged));
                } else {
                    self.update_state();
                    return Poll::Ready(Some(RTCTransportEvent::IceStateChanged(ice_state)));
                }
            }
        }

        while let Poll::Ready(data) = { self.ice_stream.as_mut().expect("missing ice stream").mut_components()[0].poll_next_unpin(cx) } {
            if let Some(data) = data {
                if let Some(event) = self.process_incoming_data(data) {
                    return Poll::Ready(Some(event));
                }
            } else {
                self.failure_tear_down(RTCTransportFailReason::IceStreamFinished);
                return Poll::Ready(Some(RTCTransportEvent::TransportStateChanged));
            }
        }

        match self.dtls.as_mut().expect("missing dtls state") {
            DTLSState::Uninitialized(_) => {
                if self.last_ice_state == ComponentState::Ready {
                    if let DTLSState::Uninitialized(ssl) = self.dtls.take().unwrap() {
                        /* lets start handshaking */
                        let mut stream_builder = ssl::SslStreamBuilder::new(ssl, DtlsStream{ source: self.dtls_buffer.clone() });

                        let result = match self.setup {
                            SdpAttributeSetup::Active => {
                                stream_builder.set_connect_state();
                                stream_builder.connect()
                            },
                            SdpAttributeSetup::Passive => {
                                stream_builder.set_accept_state();
                                stream_builder.accept()
                            },
                            _ => panic!("Invalid setup state. Expecting Active or Passive")
                        };

                        /* process_dtls_handshake_result must set the DTLS state again! */
                        if let Some(event) = self.process_dtls_handshake_result(result) {
                            assert!(self.dtls.is_some());
                            return Poll::Ready(Some(event));
                        }
                        assert!(self.dtls.is_some());
                    }
                }
            },
            DTLSState::Handshake(_) => {
                if let DTLSState::Handshake(handshake) = self.dtls.take().unwrap() {
                    /* process_dtls_handshake_result must set the DTLS state again! */
                    if let Some(event) = self.process_dtls_handshake_result(handshake.handshake()) {
                        assert!(self.dtls.is_some());
                        return Poll::Ready(Some(event));
                    }
                    assert!(self.dtls.is_some());
                }

            },
            DTLSState::Connected(stream) => {
                /*
                 * Attention: We're using DTLS, this means that we've to received a whole decrypted datagram at once.
                 * Since the MTU is around 1500 bytes 2048 should be enough.
                 */
                let mut buffer = [0u8; 2048];
                match stream.read(&mut buffer) {
                    Ok(read) => {
                        return if read == 0 {
                            self.failure_tear_down(RTCTransportFailReason::DtlsEof);
                            Poll::Ready(Some(RTCTransportEvent::TransportStateChanged))
                        } else {
                            Poll::Ready(Some(RTCTransportEvent::MessageReceivedDtls(buffer[..read].to_vec())))
                        }
                    },
                    Err(error) => {
                        match &error.kind() {
                            &ErrorKind::WouldBlock => { /* nothing to do */ },
                            _ => {
                                eprintln!("Received DTLS read error: {}", &error);
                                self.failure_tear_down(RTCTransportFailReason::DtlsError);
                                return Poll::Ready(Some(RTCTransportEvent::TransportStateChanged));
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        while let Poll::Ready(message) = self.control_receiver.poll_next_unpin(cx) {
            let message = message.expect("unexpected control channel end");
            match message {
                RTCTransportControl::SendMessage(buffer) => {
                    if let Some(DTLSState::Connected(stream)) = &mut self.dtls {
                        /* TODO: better error handling? */
                        stream.write(&buffer)
                            .expect("failed to send message via dtls");
                    } else {
                        eprintln!("Tried to send a dtls message without an active session");
                    }
                }
            }
        }

        /* maybe some of the above polled methods triggered some internal events which needs to be processed */
        if !self.pending_events.is_empty() {
            cx.waker().wake_by_ref();
        }

        Poll::Pending
    }
}

struct DtlsStreamSource {
    verbose: bool,

    read_buffer: VecDeque<Vec<u8>>,
    read_buffer_offset: usize,

    writer: ComponentWriter
}

impl DtlsStreamSource {
    pub fn flush(&mut self) {
        self.read_buffer.clear();
        self.read_buffer_offset = 0;
    }
}

struct DtlsStream {
    source: Rc<RefCell<DtlsStreamSource>>
}

impl DtlsStream {
    pub fn is_ssl_packet(buf: &[u8]) -> bool {
        buf.len() > 1 && (buf[0] >= 20 && buf[0] <= 64)
    }
}

impl Read for DtlsStream {
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        let mut source = self.source.borrow_mut();
        let result = {
            if let Some(head) = source.read_buffer.front() {
                assert!(source.read_buffer_offset < head.len());

                let written = buf.write(&head[source.read_buffer_offset..])?;
                if source.read_buffer_offset + written == head.len() {
                    source.read_buffer.pop_front();
                    source.read_buffer_offset = 0
                } else {
                    source.read_buffer_offset += written;
                }

                Ok(written)
            } else {
                Err(std::io::Error::new(ErrorKind::WouldBlock, "no data"))
            }
        };
        if source.verbose {
            println!("DtlsStream try read {:?} -> {:?}", buf.len(), result);
        }
        result
    }
}

impl Write for DtlsStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut source = self.source.borrow_mut();
        if source.verbose {
            println!("DtlsStream write {:?}", buf.len());
        }
        source.writer.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut source = self.source.borrow_mut();
        source.writer.flush()
    }
}
