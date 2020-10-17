use crate::utils::rtcp::{RtcpReportBlock, read_profile_data, profile_data_length, RtcpPacketType, write_profile_data};
use std::io::{Cursor, Result, ErrorKind, Error};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use std::collections::BTreeMap;

/// https://tools.ietf.org/html/rfc1889#section-6.3.1
#[derive(Debug, Clone)]
pub struct RtcpPacketSenderReport {
    pub ssrc: u32,
    pub network_timestamp: u64,
    pub rtp_timestamp: u32,
    pub packet_count: u32,
    pub byte_count: u32,

    pub reports: BTreeMap<u32, RtcpReportBlock>,
    pub profile_data: Option<Vec<u8>>
}

impl RtcpPacketSenderReport {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<RtcpPacketSenderReport> {
        let info = reader.read_u8()?;
        if (info >> 6) != 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid version, expected 2"));
        }

        if reader.read_u8()? != RtcpPacketType::SenderReport.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "rtcp packet isn't a sender report"));
        }

        let length = reader.read_u16::<BigEndian>()? as usize * 4 + 4;
        let sender_src = reader.read_u32::<BigEndian>()?;
        let network_timestamp = reader.read_u64::<BigEndian>()?;
        let rtp_timestamp = reader.read_u32::<BigEndian>()?;
        let packet_count = reader.read_u32::<BigEndian>()?;
        let byte_count = reader.read_u32::<BigEndian>()?;
        let mut report = RtcpPacketSenderReport{
            ssrc: sender_src,
            network_timestamp,
            rtp_timestamp,
            packet_count,
            byte_count,
            reports: BTreeMap::new(),
            profile_data: None
        };

        for _ in 0..(info & 0x3F) as usize {
            let ssrc = reader.read_u32::<BigEndian>()?;
            report.reports.insert(ssrc, RtcpReportBlock::parse(reader)?);
        }

        let bytes_read = report.byte_size();
        if length < bytes_read {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid packet length"));
        }
        report.profile_data = read_profile_data(reader, length - bytes_read, (info & 0x40) != 0)?;

        Ok(report)
    }

    pub fn byte_size(&self) -> usize {
        7 * 4 + self.reports.iter().map(|e| 4 + e.1.byte_size()).sum::<usize>() + profile_data_length(&self.profile_data)
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        if self.reports.len() > 15 {
            return Err(Error::new(ErrorKind::InvalidData, "can't write more than 15 report blocks"));
        }

        let mut info = 2 << 6;
        if let Some(payload) = &self.profile_data {
            if (payload.len() % 4) != 0 {
                info |= 1 << 5; /* we've to do some padding */
            }
        }
        info |= self.reports.len();
        writer.write_u8(info as u8)?;
        writer.write_u8(RtcpPacketType::SenderReport.value())?;

        let mut quad_size = 6 + self.reports.iter().map(|e| 1 + e.1.quad_byte_size()).sum::<usize>();
        if let Some(payload) = &self.profile_data {
            quad_size += (payload.len() + 3) / 4;
        }
        writer.write_u16::<BigEndian>(quad_size as u16)?;

        writer.write_u32::<BigEndian>(self.ssrc)?;
        writer.write_u64::<BigEndian>(self.network_timestamp)?;
        writer.write_u32::<BigEndian>(self.rtp_timestamp)?;
        writer.write_u32::<BigEndian>(self.packet_count)?;
        writer.write_u32::<BigEndian>(self.byte_count)?;

        for report in self.reports.iter() {
            writer.write_u32::<BigEndian>(*report.0)?;
            report.1.write(writer)?;
        }

        write_profile_data(writer, &self.profile_data, true)?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    // Most tests are taken from https://source.chromium.org/chromium/chromium/src/+/master:third_party/webrtc/modules/rtp_rtcp/source/rtcp_packet/sender_report_unittest.cc

    use crate::utils::rtcp::packets::RtcpPacketSenderReport;
    use std::io::Cursor;
    use std::collections::BTreeMap;

    const SENDER_SSRC: u32 = 0x12345678;
    const REMOTE_SSRC: u32 = 0x23456789;

    const NETWORK_TIMESTAMP: u64 = 0x1112141822242628;
    const RTP_TIMESTAMP: u32 = 0x33343536;

    const PACKET_COUNT: u32 = 0x44454647;
    const BYTE_COUNT: u32 = 0x55565758;

    const PACKET: [u8; 28] = [0x80, 200,  0x00, 0x06, 0x12, 0x34, 0x56,
                              0x78, 0x11, 0x12, 0x14, 0x18, 0x22, 0x24,
                              0x26, 0x28, 0x33, 0x34, 0x35, 0x36, 0x44,
                              0x45, 0x46, 0x47, 0x55, 0x56, 0x57, 0x58];

    fn do_build(packet: &RtcpPacketSenderReport) -> Vec<u8> {
        let mut buffer = [0u8; 1024];
        let mut cursor = Cursor::new(&mut buffer[..]);
        packet.write(&mut cursor).expect("failed to build packet");
        let written = cursor.position() as usize;
        buffer[0..written].to_vec()
    }

    fn create_basic_sr() ->  RtcpPacketSenderReport {
        RtcpPacketSenderReport{
            ssrc: SENDER_SSRC,
            packet_count: PACKET_COUNT,
            byte_count: BYTE_COUNT,
            rtp_timestamp: RTP_TIMESTAMP,
            network_timestamp: NETWORK_TIMESTAMP,
            profile_data: None,
            reports: BTreeMap::new()
        }
    }

    #[test]
    fn test_create_without_blocks() {
        let result = do_build(&create_basic_sr());
        assert_eq!(result, PACKET);
    }

    #[test]
    fn test_parse_without_blocks() {
        let packet = RtcpPacketSenderReport::parse(&mut Cursor::new(&PACKET[..]))
            .expect("failed to parse packet");
        assert_eq!(packet.ssrc, SENDER_SSRC);
        assert_eq!(packet.packet_count, PACKET_COUNT);
        assert_eq!(packet.byte_count, BYTE_COUNT);
        assert_eq!(packet.rtp_timestamp, RTP_TIMESTAMP);
        assert_eq!(packet.network_timestamp, NETWORK_TIMESTAMP);
        assert_eq!(packet.profile_data, None);
        assert!(packet.reports.is_empty());
    }
}