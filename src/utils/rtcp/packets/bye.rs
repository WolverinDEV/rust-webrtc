use std::io::{Cursor, Result, Error, ErrorKind, Read, Write};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use crate::utils::rtcp::RtcpPacketType;

/// https://tools.ietf.org/html/rfc3550#section-6.6
#[derive(Debug, Clone)]
pub struct RtcpPacketBye {
    pub src: Vec<u32>,
    pub reason: Option<String>
}

impl RtcpPacketBye {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<RtcpPacketBye> {
        let info = reader.read_u8()?;
        if (info >> 6) != 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid version, expected 2"));
        }

        let payload_type = reader.read_u8()?;
        if payload_type != RtcpPacketType::PayloadFeedback.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "rtcp packet isn't a feedback packet"));
        }
        let _ = reader.read_u16::<BigEndian>()?; /* the total packet length is not from interest */

        let src_count = (info & 0x1F) as usize;
        let mut src: Vec<u32> = Vec::with_capacity(src_count);

        for _ in 0..src_count {
            src.push(reader.read_u32::<BigEndian>()?);
        }

        let mut reason = None;
        if let Ok(mut length) = reader.read_u8() {
            let mut buffer = Vec::new();
            buffer.resize(length as usize, 0);
            reader.read_exact(&mut buffer)?;
            reason = Some(String::from_utf8(buffer).map_err(|_| Error::new(ErrorKind::InvalidInput, "reason is not UTF-8"))?);

            while (length & 3) != 0 {
                reader.read_u8()?;
                length = length + 1;
            }
        }

        Ok(RtcpPacketBye{ src, reason })
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        if self.src.len() > 15 {
            return Err(Error::new(ErrorKind::InvalidData, "too many srcs"));
        }

        let mut info = 2 << 6;
        info |= self.src.len();

        let reason_length = if let Some(reason) = self.reason.as_ref() { (reason.len() + 3) / 4 } else { 0 };
        writer.write_u8(info as u8)?;
        writer.write_u8(RtcpPacketType::Bye.value())?;
        writer.write_u16::<BigEndian>((1 + self.src.len() + reason_length) as u16)?;

        for src in self.src.iter() {
            writer.write_u32::<BigEndian>(*src)?;
        }

        if let Some(reason) = self.reason.as_ref() {
            if reason.len() > 255 {
                return Err(Error::new(ErrorKind::InvalidData, "reason too long"));
            }
            writer.write_u8(reason.len() as u8)?;
            writer.write_all(reason.as_bytes())?;

            let mut bytes = reason.len();
            while (bytes & 3) != 0 {
                writer.write_u8(0)?;
                bytes = bytes + 1;
            }
        }

        Ok(())
    }
}