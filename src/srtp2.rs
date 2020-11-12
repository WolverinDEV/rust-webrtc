#![allow(dead_code)]

use libsrtp2_sys as ffi;
use core::{mem};
use std::os::raw::{c_void, c_int, c_ulong};
use openssl::srtp::SrtpProfileId;
use openssl::error::ErrorStack;
use crate::global_logger;

/* SRTP stuff (http://tools.ietf.org/html/rfc3711) */
/* cipher_key_length: 128 bits / 16 bytes, cipher_salt_length: 112 bits / 14 bytes */
const SRTP_MASTER_KEY_LENGTH: usize = 16;
const SRTP_MASTER_SALT_LENGTH: usize = 14;
const SRTP_MASTER_LENGTH: usize = SRTP_MASTER_KEY_LENGTH + SRTP_MASTER_SALT_LENGTH;

/* AES-GCM stuff (http://tools.ietf.org/html/rfc7714) */
const SRTP_AEAD_AES_128_GCM: c_ulong = 0x0007;
const SRTP_AESGCM128_MASTER_KEY_LENGTH: usize = 16;
const SRTP_AESGCM128_MASTER_SALT_LENGTH: usize = 12;
const SRTP_AESGCM128_MASTER_LENGTH: usize = SRTP_AESGCM128_MASTER_KEY_LENGTH + SRTP_AESGCM128_MASTER_SALT_LENGTH;

const SRTP_AEAD_AES_256_GCM: c_ulong = 0x0008;
const SRTP_AESGCM256_MASTER_KEY_LENGTH: usize = 32;
const SRTP_AESGCM256_MASTER_SALT_LENGTH: usize = 12;
const SRTP_AESGCM256_MASTER_LENGTH: usize = SRTP_AESGCM256_MASTER_KEY_LENGTH + SRTP_AESGCM256_MASTER_SALT_LENGTH;

/// A wrapper around the Srtp2 error codes
#[derive(Debug, Copy, Clone, PartialOrd, PartialEq)]
pub enum Srtp2ErrorCode {
    Ok,
    Fail,
    BadParam,
    AllocFail,
    DeallocFail,
    InitFail,
    Terminus,
    AuthFail,
    CipherFail,
    ReplayFail,
    ReplayOld,
    AlgoFail,
    NoSuchOp,
    NoCtx,
    CantCheck,
    KeyExpired,
    SocketErr,
    SignalErr,
    NonceBad,
    ReadFail,
    WriteFail,
    ParseErr,
    EncodeErr,
    SemaphoreErr,
    PfKeyErr,
    BadMki,
    PktIdxOld,
    PktIdxAdv
}

impl Srtp2ErrorCode {
    fn from(code: ffi::srtp_err_status_t) -> Srtp2ErrorCode {
        match code {
            ffi::srtp_err_status_t_srtp_err_status_ok => Srtp2ErrorCode::Ok,
            ffi::srtp_err_status_t_srtp_err_status_fail => Srtp2ErrorCode::Fail,
            ffi::srtp_err_status_t_srtp_err_status_bad_param => Srtp2ErrorCode::BadParam,
            ffi::srtp_err_status_t_srtp_err_status_alloc_fail => Srtp2ErrorCode::AllocFail,
            ffi::srtp_err_status_t_srtp_err_status_dealloc_fail => Srtp2ErrorCode::DeallocFail,
            ffi::srtp_err_status_t_srtp_err_status_init_fail => Srtp2ErrorCode::InitFail,
            ffi::srtp_err_status_t_srtp_err_status_terminus => Srtp2ErrorCode::Terminus,
            ffi::srtp_err_status_t_srtp_err_status_auth_fail => Srtp2ErrorCode::AuthFail,
            ffi::srtp_err_status_t_srtp_err_status_cipher_fail => Srtp2ErrorCode::CipherFail,
            ffi::srtp_err_status_t_srtp_err_status_replay_fail => Srtp2ErrorCode::ReplayFail,
            ffi::srtp_err_status_t_srtp_err_status_replay_old => Srtp2ErrorCode::ReplayOld,
            ffi::srtp_err_status_t_srtp_err_status_algo_fail => Srtp2ErrorCode::AlgoFail,
            ffi::srtp_err_status_t_srtp_err_status_no_such_op => Srtp2ErrorCode::NoSuchOp,
            ffi::srtp_err_status_t_srtp_err_status_no_ctx => Srtp2ErrorCode::NoCtx,
            ffi::srtp_err_status_t_srtp_err_status_cant_check => Srtp2ErrorCode::CantCheck,
            ffi::srtp_err_status_t_srtp_err_status_key_expired => Srtp2ErrorCode::KeyExpired,
            ffi::srtp_err_status_t_srtp_err_status_socket_err => Srtp2ErrorCode::SocketErr,
            ffi::srtp_err_status_t_srtp_err_status_signal_err => Srtp2ErrorCode::SignalErr,
            ffi::srtp_err_status_t_srtp_err_status_nonce_bad => Srtp2ErrorCode::NonceBad,
            ffi::srtp_err_status_t_srtp_err_status_read_fail => Srtp2ErrorCode::ReadFail,
            ffi::srtp_err_status_t_srtp_err_status_write_fail => Srtp2ErrorCode::WriteFail,
            ffi::srtp_err_status_t_srtp_err_status_parse_err => Srtp2ErrorCode::ParseErr,
            ffi::srtp_err_status_t_srtp_err_status_encode_err => Srtp2ErrorCode::EncodeErr,
            ffi::srtp_err_status_t_srtp_err_status_semaphore_err => Srtp2ErrorCode::SemaphoreErr,
            ffi::srtp_err_status_t_srtp_err_status_pfkey_err => Srtp2ErrorCode::PfKeyErr,
            ffi::srtp_err_status_t_srtp_err_status_bad_mki => Srtp2ErrorCode::BadMki,
            ffi::srtp_err_status_t_srtp_err_status_pkt_idx_old => Srtp2ErrorCode::PktIdxOld,
            ffi::srtp_err_status_t_srtp_err_status_pkt_idx_adv => Srtp2ErrorCode::PktIdxAdv,
            _ => Srtp2ErrorCode::Fail
        }
    }

    /// Returns `true` is the error is `Srtp2ErrorCode::Ok`
    pub fn success(&self) -> Result<(), Self> {
        if self == &Srtp2ErrorCode::Ok {
            Ok(())
        } else {
            Err(*self)
        }
    }
}

/// The reason why creating a Srtp2 wrapper failed to be created from an openssl ssl context.
#[derive(Debug)]
pub enum OpenSSLImportError {
    SrtpError(Srtp2ErrorCode),
    MissingSrtpProfile,
    SrtpProfileNotSupported,
    KeyMaterialExportFailed(ErrorStack)
}

/// A wrapper around two srtp instances, used for en/decoding rt(c)p packets.
pub struct Srtp2 {
    state: ffi::srtp_t
}

/// Globally initialize Srtp.
/// This function *must* be called before any calls to the srtp library happen
pub fn srtp2_global_init() -> Result<(), Srtp2ErrorCode> {
    Srtp2ErrorCode::from(unsafe {
        ffi::srtp_init()
    }).success()
}

impl Srtp2 {
    /// Create a new Srtp2 wrapper using the incoming and outgoing srtp policies.
    pub fn new(policy: &ffi::srtp_policy_t) -> Result<Self, Srtp2ErrorCode> {
        let mut state = unsafe { mem::zeroed::<ffi::srtp_t>() };
        Srtp2ErrorCode::from(unsafe { ffi::srtp_create(&mut state, policy) }).success()?;

        Ok(Srtp2 {
            state
        })
    }

    /// Unprotect a rtp packet.
    /// The return value will hold the packet length in bytes.
    /// The buffer must be 32 bit aligned (else it's an invalid packet).
    pub fn unprotect(&self, buffer: &mut [u8]) -> Result<usize, Srtp2ErrorCode> {
        let mut length = buffer.len() as c_int;
        Srtp2ErrorCode::from(unsafe {
            ffi::srtp_unprotect(self.state, buffer.as_mut_ptr() as *mut c_void, &mut length)
        }).success()?;

        assert!(length >= 0);
        Ok(length as usize)
    }


    /// Unprotect a rtcp packet.
    /// The return value will hold the packet length in bytes.
    /// The buffer must be 32 bit aligned (else it's an invalid packet).
    pub fn unprotect_rtcp(&self, buffer: &mut [u8]) -> Result<usize, Srtp2ErrorCode> {
        let mut length = buffer.len() as c_int;
        Srtp2ErrorCode::from(unsafe {
            ffi::srtp_unprotect_rtcp(self.state, buffer.as_mut_ptr() as *mut c_void, &mut length)
        }).success()?;

        //length *= 4;
        assert!(length >= 0);
        Ok(length as usize)
    }

    /// Protect a rtp packet.
    /// Be aware, that srtp may add some bytes.
    /// Ensure your buffer has at least 148 bytes of extra space!
    /// You must also ensure that the buffer is 32 bit aligned!
    pub fn protect(&self, buffer: &mut [u8], payload_length: usize) -> Result<usize, Srtp2ErrorCode> {
        let mut length = payload_length as i32;
        Srtp2ErrorCode::from(unsafe {
            ffi::srtp_protect(self.state, buffer.as_mut_ptr() as *mut c_void, &mut length)
        }).success()?;

        assert!(length >= 0 && (length as usize) < buffer.len());
        Ok(length as usize)
    }

    /// Protect a rtcp packet.
    /// Be aware, that srtp may add some bytes.
    /// Ensure your buffer has at least 148 bytes of extra space!
    /// You must also ensure that the buffer is 32 bit aligned!
    pub fn protect_rtcp(&self, buffer: &mut [u8], payload_length: usize) -> Result<usize, Srtp2ErrorCode> {
        let mut length = payload_length as i32;
        Srtp2ErrorCode::from(unsafe {
            ffi::srtp_protect_rtcp(self.state, buffer.as_mut_ptr() as *mut c_void, &mut length)
        }).success()?;

        assert!(length >= 0 && (length as usize) < buffer.len());
        Ok(length as usize)
    }
}

impl Srtp2 {
    /// Create a Srtp2 wrapper based on the negotiated key from the Ssl handshake.
    /// `tlsext_use_srtp` has to be enabled, supporting
    /// - SRTP_AES128_CM_SHA1_80
    /// - SRTP_AES128_CM_SHA1_32
    /// Any other modes are not supported and will result in an
    /// `OpenSSLImportError::SrtpProfileNotSupported` error.
    ///
    /// It returns a tuple containing Srtp2 instances.
    /// The first one is the remote (use for receiving packets) and the second one the local instance (use for sending packets)
    pub fn from_openssl(context: &openssl::ssl::SslRef) -> Result<(Self, Self), OpenSSLImportError> {
        let profile = context.selected_srtp_profile();
        if profile.is_none() {
            return Err(OpenSSLImportError::MissingSrtpProfile);
        }
        let profile = profile.unwrap();

        let mut local_policy = unsafe { mem::zeroed::<ffi::srtp_policy_t>() };
        local_policy.ssrc.type_ = ffi::srtp_ssrc_type_t_ssrc_any_outbound;
        local_policy.window_size = 128;
        local_policy.allow_repeat_tx = 0;

        let mut remote_policy = unsafe { mem::zeroed::<ffi::srtp_policy_t>() };
        remote_policy.ssrc.type_ = ffi::srtp_ssrc_type_t_ssrc_any_inbound;
        remote_policy.window_size = 128;
        remote_policy.allow_repeat_tx = 0;

        let (key_length, salt_length, master_length) = {
            match (profile.id(), profile.id().as_raw()) {
                (SrtpProfileId::SRTP_AES128_CM_SHA1_80, _) => {
                    unsafe {
                        ffi::srtp_crypto_policy_set_rtp_default(&mut local_policy.rtp);
                        ffi::srtp_crypto_policy_set_rtp_default(&mut local_policy.rtcp);

                        ffi::srtp_crypto_policy_set_rtp_default(&mut remote_policy.rtp);
                        ffi::srtp_crypto_policy_set_rtp_default(&mut remote_policy.rtcp);
                    }
                    Ok((SRTP_MASTER_KEY_LENGTH, SRTP_MASTER_SALT_LENGTH, SRTP_MASTER_LENGTH))
                },
                (SrtpProfileId::SRTP_AES128_CM_SHA1_32, _) => {
                    unsafe {
                        // RTP HMAC is shortened to 32 bits, but RTCP remains 80 bits.
                        ffi::srtp_crypto_policy_set_aes_cm_128_hmac_sha1_32(&mut local_policy.rtp);
                        ffi::srtp_crypto_policy_set_rtp_default(&mut local_policy.rtcp);

                        ffi::srtp_crypto_policy_set_aes_cm_128_hmac_sha1_32(&mut remote_policy.rtp);
                        ffi::srtp_crypto_policy_set_rtp_default(&mut remote_policy.rtcp);
                    }

                    Ok((SRTP_MASTER_KEY_LENGTH, SRTP_MASTER_SALT_LENGTH, SRTP_MASTER_LENGTH))
                },
                #[cfg(target_os = "windows")]
                (_, SRTP_AEAD_AES_128_GCM) => {
                    unsafe {
                        ffi::srtp_crypto_policy_set_aes_gcm_128_16_auth(&mut local_policy.rtp);
                        ffi::srtp_crypto_policy_set_aes_gcm_128_16_auth(&mut local_policy.rtcp);

                        ffi::srtp_crypto_policy_set_aes_gcm_128_16_auth(&mut remote_policy.rtp);
                        ffi::srtp_crypto_policy_set_aes_gcm_128_16_auth(&mut remote_policy.rtcp);
                    }

                    Ok((SRTP_AESGCM128_MASTER_KEY_LENGTH, SRTP_AESGCM128_MASTER_SALT_LENGTH, SRTP_AESGCM128_MASTER_LENGTH))
                },
                #[cfg(target_os = "windows")]
                (_, SRTP_AEAD_AES_256_GCM) => {
                    unsafe {
                        ffi::srtp_crypto_policy_set_aes_gcm_256_16_auth(&mut local_policy.rtp);
                        ffi::srtp_crypto_policy_set_aes_gcm_256_16_auth(&mut local_policy.rtcp);

                        ffi::srtp_crypto_policy_set_aes_gcm_256_16_auth(&mut remote_policy.rtp);
                        ffi::srtp_crypto_policy_set_aes_gcm_256_16_auth(&mut remote_policy.rtcp);
                    }

                    Ok((SRTP_AESGCM256_MASTER_KEY_LENGTH, SRTP_AESGCM256_MASTER_SALT_LENGTH, SRTP_AESGCM256_MASTER_LENGTH))
                },
                _ => Err(OpenSSLImportError::SrtpProfileNotSupported)
            }
        }?;
        let mut key_buffer = [0u8; 256];
        assert!(master_length * 2 <= key_buffer.len());

        if let Err(error) = context.export_keying_material(&mut key_buffer, "EXTRACTOR-dtls_srtp", None) {
            return Err(OpenSSLImportError::KeyMaterialExportFailed(error));
        }

        let mut key_salt_a = [0u8; 128];
        let mut key_salt_b = [0u8; 128];
        assert!(master_length <= key_salt_a.len());
        assert!(master_length <= key_salt_b.len());

        let mut key_buffer_offset = 0;

        key_salt_a[0..key_length].copy_from_slice(&key_buffer[key_buffer_offset..(key_buffer_offset + key_length)]);
        key_buffer_offset += key_length;

        key_salt_b[0..key_length].copy_from_slice(&key_buffer[key_buffer_offset..(key_buffer_offset + key_length)]);
        key_buffer_offset += key_length;

        key_salt_a[key_length..(key_length + salt_length)].copy_from_slice(&key_buffer[key_buffer_offset..(key_buffer_offset + salt_length)]);
        key_buffer_offset += salt_length;

        key_salt_b[key_length..(key_length + salt_length)].copy_from_slice(&key_buffer[key_buffer_offset..(key_buffer_offset + salt_length)]);

        if context.is_server() {
            remote_policy.key = key_salt_a.as_mut_ptr();
            local_policy.key = key_salt_b.as_mut_ptr();
        } else {
            local_policy.key = key_salt_a.as_mut_ptr();
            remote_policy.key = key_salt_b.as_mut_ptr();
        }

        let remote_result = Srtp2::new(&remote_policy)
            .map_err(|err| OpenSSLImportError::SrtpError(err))?;

        let local_result = Srtp2::new(&local_policy)
            .map_err(|err| OpenSSLImportError::SrtpError(err))?;

        Ok((remote_result, local_result))
    }
}

impl Drop for Srtp2 {
    fn drop(&mut self) {
        let _ = Srtp2ErrorCode::from(unsafe { ffi::srtp_dealloc(self.state) }).success()
            .map_err(|error| slog::slog_error!(global_logger(), "failed to deallocate srtp in state: {:?}", error));
    }
}