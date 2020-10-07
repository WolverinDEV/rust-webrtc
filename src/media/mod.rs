use crate::media::application::MediaChannelApplication;
use crate::rtc::MediaId;
use webrtc_sdp::media_type::SdpMedia;
use crate::ice::PeerICEConnectionEvent;

pub mod application;

pub enum TypedMediaChannel<'a> {
    Application(&'a mut MediaChannelApplication)
}

pub trait MediaChannel {
    fn as_typed(&mut self) -> TypedMediaChannel;

    fn media_id(&self) -> &MediaId;

    fn configure(&mut self, media: &SdpMedia) -> Result<(), String>;
    fn generate_sdp(&self) -> Result<SdpMedia, String>;

    fn process_peer_event(&mut self, event: &PeerICEConnectionEvent);
}
