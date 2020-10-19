/// Source Description RTCP Packet
/// https://tools.ietf.org/html/rfc3611#section-2
///
/// 0                   1                   2                   3
/// 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |V=2|P|reserved |   PT=XR=207   |             length            |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |                              SSRC                             |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// :                         report blocks                         :
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+

use std::io::{Cursor, Result, Error, ErrorKind, Read};
use byteorder::{ReadBytesExt, BigEndian};
use crate::utils::rtcp::RtcpPacketType;

mod target_bitrate;
pub use target_bitrate::*;

mod receiver_reference_time;
pub use receiver_reference_time::*;

mod delay_last_receiver_report;
pub use delay_last_receiver_report::*;

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum ExtendedReportBlockType {
    //1  Loss RLE Report Block
    //2  Duplicate RLE Report Block
    //3  Packet Receipt Times Report Block
    ReceiverReferenceTime,
    DelayLastReceiverReport,
    //6  Statistics Summary Report Block
    //7  VoIP Metrics Report Block
    /*
     * Types 1-3 and 6-7 are unknown to googles libWebRTC implementation.
     * See: https://source.chromium.org/chromium/chromium/src/+/master:third_party/webrtc/modules/rtp_rtcp/source/rtcp_packet/extended_reports.cc
     */

    /// https://source.chromium.org/chromium/chromium/src/+/master:third_party/webrtc/modules/rtp_rtcp/source/rtcp_packet/target_bitrate.h
    TargetBitrate,
}

impl ExtendedReportBlockType {
    pub fn from(ptype: u8) -> Option<ExtendedReportBlockType> {
        match ptype {
            4 => Some(ExtendedReportBlockType::ReceiverReferenceTime),
            5 => Some(ExtendedReportBlockType::DelayLastReceiverReport),
            42 => Some(ExtendedReportBlockType::TargetBitrate),
            _ => None
        }
    }

    pub fn value(&self) -> u8 {
        match self {
            ExtendedReportBlockType::ReceiverReferenceTime => 4,
            ExtendedReportBlockType::DelayLastReceiverReport => 5,
            ExtendedReportBlockType::TargetBitrate => 42,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ExtendedReportBlock {
    ReceiverReferenceTime(ExtendedReportReceiverReferenceTime),
    TargetBitrate(ExtendedReportTargetBitrate),
    DelayLastReceiverReport(ExtendedReportDelayLastReceiverReport),
    Unknown {
        block_type: u8,
        type_specific: u8,
        content: Vec<u8>
    }
}

#[derive(Debug, Clone)]
pub struct RtcpPacketExtendedReport {
    pub ssrc: u32,
    pub blocks: Vec<ExtendedReportBlock>
}

impl RtcpPacketExtendedReport {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<RtcpPacketExtendedReport> {
        let info = reader.read_u8()?;
        if (info >> 6) != 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid version, expected 2"));
        }

        let payload_type = reader.read_u8()?;
        if payload_type != RtcpPacketType::ExtendedReport.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "rtcp packet isn't a extended report packet"));
        }

        let length = reader.read_u16::<BigEndian>()?;
        if length < 1 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid packet length"));
        }

        let ssrc = reader.read_u32::<BigEndian>()?;

        let mut blocks = Vec::with_capacity(16);
        let mut read = 1 as u16;
        while read < length {
            let block_start = reader.position();
            let block_type = reader.read_u8()?;
            let type_specific = reader.read_u8()?;
            let block_length = reader.read_u16::<BigEndian>()? as usize * 4;
            let full_block_length = block_length + 4;

            let mut block_data = unsafe { std::mem::MaybeUninit::<[u8; 2048]>::uninit().assume_init() };
            if full_block_length > block_data.len() {
                return Err(Error::new(ErrorKind::InvalidInput, "too many blocks"));
            }
            reader.set_position(block_start);
            reader.read_exact(&mut block_data[0..full_block_length])?;

            let block = {
                if let Some(block_type) = ExtendedReportBlockType::from(block_type) {
                    let mut reader = Cursor::new(&block_data[0..full_block_length]);
                    match block_type {
                        ExtendedReportBlockType::TargetBitrate => ExtendedReportBlock::TargetBitrate(ExtendedReportTargetBitrate::parse(&mut reader)?),
                        ExtendedReportBlockType::ReceiverReferenceTime => ExtendedReportBlock::ReceiverReferenceTime(ExtendedReportReceiverReferenceTime::parse(&mut reader)?),
                        ExtendedReportBlockType::DelayLastReceiverReport => ExtendedReportBlock::DelayLastReceiverReport(ExtendedReportDelayLastReceiverReport::parse(&mut reader)?)
                    }
                } else {
                    ExtendedReportBlock::Unknown {
                        block_type,
                        type_specific,
                        content: block_data[4..].to_vec()
                    }
                }
            };
            blocks.push(block);

            read += (block_length / 4 + 1) as u16;
        }

        Ok(RtcpPacketExtendedReport {
            ssrc,
            blocks
        })
    }

    /*
    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {

        Ok(())
    }
    */
}