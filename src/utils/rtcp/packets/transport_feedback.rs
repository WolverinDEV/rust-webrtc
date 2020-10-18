use crate::utils::rtcp::RtcpPacketType;
use std::io::{Cursor, Result, ErrorKind, Error};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use std::fmt::{Debug, Formatter};
use crate::utils::SequenceNumber;

#[derive(Clone)]
pub struct RtcpFeedbackGenericNACK {
    pub packet_id: u16,
    pub bitmask_lost_packets: u16
}

pub struct NackPacketIterator {
    packet_id: u16,
    bitmask: u16,
    index: u8
}

impl Iterator for NackPacketIterator {
    type Item = u16;

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < 17 {
            if self.index == 0 {
                self.index = 1;
                return Some(self.packet_id);
            } else {
                let set = (self.bitmask & (1 << (self.index - 1))) > 0;
                self.index += 1;
                if set {
                    return Some(self.packet_id.wrapping_add(self.index as u16));
                }
            }
        }

        None
    }
}

impl RtcpFeedbackGenericNACK {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<RtcpFeedbackGenericNACK> {
        let pid = reader.read_u16::<BigEndian>()?;
        let bitmask_lost_packets = reader.read_u16::<BigEndian>()?;

        Ok(RtcpFeedbackGenericNACK{
            packet_id: pid,
            bitmask_lost_packets
        })
    }

    pub fn byte_size() -> usize {
        4
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        writer.write_u16::<BigEndian>(self.packet_id)?;
        writer.write_u16::<BigEndian>(self.bitmask_lost_packets)?;
        Ok(())
    }

    pub fn lost_packets(&self) -> NackPacketIterator {
        NackPacketIterator{ index: 0, packet_id: self.packet_id, bitmask: self.bitmask_lost_packets }
    }
}

impl Debug for RtcpFeedbackGenericNACK {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut packets = Vec::<u16>::new();
        packets.reserve(17);
        self.lost_packets().for_each(|pkt| packets.push(pkt));
        f.debug_struct("RtcpFeedbackGenericNACK")
            .field("lost_packets", &packets)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum RtcpTransportFeedback {
    GenericNACK(Vec<RtcpFeedbackGenericNACK>),
}

impl RtcpTransportFeedback {
    pub fn create_generic_nack(packets: &[SequenceNumber<u16>]) -> RtcpTransportFeedback {
        let mut feedbacks = Vec::new();

        let mut index = 0usize;
        while index < packets.len() {
            let head = packets[index].clone();
            let mut mask = 0u16;

            index = index + 1;
            while index < packets.len() {
                let difference = packets[index].difference(&head, Some(256));
                if difference >= 16 { break; }
                mask |= 1 << difference;

                index = index + 1;
            }

            feedbacks.push(RtcpFeedbackGenericNACK{ packet_id: head.packet_id, bitmask_lost_packets: mask });
        }

        RtcpTransportFeedback::GenericNACK(feedbacks)
    }
}

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum RtcpTransportFeedbackType {
    GenericNACK,
}

impl RtcpTransportFeedbackType {
    pub fn id(&self) -> u8 {
        match self {
            RtcpTransportFeedbackType::GenericNACK => 1,
        }
    }

    pub fn from(id: u8) -> Option<RtcpTransportFeedbackType> {
        match id {
            1 => Some(RtcpTransportFeedbackType::GenericNACK),
            _ => None
        }
    }
}

///https://tools.ietf.org/html/rfc4585#section-6.1
#[derive(Debug, Clone)]
pub struct RtcpPacketTransportFeedback {
    pub ssrc: u32,
    pub media_ssrc: u32,
    pub feedback: RtcpTransportFeedback
}

impl RtcpPacketTransportFeedback {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<RtcpPacketTransportFeedback> {
        let info = reader.read_u8()?;
        if (info >> 6) != 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid version, expected 2"));
        }

        let payload_type = reader.read_u8()?;
        if payload_type != RtcpPacketType::TransportFeedback.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "rtcp packet isn't a transport feedback packet"));
        }

        let feedback_type = info & 0x1F;
        let feedback_type = RtcpTransportFeedbackType::from(feedback_type)
            .ok_or(Error::new(ErrorKind::InvalidInput, "invalid/unknown feedback message type"))?;

        let length = reader.read_u16::<BigEndian>()?;
        let ssrc = reader.read_u32::<BigEndian>()?;
        let media_ssrc = reader.read_u32::<BigEndian>()?;

        let feedback = {
            (match feedback_type {
                RtcpTransportFeedbackType::GenericNACK => {
                    let count = (length - 2) as usize * 4 / RtcpFeedbackGenericNACK::byte_size();
                    let mut reports = Vec::<RtcpFeedbackGenericNACK>::new();
                    reports.reserve(count);
                    for _ in 0..count {
                        reports.push(RtcpFeedbackGenericNACK::parse(reader)?);
                    }
                    Ok(RtcpTransportFeedback::GenericNACK(reports))
                }
            }) as Result<RtcpTransportFeedback>
        }?;

        Ok(RtcpPacketTransportFeedback{
            ssrc,
            media_ssrc,
            feedback
        })
    }

    pub fn feedback_type(&self) -> RtcpTransportFeedbackType {
        match &self.feedback {
            RtcpTransportFeedback::GenericNACK(..) => RtcpTransportFeedbackType::GenericNACK
        }
    }

    pub fn byte_size(&self) -> usize {
        12 + self.feedback_byte_size()
    }

    fn feedback_byte_size(&self) -> usize {
        match &self.feedback {
            RtcpTransportFeedback::GenericNACK(nacks) => nacks.len() * RtcpFeedbackGenericNACK::byte_size(),
        }
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        let mut info = 2 << 6;
        info |= self.feedback_type().id();
        writer.write_u8(info as u8)?;
        writer.write_u8(RtcpPacketType::TransportFeedback.value())?;

        let fb_byte_size = self.feedback_byte_size();
        assert_eq!(fb_byte_size % 4, 0);
        writer.write_u16::<BigEndian>((2 + fb_byte_size / 4) as u16)?;
        writer.write_u32::<BigEndian>(self.ssrc)?;
        writer.write_u32::<BigEndian>(self.media_ssrc)?;

        match &self.feedback {
            RtcpTransportFeedback::GenericNACK(nacks) => {
                for nack in nacks.iter() {
                    nack.write(writer)?;
                }
            }
        }

        Ok(())
    }
}