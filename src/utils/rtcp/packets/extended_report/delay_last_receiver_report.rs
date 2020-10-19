/// 4.5.  DLRR Report Block:
/// https://tools.ietf.org/html/rfc3611#section-4.5
///
///   0                   1                   2                   3
///   0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
///  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///  |     BT=5      |   reserved    |         block length          |
///  +=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+
///  |                 SSRC_1 (SSRC of first receiver)               | sub-
///  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+ block
///  |                         last RR (LRR)                         |   1
///  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///  |                   delay since last RR (DLRR)                  |
///  +=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+
///  |                 SSRC_2 (SSRC of second receiver)              | sub-
///  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+ block
///  :                               ...                             :   2
///  +=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+


use std::io::{Cursor, Result, ErrorKind, Error};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use crate::utils::rtcp::packets::extended_report::ExtendedReportBlockType;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct ExtendedReportDelayLastReceiverReport {
    /// The key is the SSRC,
    /// the first element in the value tuple is the last receiver report timestamp.
    /// The second element is the delay, expressed in units of 1/65536 seconds, between
    /// receiving the last Receiver Reference Time Report Block and sending this DLRR Report Block.
    pub delays: BTreeMap<u32, (u32, u32)>
}

impl ExtendedReportDelayLastReceiverReport {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<ExtendedReportDelayLastReceiverReport> {
        if reader.read_u8()? != ExtendedReportBlockType::DelayLastReceiverReport.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid block type, expected delay since last receiver report"));
        }

        let _ = reader.read_u8()?;
        let mut length = reader.read_u16::<BigEndian>()?;

        let mut delays = BTreeMap::new();
        while length > 0 {
            if length < 3 {
                return Err(Error::new(ErrorKind::InvalidInput, "invalid report block length"));
            }

            let ssrc = reader.read_u32::<BigEndian>()?;
            let last_rr = reader.read_u32::<BigEndian>()?;
            let delay = reader.read_u32::<BigEndian>()?;
            delays.insert(ssrc, (last_rr, delay));

            length -= 3;
        }

        Ok(ExtendedReportDelayLastReceiverReport { delays })
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        writer.write_u8(ExtendedReportBlockType::DelayLastReceiverReport.value())?;
        writer.write_u8(0)?;
        writer.write_u16::<BigEndian>(self.delays.len() as u16 * 3)?;

        for (ssrc, (last_rr, delay)) in self.delays.iter() {
            writer.write_u32::<BigEndian>(*ssrc)?;
            writer.write_u32::<BigEndian>(*last_rr)?;
            writer.write_u32::<BigEndian>(*delay)?;
        }

        Ok(())
    }
}
