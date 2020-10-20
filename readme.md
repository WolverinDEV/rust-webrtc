# WebRTC-Rust
WebRTC-Rust is a WebRTC implementation in Rust, depending on `libnice`, `srtp` and `usrsctp`.  

## Features
- Application channels (aka. Data Channels)
  - Dynamically open new channels
  - Dynamically close remote and local channels
  - Dynamically receive remote channels
- Media streams (Audio and Video)
  - Outgoing (Send)
    - Create streams on the fly with an automatic negotiation algorithm
    - Automatically respond to retransmission request from the peer (Using nack)
  - Incoming (Receiving)
    - Receive audio and video data
    - Automatically request lost packets
- Full session renegotiation support
- Full control of received and send data
- No unwanted overhead (e.g. Video de/encoders)

# Building
## Windows (Currently notes only)
Install OpenSSL via vcpkg: `vcpkg install openssl:x64-windows-static-md`

Required libraries:
- ffi-7.dll
- gio-2.0-0.ddl
- glib-2.0-0.dll
- gmodule-2.0-0.dll
- gobject-2.0-0.dll
- intl.dll
- nice-10.ddl
- srtp2-1.dll
- usrsctp-1.dll

# Todos
## Application channels (DataChannel)  
- Allowing local channels to be created without a connected peer
- Dynamically request more data channels when exceeding the initial requested amount
- Proper handling in case of an SCTP shutdown
  
# Usefully RFCs
- [rfc6458](https://tools.ietf.org/html/rfc6458) Sockets API Extensions for the Stream Control Transmission Protocol (SCTP)
- [rfc5761](https://tools.ietf.org/html/rfc5761) Multiplexing RTP Data and Control Packets on a Single Port
- [rfc4317](https://tools.ietf.org/html/rfc4317) Session Description Protocol (SDP) Offer/Answer Examples
- [rfc3264](https://tools.ietf.org/html/rfc3264) An Offer/Answer Model with the Session Description Protocol (SDP)
- [rfc4566](https://tools.ietf.org/html/rfc4566) SDP: Session Description Protocol
- [rfc3550](https://tools.ietf.org/html/rfc3550) RTP: A Transport Protocol for Real-Time Applications
- [rfc4585](https://tools.ietf.org/html/rfc4585) Extended RTP Profile for Real-time Transport Control Protocol (RTCP)-Based Feedback (RTP/AVPF)

## Video
- [rfc7741](https://tools.ietf.org/html/rfc7741) RTP Payload Format for VP8 Video