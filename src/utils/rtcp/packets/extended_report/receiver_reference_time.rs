use std::io::{Cursor, Result, Error, ErrorKind};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use crate::utils::rtcp::packets::extended_report::ExtendedReportBlockType;

///     0                   1                   2                   3
///     0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
///    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///    |     BT=4      |   reserved    |       block length = 2        |
///    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///    |              NTP timestamp, most significant word             |
///    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///    |             NTP timestamp, least significant word             |
///    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+

#[derive(Debug, Clone)]
pub struct ExtendedReportReceiverReferenceTime {
    pub timestamp: u64
}

impl ExtendedReportReceiverReferenceTime {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<ExtendedReportReceiverReferenceTime> {
        if reader.read_u8()? != ExtendedReportBlockType::ReceiverReferenceTime.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid block type, expected receiver reference time"));
        }

        let _ = reader.read_u8()?;
        if reader.read_u16::<BigEndian>()? != 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid block length, expected 2"));
        }

        Ok(ExtendedReportReceiverReferenceTime { timestamp: reader.read_u64::<BigEndian>()? })
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        writer.write_u8(ExtendedReportBlockType::ReceiverReferenceTime.value())?;
        writer.write_u8(0)?;
        writer.write_u16::<BigEndian>(2)?;
        writer.write_u64::<BigEndian>(self.timestamp)?;

        Ok(())
    }
}
