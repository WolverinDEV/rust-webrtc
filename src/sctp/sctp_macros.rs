/*
#define (\S+)(\s+)([\dxa-f]+)
pub const $1: u32$2 = $3;
 */

pub const AF_CONN: i32 = 123;
pub const IPPROTO_SCTP: i32 = 132; /* This is the IANA assigned protocol number of SCTP. */
pub const SOCK_STREAM: i32 = 1;
pub const MSG_NOTIFICATION: u32 = 0x2000;

pub const SCTP_FUTURE_ASSOC: u32   = 0;
pub const SCTP_CURRENT_ASSOC: u32  = 1;
pub const SCTP_ALL_ASSOC: u32      = 2;


/* Flags that go into the sinfo->sinfo_flags field */
/// tail part of the message could not be sent
pub const SCTP_DATA_LAST_FRAG   : u16 = 0x0001;
/// complete message could not be sent
pub const SCTP_DATA_NOT_FRAG    : u16 = 0x0003;
/// next message is a notification
pub const SCTP_NOTIFICATION     : u16 = 0x0010;
/// next message is complete
pub const SCTP_COMPLETE         : u16 = 0x0020;
/// Start shutdown procedures
pub const SCTP_EOF              : u16 = 0x0100;
/// Send an ABORT to peer
pub const SCTP_ABORT            : u16 = 0x0200;
/// Message is un-ordered
pub const SCTP_UNORDERED        : u16 = 0x0400;
/// Override the primary-address
pub const SCTP_ADDR_OVER        : u16 = 0x0800;
/// Send this on all associations
pub const SCTP_SENDALL          : u16 = 0x1000;
/// end of message signal
pub const SCTP_EOR              : u16 = 0x2000;
///Set I-Bit
pub const SCTP_SACK_IMMEDIATELY : u16 = 0x4000;

/*
 * user socket options: socket API defined
 */
/*
 * read-write options
 */
pub const SCTP_RTOINFO: i32                     = 0x00000001;
pub const SCTP_ASSOCINFO: i32                   = 0x00000002;
pub const SCTP_INITMSG: i32                     = 0x00000003;
pub const SCTP_NODELAY: i32                     = 0x00000004;
pub const SCTP_AUTOCLOSE: i32                   = 0x00000005;
pub const SCTP_PRIMARY_ADDR: i32                = 0x00000007;
pub const SCTP_ADAPTATION_LAYER: i32            = 0x00000008;
pub const SCTP_DISABLE_FRAGMENTS: i32           = 0x00000009;
pub const SCTP_PEER_ADDR_PARAMS: i32            = 0x0000000a;
/* ancillary data/notification interest options */
/* Without this applied we will give V4 and V6 addresses on a V6 socket */
pub const SCTP_I_WANT_MAPPED_V4_ADDR: i32       = 0x0000000d;
pub const SCTP_MAXSEG: i32                      = 0x0000000e;
pub const SCTP_DELAYED_SACK: i32                = 0x0000000f;
pub const SCTP_FRAGMENT_INTERLEAVE: i32         = 0x00000010;
pub const SCTP_PARTIAL_DELIVERY_POINT: i32      = 0x00000011;
/* authentication support */
pub const SCTP_HMAC_IDENT: i32                  = 0x00000014;
pub const SCTP_AUTH_ACTIVE_KEY: i32             = 0x00000015;
pub const SCTP_AUTO_ASCONF: i32                 = 0x00000018;
pub const SCTP_MAX_BURST: i32                   = 0x00000019;
/* assoc level context */
pub const SCTP_CONTEXT: i32                     = 0x0000001a;
/* explicit EOR signalling */
pub const SCTP_EXPLICIT_EOR: i32                = 0x0000001b;
pub const SCTP_REUSE_PORT: i32                  = 0x0000001c;

pub const SCTP_EVENT: i32                       = 0x0000001e;
pub const SCTP_RECVRCVINFO: i32                 = 0x0000001f;
pub const SCTP_RECVNXTINFO: i32                 = 0x00000020;
pub const SCTP_DEFAULT_SNDINFO: i32             = 0x00000021;
pub const SCTP_DEFAULT_PRINFO: i32              = 0x00000022;
pub const SCTP_REMOTE_UDP_ENCAPS_PORT: i32      = 0x00000024;
pub const SCTP_ECN_SUPPORTED: i32               = 0x00000025;
pub const SCTP_PR_SUPPORTED: i32                = 0x00000026;
pub const SCTP_AUTH_SUPPORTED: i32              = 0x00000027;
pub const SCTP_ASCONF_SUPPORTED: i32            = 0x00000028;
pub const SCTP_RECONFIG_SUPPORTED: i32          = 0x00000029;
pub const SCTP_NRSACK_SUPPORTED: i32            = 0x00000030;
pub const SCTP_PKTDROP_SUPPORTED: i32           = 0x00000031;
pub const SCTP_MAX_CWND: i32                    = 0x00000032;

pub const SCTP_ENABLE_STREAM_RESET: i32         = 0x00000900; /* struct sctp_assoc_value */

/* Pluggable Stream Scheduling Socket option */
pub const SCTP_PLUGGABLE_SS: i32                = 0x00001203;
pub const SCTP_SS_VALUE: i32                    = 0x00001204;

/*
 * read-only options
 */
pub const SCTP_STATUS: i32                      = 0x00000100;
pub const SCTP_GET_PEER_ADDR_INFO: i32          = 0x00000101;
/* authentication support */
pub const SCTP_PEER_AUTH_CHUNKS: i32            = 0x00000102;
pub const SCTP_LOCAL_AUTH_CHUNKS: i32           = 0x00000103;
pub const SCTP_GET_ASSOC_NUMBER: i32            = 0x00000104;
pub const SCTP_GET_ASSOC_ID_LIST: i32           = 0x00000105;
pub const SCTP_TIMEOUTS: i32                    = 0x00000106;
pub const SCTP_PR_STREAM_STATUS: i32            = 0x00000107;
pub const SCTP_PR_ASSOC_STATUS: i32             = 0x00000108;

/*
 * write-only options
 */
pub const SCTP_SET_PEER_PRIMARY_ADDR: i32       = 0x00000006;
pub const SCTP_AUTH_CHUNK: i32                  = 0x00000012;
pub const SCTP_AUTH_KEY: i32                    = 0x00000013;
pub const SCTP_AUTH_DEACTIVATE_KEY: i32         = 0x0000001d;
pub const SCTP_AUTH_DELETE_KEY: i32             = 0x00000016;
pub const SCTP_RESET_STREAMS: i32               = 0x00000901; /* struct sctp_reset_streams */
pub const SCTP_RESET_ASSOC: i32                 = 0x00000902; /* sctp_assoc_t */
pub const SCTP_ADD_STREAMS: i32                 = 0x00000903; /* struct sctp_add_streams */

pub const SCTP_SENDV_NOINFO:   u32 = 0;
pub const SCTP_SENDV_SNDINFO:  u32 = 1;
pub const SCTP_SENDV_PRINFO:   u32 = 2;
pub const SCTP_SENDV_AUTHINFO: u32 = 3;
pub const SCTP_SENDV_SPA:      u32 = 4;

pub const SPP_HB_ENABLE       :u32 = 0x00000001;
pub const SPP_HB_DISABLE      :u32 = 0x00000002;
pub const SPP_HB_DEMAND       :u32 = 0x00000004;
pub const SPP_PMTUD_ENABLE    :u32 = 0x00000008;
pub const SPP_PMTUD_DISABLE   :u32 = 0x00000010;
pub const SPP_HB_TIME_IS_ZERO :u32 = 0x00000080;
pub const SPP_IPV6_FLOWLABEL  :u32 = 0x00000100;
pub const SPP_DSCP            :u32 = 0x00000200;

/* Flags used for the stream_reset_event */
pub const SCTP_STREAM_RESET_INCOMING_SSN: i32   = 0x0001;
pub const SCTP_STREAM_RESET_OUTGOING_SSN: i32   = 0x0002;
pub const SCTP_STREAM_RESET_DENIED: i32         = 0x0004; /* SCTP_STRRESET_FAILED */
pub const SCTP_STREAM_RESET_FAILED: i32         = 0x0008; /* SCTP_STRRESET_FAILED */
pub const SCTP_STREAM_CHANGED_DENIED: i32       = 0x0010;

pub const SCTP_STREAM_RESET_INCOMING: i32       = 0x00000001;
pub const SCTP_STREAM_RESET_OUTGOING: i32       = 0x00000002;

pub const SCTP_COMM_UP: i32         = 0x0001;
pub const SCTP_COMM_LOST: i32       = 0x0002;
pub const SCTP_RESTART: i32         = 0x0003;
pub const SCTP_SHUTDOWN_COMP: i32   = 0x0004;
pub const SCTP_CANT_STR_ASSOC: i32  = 0x0005;