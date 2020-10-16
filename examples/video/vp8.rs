use std::io::{ Cursor, Result };
use byteorder::ReadBytesExt;
use std::fmt::Debug;
use serde::export::Formatter;

#[derive(Debug)]
pub enum PictureId {
    /// No picture id id present
    None,
    /// Only the last 7 bits of the picture ID are present
    Short(u8),
    /// The full picture ID is present
    Full(u16)
}

#[derive(Debug)]
pub struct LayerExtension {
    temporal_index: Option<(u8, bool)>,
    key_index: Option<u8>
}

/// https://tools.ietf.org/html/rfc7741#section-4.2
pub struct VP8PayloadDescriptor<'a> {
    pub picture_id: PictureId,
    pub tl0_pic_idx: Option<u8>,
    pub layer_extension: Option<LayerExtension>,

    pub non_reference_frame: bool,
    pub partition_start: bool,
    pub partition_index: u8,

    pub payload: &'a [u8]
}

impl<'a> VP8PayloadDescriptor<'a> {
    pub fn parse(payload: &'a [u8]) -> Result<Self> {
        let mut reader = Cursor::new(payload);

        let mut picture_id = PictureId::None;
        let mut tl0_pic_idx: Option<u8> = None;
        let mut layer_extension: Option<LayerExtension> = None;

        let bits = reader.read_u8()?;
        if (bits & 0b1000_0000) > 0 {
            /* extended control bits */
            let bits = reader.read_u8()?;
            if (bits & 0b1000_0000) > 0 {
                /* picture id present */
                let first_octets = reader.read_u8()?;
                if (first_octets & 0x80) > 0 {
                    picture_id = PictureId::Full(((first_octets as u16 & 0x7F) << 8) | reader.read_u8()? as u16);
                } else {
                    picture_id = PictureId::Short(first_octets);
                }
            }

            if (bits & 0b0100_0000) > 0 {
                /* tl0_pic_idx present */
                tl0_pic_idx = Some(reader.read_u8()?);
            }

            if (bits & 0b0011_0000) > 0 {
                /* layer extension present */
                let ext = reader.read_u8()?;
                let mut temporal_index: Option<(u8, bool)> = None;
                let mut key_index: Option<u8> = None;

                if (bits & 0b0010_0000) > 0 {
                    temporal_index = Some((ext >> 6, ((ext >> 5) & 0x01) == 1));
                }

                if (bits & 0b0001_0000) > 0 {
                    key_index = Some(ext & 0x1F);
                }

                layer_extension = Some(LayerExtension{ temporal_index, key_index });
            }
        }

        Ok(VP8PayloadDescriptor{
            picture_id,
            tl0_pic_idx,
            layer_extension,

            non_reference_frame: (bits & 0b0010_0000) > 0,

            partition_start: (bits & 0b0001_0000) > 0,
            partition_index: bits & 0x7,

            payload: &payload[reader.position() as usize..]
        })
    }
}

impl Debug for VP8PayloadDescriptor<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VP8PayloadDescriptor")
            .field("picture_id", &self.picture_id)
            .field("tl0_pic_idx", &self.tl0_pic_idx)
            .field("layer_extension", &self.layer_extension)
            .field("non_reference_frame", &self.non_reference_frame)
            .field("partition_start", &self.partition_start)
            .field("partition_index", &self.partition_index)
            .finish()
    }
}