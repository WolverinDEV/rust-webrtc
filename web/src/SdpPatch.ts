import * as sdpTransform from "sdp-transform";

export function patchLocalSdp(sdpString: string, mode: "offer" | "answer") : string {
    const sdp = sdpTransform.parse(sdpString);

    /* apply the "root" fingerprint to each media, FF fix */
    if(sdp.fingerprint) {
        sdp.media.forEach(media => media.fingerprint = sdp.fingerprint);
    }

    return sdpTransform.write(sdp);
}

export function patchRemoteSdp(sdp: string, mode: "offer" | "answer") : string {
    sdp = sdp.replace("\r\n\r\n", "\r\n");
    return sdp;
}