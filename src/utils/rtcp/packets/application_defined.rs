use std::io::{Cursor, Result, Error, ErrorKind, Read, Write};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use crate::utils::rtcp::RtcpPacketType;

/// https://tools.ietf.org/html/rfc3550#section-6.7
#[derive(Debug, Clone)]
pub struct RtcpPacketApplicationDefined {
    pub subtype: u8,
    pub src: u32,
    pub name: [u8; 4],
    pub data: Vec<u8>
}

impl RtcpPacketApplicationDefined {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<RtcpPacketApplicationDefined> {
        let info = reader.read_u8()?;
        if (info >> 6) != 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid version, expected 2"));
        }

        let payload_type = reader.read_u8()?;
        if payload_type != RtcpPacketType::ApplicationDefined.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "rtcp packet isn't a application defined packet"));
        }
        let length = reader.read_u16::<BigEndian>()?;
        if length < 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid length"));
        }

        let subtype = info & 0x1F;
        let src = reader.read_u32::<BigEndian>()?;
        let mut name = [0u8; 4];
        reader.read_exact(&mut name)?;

        let mut data = Vec::new();
        data.resize((length - 2) as usize * 4, 0);
        reader.read_exact(data.as_mut_slice())?;

        Ok(RtcpPacketApplicationDefined {
            subtype,
            src,
            name,
            data
        })
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        if (self.data.len() & 3) > 0 {
            return Err(Error::new(ErrorKind::InvalidData, "application data length must be a multiple of four"));
        }

        let mut info = 2 << 6;
        info |= self.subtype;

        writer.write_u8(info as u8)?;
        writer.write_u8(RtcpPacketType::ApplicationDefined.value())?;
        writer.write_u16::<BigEndian>((2 + self.data.len() / 4) as u16)?;

        writer.write_u32::<BigEndian>(self.src)?;
        writer.write_all(&self.name)?;
        writer.write_all(self.data.as_slice())?;

        Ok(())
    }
}