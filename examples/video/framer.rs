use web_test::utils::SequenceNumber;
use crate::video::FullPictureId;

pub enum ReframerEvent {
    /// We've failed to assemble the sequence,
    SequenceReset,
}

pub struct Reframer {

}

impl Reframer {
    pub fn process_packet(&mut self, packet_id: SequenceNumber<u16>, picture_id: FullPictureId, begin: bool, end: bool) {

    }
}