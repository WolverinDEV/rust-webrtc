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