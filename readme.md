# Windows
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

# TODOs
## Application channels (DataChannel)  
- Allowing local channels to be created without a connected peer
- Dynamically request more data channels when exceeding the initial requested amount
- Proper handling in case of an SCTP shutdown
- Add support for not bundled media streams (Mozilla than required an `o=...` tag for each individual stream!)
  Note: Prevent application channel bundling! (WebRTC does not uses this, but it's theoretically possible)
  
  
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