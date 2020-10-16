#![allow(dead_code)]

use std::io::{Cursor, Read, Result, Error, Write, Seek, SeekFrom};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use futures::io::ErrorKind;
use std::fmt::Debug;

pub mod packets;

/// Returns true if the buffer seem to contain a RTCP header
pub fn is_rtcp_header(buffer: &[u8]) -> bool {
    /* https://tools.ietf.org/html/rfc5761#section-4 */
    if buffer.len() < 4 {
        false
    } else {
        let payload_type = buffer[1] & 0b0111_1111;
        payload_type >= 64 || payload_type < 96
    }
}

fn read_profile_data(reader: &mut Cursor<&[u8]>, profile_data_length_with_padding: usize, padded: bool) -> Result<Option<Vec<u8>>> {
    if profile_data_length_with_padding == 0 {
        Ok(None)
    } else {
        let mut buffer = Vec::new();
        buffer.resize(profile_data_length_with_padding, 0);
        reader.read_exact(buffer.as_mut_slice())?;

        if padded {
            /* packet has been padded */
            let padded_bytes = buffer[buffer.len() - 1] as usize;
            if padded_bytes == 0 || padded_bytes > profile_data_length_with_padding {
                return Err(Error::new(ErrorKind::InvalidInput, "invalid packet padding"));
            }

            buffer.truncate(profile_data_length_with_padding - padded_bytes);
        }

        if !buffer.is_empty() {
            Ok(Some(buffer))
        } else {
            Ok(None)
        }
    }
}

fn profile_data_length(data: &Option<Vec<u8>>) -> usize {
    if let Some(payload) = data {
        ((payload.len() + 3) / 4) * 4
    } else {
        0
    }
}

fn write_profile_data(writer: &mut Cursor<&mut [u8]>, data: &Option<Vec<u8>>, padding: bool) -> Result<()> {
    if let Some(payload) = data {
        writer.write_all(payload)?;
        if padding {
            /* manual padding, faster than calculating anything */
            match payload.len() % 4 {
                0 => {},
                1 => {
                    writer.write_u8(0)?;
                    writer.write_u8(0)?;
                    writer.write_u8(3)?;
                },
                2 => {
                    writer.write_u8(0)?;
                    writer.write_u8(2)?;
                },
                3 => {
                    writer.write_u8(1)?;
                },
                _ => panic!()
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct RtcpReportBlock {
    pub fraction_lost: u8,
    pub cumulative_packets_lost: u32,
    pub highest_sequence_received: u32,
    pub jitter: u32,
    pub timestamp_last_sr: u32,
    pub delay_since_last_sr: u32,
}

impl RtcpReportBlock {
    pub fn new() -> Self {
        RtcpReportBlock {
            fraction_lost: 0,
            cumulative_packets_lost: 0,
            highest_sequence_received: 0,
            jitter: 0,
            timestamp_last_sr: 0,
            delay_since_last_sr: 0
        }
    }

    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<RtcpReportBlock> {
        let fraction_lost = reader.read_u8()?;
        let cumulative_packets_lost = reader.read_u24::<BigEndian>()?;
        let highest_sequence_received = reader.read_u32::<BigEndian>()?;
        let jitter = reader.read_u32::<BigEndian>()?;
        let timestamp_last_sr = reader.read_u32::<BigEndian>()?;
        let delay_since_last_sr = reader.read_u32::<BigEndian>()?;

        Ok(RtcpReportBlock {
            fraction_lost,
            cumulative_packets_lost,
            highest_sequence_received,
            jitter,
            timestamp_last_sr,
            delay_since_last_sr
        })
    }

    pub fn byte_size(&self) -> usize {
        self.quad_byte_size() * 4
    }

    pub fn quad_byte_size(&self) -> usize {
        5
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        writer.write_u8(self.fraction_lost)?;
        writer.write_u24::<BigEndian>(self.cumulative_packets_lost)?;
        writer.write_u32::<BigEndian>(self.highest_sequence_received)?;
        writer.write_u32::<BigEndian>(self.jitter)?;
        writer.write_u32::<BigEndian>(self.timestamp_last_sr)?;
        writer.write_u32::<BigEndian>(self.delay_since_last_sr)?;
        Ok(())
    }
}

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum RtcpPacketType {
    SenderReport,
    ReceiverReport,
    SourceDescription,

    TransportFeedback,
    PayloadFeedback
}

impl RtcpPacketType {
    pub fn from(ptype: u8) -> Option<RtcpPacketType> {
        match ptype {
            200 => Some(RtcpPacketType::SenderReport),
            201 => Some(RtcpPacketType::ReceiverReport),
            202 => Some(RtcpPacketType::SourceDescription),
            205 => Some(RtcpPacketType::TransportFeedback),
            206 => Some(RtcpPacketType::PayloadFeedback),
            _ => None
        }
    }

    pub fn value(&self) -> u8 {
        match self {
            RtcpPacketType::SenderReport => 200,
            RtcpPacketType::ReceiverReport => 201,
            RtcpPacketType::SourceDescription => 202,
            RtcpPacketType::TransportFeedback => 205,
            RtcpPacketType::PayloadFeedback => 206,
        }
    }
}

#[derive(Debug, Clone)]
pub enum RtcpPacket {
    SenderReport(packets::RtcpPacketSenderReport),
    ReceiverReport(packets::RtcpPacketReceiverReport),
    SourceDescription(packets::RtcpPacketSourceDescription),
    TransportFeedback(packets::RtcpPacketTransportFeedback),
    PayloadFeedback(packets::RtcpPacketPayloadFeedback),

    Unknown(Vec<u8>)
}

impl RtcpPacket {
    pub fn split_up_packets<'a>(buffer: &'a[u8], packets: &mut [&'a [u8]]) -> Result<usize> {
        let mut reader = Cursor::new(buffer);

        let mut packet_offset = 0usize;
        let mut packet_count = 0usize;
        loop {
            let available = reader.stream_len()? - reader.stream_position()?;
            if available == 0 { break; }

            if packets.len() - packet_count == 0 {
                return Err(Error::new(ErrorKind::InvalidInput, "buffer contains more packets than expected"));
            }

            reader.seek(SeekFrom::Current(2))?;
            let length = reader.read_u16::<BigEndian>()?;
            if length as u64 * 4 > available - 4 {
                return Err(Error::new(ErrorKind::InvalidInput, "packet length invalid"));
            }
            reader.seek(SeekFrom::Current(length as i64 * 4))?;
            if length == 0 {
                panic!();
            }
            let packet_end = packet_offset + length as usize * 4 + 4;
            packets[packet_count] = &buffer[packet_offset..packet_end];
            packet_count = packet_count + 1;
            packet_offset = packet_end;
        }

        Ok(packet_count)
    }

    pub fn parse(buffer: &[u8]) -> Result<RtcpPacket> {
        if buffer.len() < 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "truncated packet"));
        }

        match RtcpPacketType::from(buffer[1]) {
            Some(packet_type) => {
                let mut reader = Cursor::new(buffer);
                match packet_type {
                    RtcpPacketType::SenderReport => Ok(RtcpPacket::SenderReport(packets::RtcpPacketSenderReport::parse(&mut reader)?)),
                    RtcpPacketType::ReceiverReport => Ok(RtcpPacket::ReceiverReport(packets::RtcpPacketReceiverReport::parse(&mut reader)?)),
                    RtcpPacketType::SourceDescription => Ok(RtcpPacket::SourceDescription(packets::RtcpPacketSourceDescription::parse(&mut reader)?)),
                    RtcpPacketType::TransportFeedback => Ok(RtcpPacket::TransportFeedback(packets::RtcpPacketTransportFeedback::parse(&mut reader)?)),
                    RtcpPacketType::PayloadFeedback => Ok(RtcpPacket::PayloadFeedback(packets::RtcpPacketPayloadFeedback::parse(&mut reader)?)),
                }
            },
            _ => {
                Ok(RtcpPacket::Unknown(Vec::from(buffer)))
            }
        }
    }

    pub fn write(&self, buffer: &mut [u8]) -> Result<usize> {
        let mut writer = Cursor::new(buffer);
        match self {
            RtcpPacket::SenderReport(report) => report.write(&mut writer),
            RtcpPacket::ReceiverReport(report) => report.write(&mut writer),
            RtcpPacket::SourceDescription(report) => report.write(&mut writer),
            RtcpPacket::TransportFeedback(report) => report.write(&mut writer),
            RtcpPacket::PayloadFeedback(report) => report.write(&mut writer),
            RtcpPacket::Unknown(payload) => writer.write_all(payload),
        }?;
        Ok(writer.position() as usize)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>> {
        let mut buffer = [0u8; 2048];
        let write_result = self.write(&mut buffer)?;
        Ok(buffer[0..write_result].to_vec())
    }
}