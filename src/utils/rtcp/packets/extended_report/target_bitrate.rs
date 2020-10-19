//!  ~RFC 4585: Feedback format.~
//!  Defined by libWebRTC: https://source.chromium.org/chromium/chromium/src/+/master:third_party/webrtc/modules/rtp_rtcp/source/rtcp_packet/target_bitrate.cc
//!
//!  Common packet format:
//!
//!   0                   1                   2                   3
//!   0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//!  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//!  |     BT=42     |   reserved    |         block length          |
//!  +=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+
//!
//!  Target bitrate item (repeat as many times as necessary).
//!
//!  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//!  |   S   |   T   |                Target Bitrate                 |
//!  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//!  :  ...                                                          :
//!
//!  Spatial Layer (S): 4 bits
//!    Indicates which temporal layer this bitrate concerns.
//!
//!  Temporal Layer (T): 4 bits
//!    Indicates which temporal layer this bitrate concerns.
//!
//!  Target Bitrate: 24 bits
//!    The encoder target bitrate for this layer, in kbps.
//!
//!  As an example of how S and T are intended to be used, VP8 simulcast will
//!  use a separate TargetBitrate message per stream, since they are transmitted
//!  on separate SSRCs, with temporal layers grouped by stream.
//!  If VP9 SVC is used, there will be only one SSRC, so each spatial and
//!  temporal layer combo used shall be specified in the TargetBitrate packet.

use std::io::{Cursor, Result, ErrorKind, Error};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use crate::utils::rtcp::packets::extended_report::ExtendedReportBlockType;

#[derive(Debug, Clone)]
pub struct TargetBitrateItem {
    pub spatial_layer: u8,
    pub temporal_layer: u8,
    pub bitrate: u32
}

#[derive(Debug, Clone)]
pub struct ExtendedReportTargetBitrate {
    pub items: Vec<TargetBitrateItem>
}

impl ExtendedReportTargetBitrate {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<ExtendedReportTargetBitrate> {
        if reader.read_u8()? != ExtendedReportBlockType::TargetBitrate.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid block type, expected target bitrate"));
        }

        let _ = reader.read_u8()?;
        let length = reader.read_u16::<BigEndian>()? as usize;

        let mut items = Vec::<TargetBitrateItem>::with_capacity(length);
        for _ in 0..length {
            let info = reader.read_u8()?;
            let bitrate = reader.read_u24::<BigEndian>()?;
            items.push(TargetBitrateItem {
                bitrate,
                spatial_layer: info >> 4,
                temporal_layer: info & 0xF
            });
        }

        Ok(ExtendedReportTargetBitrate { items })
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        writer.write_u8(ExtendedReportBlockType::TargetBitrate.value())?;
        writer.write_u8(0)?;
        writer.write_u16::<BigEndian>(self.items.len() as u16)?;

        for item in self.items.iter() {
            writer.write_u8((item.spatial_layer << 4) as u8 | (item.temporal_layer & 0xF) as u8)?;
            writer.write_u24::<BigEndian>(item.bitrate)?;
        }

        Ok(())
    }
}

