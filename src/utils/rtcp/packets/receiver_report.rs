use crate::utils::rtcp::{RtcpReportBlock, read_profile_data, profile_data_length, RtcpPacketType, write_profile_data};
use std::io::{Cursor, Result, ErrorKind, Error};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use std::collections::BTreeMap;

///https://tools.ietf.org/html/rfc1889#section-6.3.2
#[derive(Debug, Clone)]
pub struct RtcpPacketReceiverReport {
    pub ssrc: u32,
    pub reports: BTreeMap<u32, RtcpReportBlock>,
    pub profile_data: Option<Vec<u8>>
}

impl RtcpPacketReceiverReport {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<RtcpPacketReceiverReport> {
        let info = reader.read_u8()?;
        if (info >> 6) != 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid version, expected 2"));
        }

        if reader.read_u8()? != RtcpPacketType::ReceiverReport.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "rtcp packet isn't a sender report"));
        }

        let length = reader.read_u16::<BigEndian>()? as usize * 4 + 4;
        let sender_src = reader.read_u32::<BigEndian>()?;

        let mut report = RtcpPacketReceiverReport{
            ssrc: sender_src,
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
        8 + self.reports.iter().map(|e| e.1.byte_size() + 4).sum::<usize>() + profile_data_length(&self.profile_data)
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
        writer.write_u8(RtcpPacketType::ReceiverReport.value())?;

        let mut quad_size = 1 + self.reports.iter().map(|e| 1 + e.1.quad_byte_size()).sum::<usize>();
        if let Some(payload) = &self.profile_data {
            quad_size += (payload.len() + 3) / 4;
        }
        writer.write_u16::<BigEndian>(quad_size as u16)?;

        writer.write_u32::<BigEndian>(self.ssrc)?;
        for report in self.reports.iter() {
            writer.write_u32::<BigEndian>(*report.0)?;
            report.1.write(writer)?;
        }

        write_profile_data(writer, &self.profile_data, true)?;
        Ok(())
    }
}