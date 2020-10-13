use std::pin::Pin;
use rtp_rs::RtpReaderError;
use std::ops::Deref;
use std::fmt::Debug;
use serde::export::Formatter;

/// Returns true if the buffer seem to contain a RTP header
pub fn is_rtp_header(buffer: &[u8]) -> bool {
    /* https://tools.ietf.org/html/rfc5761#section-4 */
    if buffer.len() < rtp_rs::RtpReader::MIN_HEADER_LEN {
        false
    } else {
        let payload_type = buffer[1] & 0b0111_1111;
        payload_type < 64 || payload_type >= 96
    }
}

/// A wrapper around a rtp packet, owning the packet buffer and holding an instance of
/// `rtp_rs::RtpReader` which has parsed this packet.
pub struct ParsedRtpPacket {
    /* Attention: Mutating the buffer content does change the package validity; So DON'T do it! */
    buffer: Pin<Vec<u8>>,
    pub parser: rtp_rs::RtpReader<'static>
}

impl ParsedRtpPacket {
    /// Parse and create a new `ParsedRtpPacket`.
    /// If it fails to parse the packet, the original packet along with the error will be returned.
    pub fn new(buffer: Vec<u8>) -> Result<Self, (RtpReaderError, Vec<u8>)> {
        let slice = unsafe {
            std::slice::from_raw_parts(buffer.as_ptr(), buffer.len())
        };

        let reader = rtp_rs::RtpReader::new(slice);
        if let Err(err) = reader {
            return Err((err, buffer));
        }

        Ok(ParsedRtpPacket {
            buffer: Pin::new(buffer),
            parser: reader.unwrap()
        })
    }

    /// Get the underlying buffer for the packet
    pub fn into_raw_buffer(self) -> Vec<u8> {
        Pin::<Vec<u8>>::into_inner(self.buffer)
    }
}

impl Deref for ParsedRtpPacket {
    type Target = rtp_rs::RtpReader<'static>;

    fn deref(&self) -> &Self::Target {
        &self.parser
    }
}

impl Debug for ParsedRtpPacket {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.parser.fmt(f)
    }
}