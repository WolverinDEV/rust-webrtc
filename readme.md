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