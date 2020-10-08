#![allow(dead_code)]

use std::io::{Cursor, Read, Result, Error, Write};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use futures::io::ErrorKind;
use std::fmt::Debug;
use serde::export::Formatter;

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
        if reader.read_to_end(&mut buffer)? != profile_data_length_with_padding {
            return Err(Error::new(ErrorKind::InvalidInput, "given packet length was too long"));
        }

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
        if padding && (payload.len() & 0x3) != 0 {
            let pad_bytes = !(payload.len() & 0x3) + 1;
            for _ in 0..(pad_bytes - 1) {
                writer.write_u8(0)?;
            }
            writer.write_u8(pad_bytes as u8)?;
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct RtcpReportBlock {
    pub fraction_lost: u8,
    pub cumulative_packets_lost: u32,
    pub highest_sequence_received: u32,
    pub jitter: u32,
    pub timestamp_last_sr: u32,
    pub delay_since_last_sr: u32,
}

impl RtcpReportBlock {
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

/// https://tools.ietf.org/html/rfc1889#section-6.3.1
#[derive(Debug)]
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

///https://tools.ietf.org/html/rfc1889#section-6.3.2
#[derive(Debug)]
pub struct RtcpPacketReceiverReport {
    pub ssrc: u32,
    pub reports: Vec<RtcpReportBlock>,
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
        8 + self.reports.iter().map(|e| e.byte_size()).sum::<usize>() + profile_data_length(&self.profile_data)
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        if self.reports.len() > 15 {
            return Err(Error::new(ErrorKind::InvalidData, "can't write more than 15 report blocks"));
        }

        let mut info = 2 << 6;
        info |= 1 << 5; /* we do padding */
        info |= self.reports.len();
        writer.write_u8(info as u8)?;
        writer.write_u8(RtcpPacketType::ReceiverReport.value())?;

        let mut quad_size = 1 + self.reports.iter().map(|e| e.quad_byte_size()).sum::<usize>();
        if let Some(payload) = &self.profile_data {
            quad_size += (payload.len() + 3) / 4;
        }
        writer.write_u16::<BigEndian>(quad_size as u16)?;

        writer.write_u32::<BigEndian>(self.ssrc)?;

        for report in self.reports.iter() {
            report.write(writer)?;
        }

        write_profile_data(writer, &self.profile_data, true)?;
        Ok(())
    }
}

pub struct RtcpFeedbackGenericNACK {
    packet_id: u16,
    bitmask_lost_packets: u16
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
}

impl Debug for RtcpFeedbackGenericNACK {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut packets = Vec::<u16>::new();
        packets.reserve(17);
        packets.push(self.packet_id);
        for index in 0..16 {
            let value = self.bitmask_lost_packets & (1 << index);
            if value == 0 {
                continue;
            }

            packets.push(self.packet_id.wrapping_add(index + 1));
        }

        f.debug_struct("RtcpFeedbackGenericNACK")
            .field("lost_packets", &packets)
            .finish()
    }
}

#[derive(Debug)]
pub enum RtcpFeedback {
    GenericNACK(Vec<RtcpFeedbackGenericNACK>),
}

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum RtcpFeedbackType {
    /* Transport layer FB message */
    GenericNACK,

    /* Payload-specific FB message */
    PictureLossIndication,
    SliceLossIndication,
    ReferencePictureSelectionIndication,
    ApplicationLayerFeedback
}

impl RtcpFeedbackType {
    pub fn id(&self) -> (u8, u8) {
        match self {
            RtcpFeedbackType::GenericNACK => (205, 1),
            RtcpFeedbackType::PictureLossIndication => (206, 1),
            RtcpFeedbackType::SliceLossIndication => (206, 2),
            RtcpFeedbackType::ReferencePictureSelectionIndication => (206, 3),
            RtcpFeedbackType::ApplicationLayerFeedback => (206, 15),
        }
    }

    pub fn from(id: &(u8, u8)) -> Option<RtcpFeedbackType> {
        match id {
            &(205, 1) => Some(RtcpFeedbackType::GenericNACK),
            &(206, 1) => Some(RtcpFeedbackType::PictureLossIndication),
            &(206, 2) => Some(RtcpFeedbackType::SliceLossIndication),
            &(206, 3) => Some(RtcpFeedbackType::ReferencePictureSelectionIndication),
            &(206, 25) => Some(RtcpFeedbackType::ApplicationLayerFeedback),
            _ => None
        }
    }
}

///https://tools.ietf.org/html/rfc4585#section-6.1
#[derive(Debug)]
pub struct RtcpPacketFeedback {
    ssrc: u32,
    media_ssrc: u32,
    feedback: RtcpFeedback
}

impl RtcpPacketFeedback {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<RtcpPacketFeedback> {
        let info = reader.read_u8()?;
        if (info >> 6) != 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid version, expected 2"));
        }

        let payload_type = reader.read_u8()?;
        if payload_type != 205 && payload_type != 206 {
            return Err(Error::new(ErrorKind::InvalidInput, "rtcp packet isn't a feedback packet"));
        }

        let feedback_type = info & 0x1F;
        let feedback_type = RtcpFeedbackType::from(&(payload_type, feedback_type))
            .ok_or(Error::new(ErrorKind::InvalidInput, "invalid/unknown feedback message type"))?;

        let length = reader.read_u16::<BigEndian>()?;
        let ssrc = reader.read_u32::<BigEndian>()?;
        let media_ssrc = reader.read_u32::<BigEndian>()?;

        let feedback = {
            match feedback_type {
                RtcpFeedbackType::GenericNACK => {
                    let count = (length - 2) as usize * 4 / RtcpFeedbackGenericNACK::byte_size();
                    let mut reports = Vec::<RtcpFeedbackGenericNACK>::new();
                    reports.reserve(count);
                    for _ in 0..count {
                        reports.push(RtcpFeedbackGenericNACK::parse(reader)?);
                    }
                    Ok(RtcpFeedback::GenericNACK(reports))
                }
                _ => Err(Error::new(ErrorKind::Other, "unsupported feedback type"))
            }
        }?;

        Ok(RtcpPacketFeedback{
            ssrc,
            media_ssrc,
            feedback
        })
    }

    pub fn byte_size(&self) -> usize {
        4
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        unimplemented!(); /* TODO! */
        Ok(())
    }
}

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum RtcpPacketType {
    SenderReport,
    ReceiverReport,

    Feedback
}

impl RtcpPacketType {
    pub fn from(ptype: u8) -> Option<RtcpPacketType> {
        match ptype {
            200 => Some(RtcpPacketType::SenderReport),
            201 => Some(RtcpPacketType::ReceiverReport),
            205 | 206 => Some(RtcpPacketType::Feedback),
            _ => None
        }
    }

    pub fn value(&self) -> u8 {
        match self {
            RtcpPacketType::SenderReport => 200,
            RtcpPacketType::ReceiverReport => 201,
            RtcpPacketType::Feedback => 205,
        }
    }
}

#[derive(Debug)]
pub enum RtcpPacket {
    SenderReport(RtcpPacketSenderReport),
    ReceiverReport(RtcpPacketReceiverReport),
    Feedback(RtcpPacketFeedback)
}

impl RtcpPacket {
    pub fn parse(buffer: &[u8]) -> Result<RtcpPacket> {
        if buffer.len() < 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "truncated packet"));
        }

        let packet_type = RtcpPacketType::from(buffer[1])
            .ok_or(Error::new(ErrorKind::InvalidInput, "unknown rtcp packet"))?;

        let mut reader = Cursor::new(buffer);
        match packet_type {
            RtcpPacketType::SenderReport => Ok(RtcpPacket::SenderReport(RtcpPacketSenderReport::parse(&mut reader)?)),
            RtcpPacketType::ReceiverReport => Ok(RtcpPacket::ReceiverReport(RtcpPacketReceiverReport::parse(&mut reader)?)),
            RtcpPacketType::Feedback => Ok(RtcpPacket::Feedback(RtcpPacketFeedback::parse(&mut reader)?)),
        }
    }

    /* TODO: Write */
}