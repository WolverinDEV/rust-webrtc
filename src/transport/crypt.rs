use webrtc_sdp::attribute_type::{SdpAttributeFingerprint, SdpAttributeFingerprintHashType};
use crate::transport::RTCTransportInitializeError;
use openssl::{
    ssl::{Ssl, SslMethod, SslContext},
    bn::BigNum,
    pkey::PKey,
    x509::{X509NameBuilder, X509},
    asn1::Asn1Time,
    hash::MessageDigest,
    rsa::Rsa
};
use std::sync::{Mutex, Arc, Weak};
use tokio::macros::support::{Pin, Future, Poll};
use std::collections::VecDeque;
use futures::task::Waker;
use crate::global_logger;
use std::time::Instant;
use tokio::time::{Duration};
use futures::future::AbortHandle;
use futures::StreamExt;
use lazy_static::lazy_static;

fn generate_ssl_components() -> Result<(SdpAttributeFingerprint, SslContext), RTCTransportInitializeError> {
    let private_key = Rsa::generate_with_e(4096, &BigNum::from_u32(0x10001u32).unwrap())
        .map_err(|stack| RTCTransportInitializeError::PrivateKeyGenFailed {stack})?;

    let private_key = PKey::from_rsa(private_key)
        .map_err(|stack| RTCTransportInitializeError::PrivateKeyGenFailed {stack})?;

    let certificate = {
        let subject = {
            let mut builder = X509NameBuilder::new().unwrap();
            builder.append_entry_by_text("CN", "WebRTC - IMM")
                .map_err(|stack| RTCTransportInitializeError::CertificateGenFailed{ stack })?;
            builder.build()
        };

        let mut cert_builder = X509::builder()
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
        let mut builder = SslContext::builder(SslMethod::dtls())
            .map_err(|stack| RTCTransportInitializeError::SslInitFailed { stack })?;

        builder.set_tlsext_use_srtp("SRTP_AES128_CM_SHA1_80:SRTP_AES128_CM_SHA1_32")
            .map_err(|stack| RTCTransportInitializeError::SslInitFailed { stack })?;

        builder.set_private_key(&private_key)
            .map_err(|stack| RTCTransportInitializeError::SslInitFailed { stack })?;

        builder.set_certificate(&certificate)
            .map_err(|stack| RTCTransportInitializeError::SslInitFailed { stack })?;

        builder.build()
    };

    Ok((fingerprint, ctx))
}

lazy_static!{
    static ref CERTIFICATE_CACHE: Arc<Mutex<CertificateCache>> = CertificateCache::new();
}

pub fn certificate_cache() -> &'static Arc<Mutex<CertificateCache>> {
    &CERTIFICATE_CACHE
}

struct RegenerateContext {
    result: Option<Result<(), RTCTransportInitializeError>>,
    waker: VecDeque<Waker>,
}

pub struct CertificateCache {
    ref_self: Weak<Mutex<Self>>,
    current_context: Result<(SdpAttributeFingerprint, SslContext), RTCTransportInitializeError>,

    current_regenerate: Option<Arc<Mutex<RegenerateContext>>>,
    regenerate_timer_abort: Option<AbortHandle>
}

pub type CertificateRegenerateFuture = Box<dyn Future<Output=Result<(), RTCTransportInitializeError>> + Send>;
impl CertificateCache {
    pub fn new() -> Arc<Mutex<Self>> {
        let result = Arc::new(Mutex::new(CertificateCache{
            ref_self: Weak::new(),
            current_context: Err(RTCTransportInitializeError::NoGeneratedCertificated),

            current_regenerate: None,
            regenerate_timer_abort: None
        }));
        result.lock().unwrap().ref_self = Arc::downgrade(&result);
        result
    }

    pub fn initialize(&mut self) -> Pin<CertificateRegenerateFuture> {
        let cache = self.ref_self.clone();
        let (timer, abort) = futures::future::abortable(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(60 * 60 * 24));
            timer.next().await; /* skip first tick */
            loop {
                timer.next().await;
                if let Some(cache) = cache.upgrade() {
                    let generate_promise = cache.lock().unwrap().regenerate_certificate();
                    drop(cache); /* Don't hold a reference here */
                    let _ = generate_promise.await;
                } else {
                    /* cache went away */
                    break;
                }
            }
        });
        self.regenerate_timer_abort = Some(abort);
        tokio::spawn(timer);

        self.regenerate_certificate()
    }

    pub fn generate_session_ssl(&mut self) -> Result<(Ssl, SdpAttributeFingerprint), RTCTransportInitializeError> {
        let (fingerprint, context) = match &self.current_context {
            Ok(result) => result,
            Err(error) => return Err(error.clone())
        };

        let ssl = Ssl::new(context)
            .map_err(|error| RTCTransportInitializeError::SslInitFailed { stack: error })?;

        Ok((ssl, fingerprint.clone()))
    }

    pub fn regenerate_certificate(&mut self) -> Pin<CertificateRegenerateFuture> {
        let mut dispatch_context = false;
        let current_context = match &self.current_regenerate {
            Some(context) => context.clone(),
            None => {
                let context = Arc::new(Mutex::new(RegenerateContext{
                    waker: VecDeque::with_capacity(4),
                    result: None
                }));
                self.current_regenerate = Some(context.clone());
                dispatch_context = true;
                context
            }
        };

        if dispatch_context {
            let logger = global_logger();
            let cache = self.ref_self.clone();
            let current_context = current_context.clone();

            std::thread::spawn(move || {
                slog::debug!(logger, "Generating new transport private key and certificate");

                let time_begin = Instant::now();
                let (mut generated_certificate, generate_error) = match generate_ssl_components() {
                    Ok(result) => (Some(result), None),
                    Err(error) => (None, Some(error))
                };

                let mut cache_gone = false;
                if let Some(cache) = cache.upgrade() {
                    let mut cache = cache.lock().unwrap();
                    if let Some(result) = generated_certificate.take() {
                        cache.current_context = Ok(result);
                    } else if let Some(error) = &generate_error {
                        cache.current_context = Err(error.clone());
                    } else {
                        unreachable!();
                    }

                    cache.current_regenerate = None;
                } else {
                    cache_gone = true;
                }

                {
                    let mut current_context = current_context.lock().unwrap();
                    if let Some(error) = &generate_error {
                        current_context.result = Some(Err(error.clone()));
                    } else {
                        /* the actual result might already be taken */
                        current_context.result = Some(Ok(()));
                    }

                    for waker in current_context.waker.iter() {
                        waker.wake_by_ref();
                    }
                }
                let time_end = Instant::now();

                if !cache_gone {
                    let time_needed =  time_end.duration_since(time_begin).as_millis();
                    if let Some(error) = &generate_error {
                        slog::error!(logger, "Failed to generate transport certificate and private key successfully (time needed: {}ms): {:?}", time_needed, error);
                    } else {
                        slog::debug!(logger, "Transport certificate and private key successfully generated within {}ms", time_needed);
                    }
                }
            });
        }

        return Box::pin(futures::future::poll_fn(move |cx| {
            let mut current_context = current_context.lock().unwrap();
            if let Some(result) = &current_context.result {
                Poll::Ready(result.clone())
            } else {
                current_context.waker.push_back(cx.waker().clone());
                Poll::Pending
            }
        }));
    }
}

impl Drop for CertificateCache {
    fn drop(&mut self) {
        if let Some(handle) = self.regenerate_timer_abort.take() {
            handle.abort();
        }
    }
}