use crate::utils::rtcp::{RtcpReportBlock, read_profile_data, profile_data_length, RtcpPacketType, write_profile_data};
use std::io::{Cursor, Result, ErrorKind, Error, Read, Write};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};

/// Source Description RTCP Packet
/// https://tools.ietf.org/html/rfc3550#section-6.5
///
///         0                   1                   2                   3
///         0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
///        +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// header |V=2|P|    SC   |  PT=SDES=202  |             length            |
///        +=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+
/// chunk  |                          SSRC/CSRC_1                          |
///   1    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///        |                           SDES items                          |
///        |                              ...                              |
///        +=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+
/// chunk  |                          SSRC/CSRC_2                          |
///   2    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///        |                           SDES items                          |
///        |                              ...                              |
///        +=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+

#[derive(Clone, Debug, PartialEq)]
pub enum SourceDescriptionType {
    CName,
    Name,
    Email,
    Phone,
    Location,
    Tool,
    Note,
    Private
}

impl SourceDescriptionType {
    pub fn from(ptype: u8) -> Option<SourceDescriptionType> {
        match ptype {
            1 => Some(SourceDescriptionType::CName),
            2 => Some(SourceDescriptionType::Name),
            3 => Some(SourceDescriptionType::Email),
            4 => Some(SourceDescriptionType::Phone),
            5 => Some(SourceDescriptionType::Location),
            6 => Some(SourceDescriptionType::Tool),
            7 => Some(SourceDescriptionType::Note),
            8 => Some(SourceDescriptionType::Private),
            _ => None
        }
    }

    pub fn value(&self) -> u8 {
        match self {
            SourceDescriptionType::CName => 1,
            SourceDescriptionType::Name => 2,
            SourceDescriptionType::Email => 3,
            SourceDescriptionType::Phone => 4,
            SourceDescriptionType::Location => 5,
            SourceDescriptionType::Tool => 6,
            SourceDescriptionType::Note => 7,
            SourceDescriptionType::Private => 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SourceDescription {
    /// https://tools.ietf.org/html/rfc3550#section-6.5.1
    CName(String),

    /// https://tools.ietf.org/html/rfc3550#section-6.5.2
    Name(String),

    /// https://tools.ietf.org/html/rfc3550#section-6.5.3
    Email(String),

    /// https://tools.ietf.org/html/rfc3550#section-6.5.4
    Phone(String),

    /// https://tools.ietf.org/html/rfc3550#section-6.5.5
    Location(String),

    /// https://tools.ietf.org/html/rfc3550#section-6.5.6
    Tool(String),

    /// https://tools.ietf.org/html/rfc3550#section-6.5.7
    Note(String),

    /// https://tools.ietf.org/html/rfc3550#section-6.5.7
    Private{ prefix: String, value: String },
}

impl SourceDescription {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<SourceDescription> {
        let description_type = reader.read_u8()?;
        let total_length = reader.read_u8()? as usize;

        let mut str_buffer = Vec::<u8>::new();
        str_buffer.resize(total_length, 0);
        reader.read_exact(str_buffer.as_mut_slice())?;
        let value = String::from_utf8(str_buffer)
            .map_err(|err| Error::new(ErrorKind::InvalidInput, "value isn't UTF-8 encoded"))?;

        let description_type = SourceDescriptionType::from(description_type);
        if description_type.is_none() {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid description type"));
        }

        Ok(match description_type.unwrap() {
            SourceDescriptionType::CName => SourceDescription::CName(value),
            SourceDescriptionType::Name => SourceDescription::Name(value),
            SourceDescriptionType::Email => SourceDescription::Email(value),
            SourceDescriptionType::Phone => SourceDescription::Phone(value),
            SourceDescriptionType::Location => SourceDescription::Location(value),
            SourceDescriptionType::Tool => SourceDescription::Tool(value),
            SourceDescriptionType::Note => SourceDescription::Note(value),
            SourceDescriptionType::Private => {
                /* FIXME! */
                SourceDescription::Private { value: String::new(), prefix: String::new() }
            }
        })
    }

    pub fn byte_size(&self) -> usize {
        2 + match self {
            SourceDescription::CName(..) |
            SourceDescription::Name(..) |
            SourceDescription::Email(..) |
            SourceDescription::Phone(..) |
            SourceDescription::Location(..) |
            SourceDescription::Tool(..) |
            SourceDescription::Note(..) => self.description_value().unwrap().len(),
            SourceDescription::Private { prefix, value } => 1 + prefix.len() + value.len()
        }
    }

    pub fn description_type(&self) -> SourceDescriptionType {
        match self {
            SourceDescription::CName(..) => SourceDescriptionType::CName,
            SourceDescription::Name(..) => SourceDescriptionType::Name,
            SourceDescription::Email(..) => SourceDescriptionType::Email,
            SourceDescription::Phone(..) => SourceDescriptionType::Phone,
            SourceDescription::Location(..) => SourceDescriptionType::Location,
            SourceDescription::Tool(..) => SourceDescriptionType::Tool,
            SourceDescription::Note(..) => SourceDescriptionType::Note,
            SourceDescription::Private{ .. } => SourceDescriptionType::Private,
        }
    }

    fn description_value(&self) -> Option<&String> {
        match self {
            SourceDescription::CName(value) => Some(value),
            SourceDescription::Name(value) => Some(value),
            SourceDescription::Email(value) => Some(value),
            SourceDescription::Phone(value) => Some(value),
            SourceDescription::Location(value) => Some(value),
            SourceDescription::Tool(value) => Some(value),
            SourceDescription::Note(value) => Some(value),
            SourceDescription::Private { .. } => None
        }
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        writer.write_u8(self.description_type().value())?;

        let bytes_written = match self {
            SourceDescription::CName(..) |
            SourceDescription::Name(..) |
            SourceDescription::Email(..) |
            SourceDescription::Phone(..) |
            SourceDescription::Location(..) |
            SourceDescription::Tool(..) |
            SourceDescription::Note(..) => {
                let value = self.description_value().unwrap();
                if value.len() > 255 {
                    return Err(Error::new(ErrorKind::InvalidData, "description value can not be larger than 255"));
                }
                writer.write_u8(value.len() as u8)?;
                writer.write_all(value.as_bytes())?;
                value.len() + 2
            },
            SourceDescription::Private { prefix, value } => {
                let total_length = 1 + value.len() + prefix.len();
                if total_length > 255 {
                    return Err(Error::new(ErrorKind::InvalidData, "total length (prefix + value + 1) can not be larger than 255"));
                }
                writer.write_u8(total_length as u8)?;
                writer.write_u8(prefix.len() as u8)?;
                writer.write_all(prefix.as_bytes())?;
                writer.write_all(value.as_bytes())?;
                total_length + 1
            }
        };

        /* manual padding, faster than calculating anything */
        match bytes_written % 4 {
            0 => {},
            1 => {
                writer.write_u8(0)?;
                writer.write_u8(0)?;
                writer.write_u8(0)?; /* Don't confuse with the general RTCP padding. We don't have to put in the amount of padded bytes! */
            },
            2 => {
                writer.write_u8(0)?;
                writer.write_u8(0)?; /* Don't confuse with the general RTCP padding. We don't have to put in the amount of padded bytes! */
            },
            3 => {
                writer.write_u8(0)?; /* Don't confuse with the general RTCP padding. We don't have to put in the amount of padded bytes! */
            },
            _ => panic!()
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct RtcpPacketSourceDescription {
    descriptions: Vec<(u32, SourceDescription)>
}

impl RtcpPacketSourceDescription {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<RtcpPacketSourceDescription> {
        let info = reader.read_u8()?;
        if (info >> 6) != 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid version, expected 2"));
        }

        if reader.read_u8()? != RtcpPacketType::SourceDescription.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "rtcp packet isn't a source description"));
        }

        let source_count = (info & 0x3F) as usize;
        let _ = reader.read_u16::<BigEndian>()?; /* the total packet length */

        let mut descriptions = Vec::with_capacity(source_count);
        for _ in 0..source_count {
            let ssrc = reader.read_u32::<BigEndian>()?;
            descriptions.push((ssrc, SourceDescription::parse(reader)?));
        }

        Ok(RtcpPacketSourceDescription { descriptions })
    }

    pub fn byte_size(&self) -> usize {
        4 + self.descriptions.iter().map(|e| e.1.byte_size() + 4).sum::<usize>()
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        if self.descriptions.len() > 15 {
            return Err(Error::new(ErrorKind::InvalidData, "can't write more than 15 descriptions"));
        }

        let payload_length = self.descriptions.iter().map(|e| e.1.byte_size() + 4).sum::<usize>();

        let mut info = 2 << 6;
        info |= self.descriptions.len();
        writer.write_u8(info as u8)?;
        writer.write_u8(RtcpPacketType::SourceDescription.value())?;
        writer.write_u16::<BigEndian>(((payload_length + 3) / 4) as u16)?;

        for (ssrc, description) in &self.descriptions {
            writer.write_u32::<BigEndian>(*ssrc)?;
            description.write(writer)?;
        }
        Ok(())
    }
}