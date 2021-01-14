import * as sdpTransform from "sdp-transform";

const H264_PAYLOAD_TYPE = 105;
const kVideoCodecs = [
    {
        payload: H264_PAYLOAD_TYPE,
        codec: "H264",
        rate: 90000,
        rtcpFb: [ "nack", "nack pli", "ccm fir", "transport-cc" ],
        //42001f | Original: 42e01f
        fmtp: {
            "level-asymmetry-allowed": 1,
            "packetization-mode": 1,
            "profile-level-id": "42e01f",
            "max-fr": 30,
        }
    }
];

export function patchLocalSdp(sdpString: string, mode: "offer" | "answer") : string {
    const sdp = sdpTransform.parse(sdpString);

    /* apply the "root" fingerprint to each media, FF fix */
    if(sdp.fingerprint) {
        sdp.media.forEach(media => media.fingerprint = sdp.fingerprint);
    }

    patchCodecs(sdp);

    return sdpTransform.write(sdp);
}

export function patchRemoteSdp(sdp: string, mode: "offer" | "answer") : string {
    sdp = sdp.replace("\r\n\r\n", "\r\n");

    let parsedSdp = sdpTransform.parse(sdp);
    patchCodecs(parsedSdp);
    return sdpTransform.write(parsedSdp);
}

function patchCodecs(sdp: sdpTransform.SessionDescription) {
    for(let media of sdp.media) {
        if(media.type !== "video") {
            continue;
        }

        media.fmtp = [];
        media.rtp = [];
        media.rtcpFb = [];
        media.rtcpFbTrrInt = [];

        for(let codec of kVideoCodecs) {
            media.rtp.push({
                payload: codec.payload,
                codec: codec.codec,
                encoding: undefined,
                rate: codec.rate
            });

            codec.rtcpFb?.forEach(fb => media.rtcpFb.push({
                payload: codec.payload,
                type: fb
            }));

            if(codec.fmtp && Object.keys(codec.fmtp).length > 0) {
                media.fmtp.push({
                    payload: codec.payload,
                    config: Object.keys(codec.fmtp).map(e => e + "=" + codec.fmtp[e]).join(";")
                });
                if(media.type === "audio") {
                    media.maxptime = media.fmtp["maxptime"];
                }
            }

            // if(window.detectedBrowser.name === "firefox") {
            //     /*
            //      * Firefox does not support multiple payload formats not switching between them.
            //      * This causes us only to add one, the primary, codec and hope for the best
            //      * (Opus Stereo and Mono mixing seem to work right now 11.2020)
            //      */
            //     /* TODO: Test this again since we're not sending a end signal via RTCP. This might change the behaviour? */
            //     break;
            // }
        }

        media.payloads = media.rtp.map(e => e.payload).join(" ");
    }
}