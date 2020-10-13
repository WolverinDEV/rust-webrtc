use crate::utils::rtcp::{RtcpReportBlock, read_profile_data, profile_data_length, RtcpPacketType, write_profile_data};
use std::io::{Cursor, Result, ErrorKind, Error, Read, Write};
use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use std::fmt::{Debug, Formatter};

#[derive(Debug, Clone)]
pub enum RtcpPayloadFeedback {
    PictureLossIndication,
    //SliceLossIndication,
    ReferencePictureSelectionIndication,
    ApplicationSpecific(Vec<u8>),
}

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum RtcpPayloadFeedbackType {
    PictureLossIndication,
    SliceLossIndication,
    ReferencePictureSelectionIndication,
    ApplicationLayerFeedback
}

impl RtcpPayloadFeedbackType {
    pub fn id(&self) -> u8 {
        match self {
            RtcpPayloadFeedbackType::PictureLossIndication => 1,
            RtcpPayloadFeedbackType::SliceLossIndication => 2,
            RtcpPayloadFeedbackType::ReferencePictureSelectionIndication => 3,
            RtcpPayloadFeedbackType::ApplicationLayerFeedback => 15,
        }
    }

    pub fn from(id: u8) -> Option<RtcpPayloadFeedbackType> {
        match id {
            1 => Some(RtcpPayloadFeedbackType::PictureLossIndication),
            2 => Some(RtcpPayloadFeedbackType::SliceLossIndication),
            3 => Some(RtcpPayloadFeedbackType::ReferencePictureSelectionIndication),
            15 => Some(RtcpPayloadFeedbackType::ApplicationLayerFeedback),
            _ => None
        }
    }
}

///https://tools.ietf.org/html/rfc4585#section-6.1
#[derive(Debug, Clone)]
pub struct RtcpPacketPayloadFeedback {
    pub ssrc: u32,
    pub media_ssrc: u32,
    pub feedback: RtcpPayloadFeedback
}

impl RtcpPacketPayloadFeedback {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<RtcpPacketPayloadFeedback> {
        let info = reader.read_u8()?;
        if (info >> 6) != 2 {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid version, expected 2"));
        }

        let payload_type = reader.read_u8()?;
        if payload_type != RtcpPacketType::PayloadFeedback.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "rtcp packet isn't a feedback packet"));
        }

        let feedback_type = info & 0x1F;
        let feedback_type = RtcpPayloadFeedbackType::from(feedback_type)
            .ok_or(Error::new(ErrorKind::InvalidInput, "invalid/unknown feedback message type"))?;

        let length = reader.read_u16::<BigEndian>()?;
        let ssrc = reader.read_u32::<BigEndian>()?;
        let media_ssrc = reader.read_u32::<BigEndian>()?;

        let feedback = {
            match feedback_type {
                RtcpPayloadFeedbackType::PictureLossIndication => {
                    if length != 2 {
                        return Err(Error::new(ErrorKind::InvalidInput, "invalid payload, expected 2"));
                    }
                    Ok(RtcpPayloadFeedback::PictureLossIndication)
                },
                /* TODO: Slice loss indication */
                RtcpPayloadFeedbackType::ReferencePictureSelectionIndication => {
                    if length != 2 {
                        return Err(Error::new(ErrorKind::InvalidInput, "invalid payload, expected 2"));
                    }
                    Ok(RtcpPayloadFeedback::ReferencePictureSelectionIndication)
                },
                RtcpPayloadFeedbackType::ApplicationLayerFeedback => {
                    let mut buffer = Vec::<u8>::new();
                    buffer.resize((length - 2) as usize * 4, 0);
                    reader.read_exact(buffer.as_mut_slice())?;
                    Ok(RtcpPayloadFeedback::ApplicationSpecific(buffer))
                }
                _ => Err(Error::new(ErrorKind::Other, "unsupported feedback type"))
            }
        }?;

        Ok(RtcpPacketPayloadFeedback {
            ssrc,
            media_ssrc,
            feedback
        })
    }

    pub fn feedback_type(&self) -> RtcpPayloadFeedbackType {
        match self.feedback {
            RtcpPayloadFeedback::PictureLossIndication => RtcpPayloadFeedbackType::PictureLossIndication,
            RtcpPayloadFeedback::ReferencePictureSelectionIndication => RtcpPayloadFeedbackType::ReferencePictureSelectionIndication,
            RtcpPayloadFeedback::ApplicationSpecific(..) => RtcpPayloadFeedbackType::ApplicationLayerFeedback
        }
    }

    pub fn byte_size(&self) -> usize {
        12 + self.feedback_byte_size()
    }

    fn feedback_byte_size(&self) -> usize {
        match &self.feedback {
            RtcpPayloadFeedback::PictureLossIndication => 0,
            RtcpPayloadFeedback::ReferencePictureSelectionIndication => 0,
            RtcpPayloadFeedback::ApplicationSpecific(payload) => payload.len()
        }
    }

    pub fn write(&self, writer: &mut Cursor<&mut [u8]>) -> Result<()> {
        let mut info = 2 << 6;
        info |= self.feedback_type().id();
        writer.write_u8(info as u8)?;
        writer.write_u8(RtcpPacketType::PayloadFeedback.value())?;

        let fb_byte_size = self.feedback_byte_size();
        if (fb_byte_size % 4) != 0 {
            /* set the padding flag */
            info |= 1 << 5;
        }
        writer.write_u16::<BigEndian>((2 + (fb_byte_size + 3) / 4) as u16)?;
        writer.write_u32::<BigEndian>(self.ssrc)?;
        writer.write_u32::<BigEndian>(self.media_ssrc)?;

        match &self.feedback {
            RtcpPayloadFeedback::PictureLossIndication => {},
            RtcpPayloadFeedback::ReferencePictureSelectionIndication => {},
            RtcpPayloadFeedback::ApplicationSpecific(payload) => {
                writer.write_all(payload)?;
            },
        }

        /* manual padding, faster than calculating anything */
        match fb_byte_size % 4 {
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

        Ok(())
    }
}