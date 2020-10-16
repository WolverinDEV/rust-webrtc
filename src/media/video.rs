use crate::media::rtp_based::MediaChannelRtpBased;
use std::ops::{Deref, DerefMut};
use crate::rtc::MediaId;
use crate::transport::{RTCTransportControl};
use tokio::sync::mpsc;
use crate::media::{MediaChannel, TypedMediaChannel, MediaChannelIncomingEvent};
use webrtc_sdp::media_type::{SdpMedia, SdpMediaValue};

pub use crate::media::rtp_based::MediaChannelRtpBasedEvents as MediaChannelVideoEvents;

pub struct MediaChannelVideo {
    base: MediaChannelRtpBased
}

impl MediaChannelVideo {
    pub fn new(media_id: MediaId, ice_control: mpsc::UnboundedSender<RTCTransportControl>) -> Self {
        MediaChannelVideo {
            base: MediaChannelRtpBased::new(SdpMediaValue::Video, media_id, ice_control)
        }
    }
}

impl Deref for MediaChannelVideo {
    type Target = MediaChannelRtpBased;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl DerefMut for MediaChannelVideo {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.base
    }
}

impl MediaChannel for MediaChannelVideo {
    fn as_typed(&mut self) -> TypedMediaChannel {
        TypedMediaChannel::Video(self)
    }

    fn media_id(&self) -> &(usize, String) {
        self.base.media_id()
    }

    fn configure(&mut self, media: &SdpMedia) -> Result<(), String> {
        self.base.configure(media)
    }

    fn generate_sdp(&self) -> Result<SdpMedia, String> {
        self.base.generate_sdp()
    }

    fn process_peer_event(&mut self, event: &mut Option<MediaChannelIncomingEvent>) {
        self.base.process_peer_event(event)
    }
}