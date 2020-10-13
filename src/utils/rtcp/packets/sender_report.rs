use crate::utils::rtcp::{RtcpReportBlock, read_profile_data, profile_data_length, RtcpPacketType, write_profile_data};
use std::io::{Cursor, Result, ErrorKind, Error};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};

/// https://tools.ietf.org/html/rfc1889#section-6.3.1
#[derive(Debug, Clone)]
pub struct RtcpPacketSenderReport {
    pub ssrc: u32,
    pub network_timestamp: u64,
    pub rtp_timestamp: u32,
    pub packet_count: u32,
    pub byte_count: u32,

    pub reports: Vec<RtcpReportBlock>,
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
            reports: Vec::new(),
            profile_data: None
        };
        report.reports.reserve((info & 0x3F) as usize);
        for _ in 0..(info & 0x3F) as usize {
            report.reports.push(RtcpReportBlock::parse(reader)?);
        }

        let bytes_read = report.byte_size();
        if length < bytes_read {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid packet length"));
        }
        report.profile_data = read_profile_data(reader, length - bytes_read, (info & 0x40) != 0)?;

        Ok(report)
    }

    pub fn byte_size(&self) -> usize {
        7 * 4 + self.reports.iter().map(|e| e.byte_size()).sum::<usize>() + profile_data_length(&self.profile_data)
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

        let mut quad_size = 6 + self.reports.iter().map(|e| e.quad_byte_size()).sum::<usize>();
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
            report.write(writer)?;
        }

        write_profile_data(writer, &self.profile_data, true)?;
        Ok(())
    }
}