import {VirtualCamera} from "./VirtualCamera";
import {patchLocalSdp, patchRemoteSdp} from "./SdpPatch";

let socket: WebSocket;
let peer: RTCPeerConnection;
let audioContext: AudioContext;
let audioElement: HTMLAudioElement;

interface WebCommand {
    RtcSetRemoteDescription: {
        sdp: string,
        mode: "offer" | "answer"
    },
    RtcAddIceCandidate: {
        media_index: number,
        candidate: string | undefined
    },
    RtcFinishedIceCandidates: {}
}

async function connect() {
    socket = new WebSocket("ws://localhost:1234/x?asdasd");
    await new Promise((resolve, reject) => {
        socket.onopen = resolve;
        socket.onerror = reject;
    });
    socket.onerror = () => console.error("WS-Error...");
    socket.onclose = event => console.log("WS-Disconnect: %d%s", event.code, event.reason ? ` (${event.reason})` : "");
    console.log("WS-Connected");

    socket.onmessage = event => handleMessage(event.data);
    //socket.close();
}

type ReceivedMessage<T extends keyof WebCommand> = { type: T, payload: WebCommand[T] };
async function handleMessage(data: any) {
    const message = JSON.parse(data) as ReceivedMessage<keyof WebCommand>;

    switch (message.type) {
        case "RtcAddIceCandidate": {
            const payload = message.payload as WebCommand["RtcAddIceCandidate"];
            if(!peer) { throw "Missing peer"; }
            peer.addIceCandidate(new RTCIceCandidate({
                candidate: payload.candidate,
                sdpMLineIndex: payload.media_index
            })).catch(error => console.error("[ICE] Failed to add candidate (%o)", error));
            break;
        }

        case "RtcSetRemoteDescription": {
            const payload = message.payload as WebCommand["RtcSetRemoteDescription"];
            if(!peer) { throw "Missing peer"; }

            let sdp = patchRemoteSdp(payload.sdp, payload.mode);
            console.log("[SDP] Received %s:\n%s", payload.mode, sdp);
            peer.setRemoteDescription(new RTCSessionDescription({
                sdp: sdp.replace(/\r?\n\r?\n/g, "\n"),
                type: payload.mode
            })).catch(error => {
                console.error("Failed to apply answer: %o", error);
            });
            if(payload.mode === "offer") {
                let answer = await peer.createAnswer();
                answer.sdp = patchLocalSdp(answer.sdp, "answer");
                await peer.setLocalDescription(answer);
                console.log("[SDP] Sending answer:\n%s", answer.sdp);
                sendCommand("RtcSetRemoteDescription", { sdp: answer.sdp, mode: "answer" });
            }
            break;
        }

        default:
            console.error(data);
    }
}
//http://localhost:9000/webpack-dev-server/
function sendCommand<T extends keyof WebCommand>(command: T, payload: WebCommand[T]) {
    socket.send(JSON.stringify({ type: command, payload: payload }));
}

function initializePeerApplication(peer: RTCPeerConnection) {
    {
        const channel = peer.createDataChannel("test", { ordered: false });
        channel.onopen = () => {
            console.log("[DC ] Channel test open");
            channel.send("Hello World");
            (window as any).testChannel = channel;
        };
        channel.onmessage = event => console.log("[DC ] Received message: %o", event.data);
        channel.onclose = () => console.log("[DC ] Channel test closed");
    }

    {
        const channel = peer.createDataChannel("test 1");
        //channel.onopen = () => { console.log("Closing %s", channel.label); channel.close(); }
        channel.onclose = () => console.log("[DC ] Channel test 1 closed");
    }
    /*
    setInterval(() => {
        if(peer.connectionState !== "connected") { return; }
        peer.createDataChannel("DC - " + Date.now());
    }, 1000);
    */

    peer.ondatachannel = event => {
        console.log("[DC ] Remote host opened data channel %s", event.channel.label);
    };
}

async function initializePeerAudio(peer: RTCPeerConnection) {
    const microphoneStream = await navigator.mediaDevices.getUserMedia({
        audio: {
            echoCancellation: false,
            noiseSuppression: false
        }
    });
    microphoneStream.getAudioTracks().forEach(track => {
        const sender = peer.addTrack(track);
        setInterval(() => {
            console.error("Track set to %o", sender.track ? null : track);
            sender.replaceTrack(sender.track ? null : track);
        }, 2000);
        (window as any).sender = sender;
    });
    //audioContext.createMediaStreamSource(microphoneStream).connect(audioContext.destination);
}

function showVideoStream(stream: MediaStream) {
    let element = document.createElement("video");
    element.width = 400;
    element.height = 300;
    element.style.border = "1px solid black";
    element.style.borderRadius = "1px";
    element.srcObject = stream;
    element.autoplay = true;
    document.body.append(element);
}

let virtualCamera: VirtualCamera;
async function createVirtualCameraStream() : Promise<MediaStream> {
    if(!virtualCamera) {
        let scale = .4;
        virtualCamera = new VirtualCamera(30, { height: 1024 * scale, width: 1024 * scale });
        virtualCamera.start();
        (window as any).virtualCamera = virtualCamera;
    }

    return virtualCamera.getMediaStream();
}

async function initializePeerVideo(peer: RTCPeerConnection) {
    let stream: MediaStream;
    try {
        stream = await navigator.mediaDevices.getUserMedia({
            video: true
        });
    } catch (error) {
        try {
            stream = await (navigator.mediaDevices as any).getDisplayMedia({
                video: true
            });
        } catch(error) {
            console.warn("Failed to get camera input, using virtual camera instead (%o)", error);
            stream = await createVirtualCameraStream();
        }
    }

    const track = stream.getVideoTracks()[0];

    const canvas = document.createElement("canvas");
    canvas.getContext("2d");
    const cstream = canvas.captureStream(1);
    const transceiver = peer.addTransceiver(cstream.getVideoTracks()[0]);

    //track.enabled = false;
    setTimeout(() => {
        track.enabled = false;
        transceiver.sender.replaceTrack(track).then(() => {
            console.error("Track replaced 2");
            console.error(track.enabled);
            console.error(transceiver.currentDirection);
            track.enabled = true;
        });
    }, 2000);

    //const transceiver = peer.addTransceiver(stream.getVideoTracks()[0]);
    //transceiver.direction = "sendrecv";
    //(window as any).tr = transceiver;
    //let sender = peer.addTrack(stream.getVideoTracks()[0]);
    //showVideoStream(stream);

    /*
    setTimeout(() => {
        peer.addTrack(stream.getVideoTracks()[0].clone());
    }, 5000);
    */
    /*
    setTimeout(() => {
        console.log("Removing sender");
        peer.removeTrack(sender);
        setTimeout(() => {
            peer.addTrack(stream.getVideoTracks()[0]);

            setTimeout(() => {
                let sender = peer.addTrack(stream.getVideoTracks()[0].clone());

                setTimeout(() => {
                    peer.removeTrack(sender);

                    setTimeout(() => {
                        let sender = peer.addTrack(stream.getVideoTracks()[0].clone());
                    }, 5000);
                }, 5000);
            }, 5000);
        }, 5000);
    }, 5000);
    */
    /*
    setTimeout(() => {
        const clone = stream.getVideoTracks()[0].clone();
        peer.addTrack(clone);
        setTimeout(async () => {
            console.log("Track end");
            peer.removeTrack(sender);
            setTimeout(async () => {
                console.log("Add new track");

                stream = await createVirtualCameraStream();
                peer.addTrack(stream.getVideoTracks()[0].clone());

                setTimeout(() => {
                    console.log("Add new track 2");
                    peer.addTrack(stream.getVideoTracks()[0].clone());
                }, 5000);
            }, 5000);
        }, 5000);
    }, 5000);
    */
    //audioContext.createMediaStreamSource(microphoneStream).connect(audioContext.destination);
}

async function initializePeer() {
    peer = new RTCPeerConnection({
        iceServers: [ { urls: ["stun:stun.l.google.com:19302"] } ]
    });
    (window as any).peer = peer;

    peer.oniceconnectionstatechange = () => console.log("[ICE] Connection state changed to %s", peer.iceConnectionState);
    peer.onicegatheringstatechange = () => console.log("[ICE] Gathering state changed to %s", peer.iceGatheringState);
    peer.onconnectionstatechange = () => console.log("[PC ] Connection state changed to %s", peer.connectionState);
    peer.onicecandidateerror = event => console.warn("[ICE] Candidate error %d/%s for %s", event.errorCode, event.errorText, event.url);
    peer.onsignalingstatechange = () => console.log("[PC ] Signalling state changed to %s", peer.signalingState);

    peer.onicecandidate = event => {
        //if(event.candidate?.protocol !== "tcp") { return; }
        console.log("[ICE] Found local ICE candidate: %o", event.candidate);
        if(event.candidate?.candidate) {
            sendCommand("RtcAddIceCandidate", {
                media_index: event.candidate.sdpMLineIndex,
                candidate: event.candidate.candidate
            });
        } else {
            sendCommand("RtcFinishedIceCandidates", {});
        }
    }

    peer.ontrack = event => {
        const stream = event.streams[0];
        if(!stream) {
            /* tracks sadly don't have any reliably names, but streams could have */
            console.warn("Received track without a video stream.");
            return;
        }
        console.log("[AUD] Received remote %s track %s (%s) %s", event.track.kind, event.track.id, event.track.label, event.streams[0]?.id);
        event.track.onmute = () => console.log("[AUD] Muted %s", event.track.id);
        event.track.onunmute = () => console.log("[AUD] Unmute %s", event.track.id);
        event.track.onended = () => console.log("[AUD] Ended %s", event.track.id);
        event.track.onisolationchange = () => console.log("[AUD] Isolationchange %s", event.track.id);
        if(event.track.kind === "audio") {
            console.error("Streams: %o", event.streams);

            const mstream = new MediaStream();
            (window as any).track = event.track;
            mstream.addTrack(event.track);
            let stream = audioContext.createMediaStreamSource(mstream);
            stream.connect(audioContext.destination);

            audioElement = new Audio();
            document.body.append(audioElement);
            audioElement.srcObject = mstream;
            audioElement.autoplay = true;
            audioElement.muted = true;
            (window as any).audioElement = audioElement;
        } else if(event.track.kind === "video") {
            console.error("Received video track");
            let str = new MediaStream();
            str.addTrack(event.track);
            showVideoStream(str);
        }
    };

    await initializePeerApplication(peer);

    const kEnableAudio = false;
    if(kEnableAudio) {
        await initializePeerAudio(peer);
    }

    const kEnableVideo = true;
    if(kEnableVideo) {
        await initializePeerVideo(peer);
    }

    const offer = await peer.createOffer({
        offerToReceiveAudio: kEnableAudio,
        offerToReceiveVideo: kEnableVideo
    });
    await peer.setLocalDescription(offer);

    console.log("[SDP] Offer:\n%s", offer.sdp);
    sendCommand("RtcSetRemoteDescription", { sdp: patchLocalSdp(offer.sdp, "offer"), mode: "offer" });

    /* if something changes, signal it to the remote */
    peer.onnegotiationneeded = async () => {
        console.error("Nego needed");
        return;

        const offer = await peer.createOffer({
            offerToReceiveAudio: kEnableAudio,
            offerToReceiveVideo: kEnableVideo
        });
        offer.sdp = patchLocalSdp(offer.sdp, "offer");
        await peer.setLocalDescription(offer);

        console.log("[SDP] Offer (Nego):\n%s", offer.sdp);
        sendCommand("RtcSetRemoteDescription", { sdp: patchLocalSdp(offer.sdp, "offer"), mode: "offer" });
    }
}

const kAutoReloadRequest = false;
async function main() {
    audioContext = new AudioContext();
    (window as any).audioContext = audioContext;

    if(audioContext.state === "suspended" && !kAutoReloadRequest) {
        await new Promise((resolve, reject) => {
            console.error("CLICK SOMEWHERE ON THE PAGE TO CONTINUE!");
            const callback = () => {
                console.error("Resume");
                document.removeEventListener("mousedown", callback);
                audioContext.resume().then(resolve).catch(reject);
            };
            document.addEventListener("mousedown", callback);
        });
    }

    await connect();
    await initializePeer();

    if(false) {
        const microphoneStream = await navigator.mediaDevices.getUserMedia({
            audio: {
                echoCancellation: false,
                noiseSuppression: false
            }
        });
        console.error("Have mic");
        const dest = audioContext.createMediaStreamDestination();
        audioContext.createMediaStreamSource(microphoneStream).connect(dest);

        const target = document.createElement("audio");
        target.autoplay = true;
        target.onpause = () => console.error("Audio pause");
        target.onloadstart = () => console.error("Start load");
        target.onplay = () => console.error("Start play");
        target.srcObject = dest.stream;
        document.body.append(target);

        console.error(dest.stream.getAudioTracks()[0].getCapabilities());
    }
}
main();

if(kAutoReloadRequest) {
    setTimeout(() => {
        location.href = location.toString();
    }, 3000);
}