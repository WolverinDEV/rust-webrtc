use std::io::{Result, Error, ErrorKind, Cursor};
use byteorder::{ReadBytesExt, LittleEndian};
use crate::sctp::sctp_macros::{SCTP_COMM_UP, SCTP_COMM_LOST, SCTP_RESTART, SCTP_SHUTDOWN_COMP, SCTP_CANT_STR_ASSOC};

#[derive(Debug, PartialEq)]
pub struct SctpNotificationStreamReset {
    /// See `SCTP_STREAM_*` elements within sctp_macros
    pub flags: u16,
    pub assoc_id: u32,
    pub streams: Vec<u16>
}

impl SctpNotificationStreamReset {
    pub fn parse(cursor: &mut Cursor<&[u8]>) -> Result<SctpNotificationStreamReset> {
        if cursor.read_u16::<LittleEndian>()? != SctpNotificationType::StreamResetEvent.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "expected notification type stream reset"));
        }

        let flags = cursor.read_u16::<LittleEndian>()?;
        let length = cursor.read_u32::<LittleEndian>()?;
        let assoc_id = cursor.read_u32::<LittleEndian>()?;
        let element_count = (length - 12) / 2; /* 12 is the size of the own header */
        let mut streams = Vec::with_capacity(element_count as usize);
        for _ in 0..element_count {
            streams.push(cursor.read_u16::<LittleEndian>()?);
        }

        Ok(SctpNotificationStreamReset {
            flags,
            assoc_id,
            streams
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum SctpSacState {
    CommUp,
    CommLost,
    Restart,
    ShutdownComp,
    CantStrAssoc
}

impl SctpSacState {
    pub fn value(&self) -> i32 {
        match self {
            SctpSacState::CommUp => SCTP_COMM_UP,
            SctpSacState::CommLost => SCTP_COMM_LOST,
            SctpSacState::Restart => SCTP_RESTART,
            SctpSacState::ShutdownComp => SCTP_SHUTDOWN_COMP,
            SctpSacState::CantStrAssoc => SCTP_CANT_STR_ASSOC,
        }
    }

    pub fn from(value: i32) -> Option<SctpSacState> {
        match value {
            SCTP_COMM_UP => Some(SctpSacState::CommUp),
            SCTP_COMM_LOST => Some(SctpSacState::CommLost),
            SCTP_RESTART => Some(SctpSacState::Restart),
            SCTP_SHUTDOWN_COMP => Some(SctpSacState::ShutdownComp),
            SCTP_CANT_STR_ASSOC => Some(SctpSacState::CantStrAssoc),
            _ => None
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct SctpNotificationAssocChange {
    pub flags: u16,
    pub state: SctpSacState,
    pub error: u16,
    pub outbound_streams: u16,
    pub inbound_streams: u16,
    pub assoc_id: u32,
    /* Not yet supported */
    /* pub info: Vec<u8> */
}

impl SctpNotificationAssocChange {
    pub fn parse(cursor: &mut Cursor<&[u8]>) -> Result<SctpNotificationAssocChange> {
        if cursor.read_u16::<LittleEndian>()? != SctpNotificationType::AssocChange.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "expected notification type stream reset"));
        }

        let flags = cursor.read_u16::<LittleEndian>()?;
        let _length = cursor.read_u32::<LittleEndian>()?;
        let state = SctpSacState::from(cursor.read_u16::<LittleEndian>()? as i32)
            .ok_or(Error::new(ErrorKind::InvalidInput, "invalid sac state"))?;
        let error = cursor.read_u16::<LittleEndian>()?;
        let outbound_streams = cursor.read_u16::<LittleEndian>()?;
        let inbound_streams = cursor.read_u16::<LittleEndian>()?;
        let assoc_id = cursor.read_u32::<LittleEndian>()?;

        Ok(SctpNotificationAssocChange {
            flags,
            state,
            error,
            outbound_streams,
            inbound_streams,
            assoc_id
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum SctpNotificationType {
    AssocChange,
    PeerAddrChange,
    RemoteError,
    SendFailed,
    ShutdownEvent,
    AdaptationIndication,
    PartialDeliveryEvent,
    AuthenticationEvent,
    StreamResetEvent,
    SenderDryEvent,
    NotificationsStoppedEvent,
    AssocResetEvent,
    StreamChangeEvent,
    SendFailedEvent
}

impl SctpNotificationType {
    pub fn value(&self) -> u16 {
        match self {
            SctpNotificationType::AssocChange => 0x0001,
            SctpNotificationType::PeerAddrChange => 0x0002,
            SctpNotificationType::RemoteError => 0x0003,
            SctpNotificationType::SendFailed => 0x0004,
            SctpNotificationType::ShutdownEvent => 0x0005,
            SctpNotificationType::AdaptationIndication => 0x0006,
            SctpNotificationType::PartialDeliveryEvent => 0x0007,
            SctpNotificationType::AuthenticationEvent => 0x0008,
            SctpNotificationType::StreamResetEvent => 0x0009,
            SctpNotificationType::SenderDryEvent => 0x000A,
            SctpNotificationType::NotificationsStoppedEvent => 0x000B,
            SctpNotificationType::AssocResetEvent => 0x000C,
            SctpNotificationType::StreamChangeEvent => 0x000D,
            SctpNotificationType::SendFailedEvent => 0x000e
        }
    }

    pub fn from(value: u16) -> Option<SctpNotificationType> {
        match value {
            0x0001 => Some(SctpNotificationType::AssocChange),
            0x0002 => Some(SctpNotificationType::PeerAddrChange),
            0x0003 => Some(SctpNotificationType::RemoteError),
            0x0004 => Some(SctpNotificationType::SendFailed),
            0x0005 => Some(SctpNotificationType::ShutdownEvent),
            0x0006 => Some(SctpNotificationType::AdaptationIndication),
            0x0007 => Some(SctpNotificationType::PartialDeliveryEvent),
            0x0008 => Some(SctpNotificationType::AuthenticationEvent),
            0x0009 => Some(SctpNotificationType::StreamResetEvent),
            0x000A => Some(SctpNotificationType::SenderDryEvent),
            0x000B => Some(SctpNotificationType::NotificationsStoppedEvent),
            0x000C => Some(SctpNotificationType::AssocResetEvent),
            0x000D => Some(SctpNotificationType::StreamChangeEvent),
            0x000E => Some(SctpNotificationType::SendFailedEvent),
            _ => None
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum SctpNotification {
    StreamReset(SctpNotificationStreamReset),
    AssocChange(SctpNotificationAssocChange),
    Generic(Vec<u8>)
}

impl SctpNotification {
    pub fn parse(buffer: &[u8]) -> Result<SctpNotification> {
        let mut cursor = Cursor::new(buffer);
        let initial_position = cursor.position();
        let notification_type = SctpNotificationType::from(cursor.read_u16::<LittleEndian>()?)
            .ok_or(Error::new(ErrorKind::InvalidInput, "invalid sctp notification type"))?;
        cursor.set_position(initial_position);

        match notification_type {
            SctpNotificationType::StreamResetEvent => Ok(SctpNotification::StreamReset(SctpNotificationStreamReset::parse(&mut cursor)?)),
            SctpNotificationType::AssocChange => Ok(SctpNotification::AssocChange(SctpNotificationAssocChange::parse(&mut cursor)?)),
            _ => Ok(SctpNotification::Generic(Vec::from(buffer)))
        }
    }

}