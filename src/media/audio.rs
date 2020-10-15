use crate::media::rtp_based::MediaChannelRtpBased;
use std::ops::{Deref, DerefMut};
use crate::rtc::MediaId;
use crate::ice::{PeerICEConnectionControl};
use tokio::sync::mpsc;
use crate::media::{MediaChannel, TypedMediaChannel, MediaChannelIncomingEvent};
use webrtc_sdp::media_type::{SdpMedia, SdpMediaValue};

pub struct MediaChannelAudio {
    base: MediaChannelRtpBased
}

impl MediaChannelAudio {
    pub fn new(media_id: MediaId, ice_control: mpsc::UnboundedSender<PeerICEConnectionControl>) -> Self {
        MediaChannelAudio {
            base: MediaChannelRtpBased::new(SdpMediaValue::Audio, media_id, ice_control)
        }
    }
}

impl Deref for MediaChannelAudio {
    type Target = MediaChannelRtpBased;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl DerefMut for MediaChannelAudio {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.base
    }
}

impl MediaChannel for MediaChannelAudio {
    fn as_typed(&mut self) -> TypedMediaChannel {
        TypedMediaChannel::Audio(self)
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