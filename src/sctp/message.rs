use byteorder::{ReadBytesExt, BigEndian, WriteBytesExt};
use std::io::{Cursor, Error, Result, ErrorKind, Read, Write};

/// This enum specifies the data channel types which
/// are defined in [draft-ietf-rtcweb-data-protocol-08](https://tools.ietf.org/html/draft-ietf-rtcweb-data-protocol-08#section-8.2.2)
#[derive(PartialOrd, PartialEq, Debug, Copy, Clone, Eq)]
pub enum DataChannelType {
    /// The Data Channel provides a reliable in-order bi-directional communication.
    Reliable,

    /// The Data Channel provides a reliable unordered bi-directional communication.
    ReliableUnordered,

    /// The Data Channel provides a partially-reliable in-order bi-directional
    /// communication. User messages will not be retransmitted more
    /// times than specified in the Reliability Parameter.
    PartialReliableRexmit(u32),

    /// The Data Channel provides a partial reliable unordered bi-directional
    /// communication. User messages will not be retransmitted more
    /// times than specified in the Reliability Parameter.
    PartialReliableRexmitUnordered(u32),

    /// The Data Channel provides a partial reliable in-order bi-directional
    /// communication. User messages might not be transmitted or
    /// retransmitted after a specified life-time given in milliseconds
    /// in the Reliability Parameter. This life-time starts when
    /// providing the user message to the protocol stack.
    PartialReliableTimed(u32),

    /// The Data Channel provides a partial reliable unordered bi-directional
    /// communication. User messages might not be transmitted or
    /// retransmitted after a specified life-time given in milliseconds
    /// in the Reliability Parameter. This life-time starts when
    /// providing the user message to the protocol stack.
    PartialReliableTimedUnordered(u32)
}

impl DataChannelType {
    /// Returns the assigned type value
    pub fn value(&self) -> u8 {
        match self {
            DataChannelType::Reliable                          => 0x00,
            DataChannelType::ReliableUnordered                 => 0x80,
            DataChannelType::PartialReliableRexmit(_)          => 0x01,
            DataChannelType::PartialReliableRexmitUnordered(_) => 0x81,
            DataChannelType::PartialReliableTimed(_)           => 0x02,
            DataChannelType::PartialReliableTimedUnordered(_)  => 0x82,
        }
    }

    /// Returns whatever the channel type is ordered
    pub fn is_ordered(&self) -> bool {
        match self {
            DataChannelType::Reliable                          => true,
            DataChannelType::ReliableUnordered                 => false,
            DataChannelType::PartialReliableRexmit(_)          => true,
            DataChannelType::PartialReliableRexmitUnordered(_) => false,
            DataChannelType::PartialReliableTimed(_)           => true,
            DataChannelType::PartialReliableTimedUnordered(_)  => false,
        }
    }

    /// Returns the value for the reliability parameter.
    pub fn reliability_parameter(&self) -> u32 {
        match self {
            DataChannelType::Reliable                          => 0,
            DataChannelType::ReliableUnordered                 => 0,
            DataChannelType::PartialReliableRexmit(p)          => *p,
            DataChannelType::PartialReliableRexmitUnordered(p) => *p,
            DataChannelType::PartialReliableTimed(p)           => *p,
            DataChannelType::PartialReliableTimedUnordered(p)  => *p,
        }
    }

    /// Parse the type value
    fn from(value: u8, reliability_parameter: u32) -> Option<Self> {
        match value {
            0x00u8 => Some(DataChannelType::Reliable),
            0x80u8 => Some(DataChannelType::ReliableUnordered),
            0x01u8 => Some(DataChannelType::PartialReliableRexmit(reliability_parameter)),
            0x81u8 => Some(DataChannelType::PartialReliableRexmitUnordered(reliability_parameter)),
            0x02u8 => Some(DataChannelType::PartialReliableTimed(reliability_parameter)),
            0x82u8 => Some(DataChannelType::PartialReliableTimedUnordered(reliability_parameter)),
            _ => None
        }
    }
}

/// Parsed DataChannel open control message as described in
/// https://tools.ietf.org/html/draft-ietf-rtcweb-data-protocol-08#section-5.1
#[derive(PartialEq, Debug)]
pub struct DataChannelControlMessageOpen {
    /// This field specifies the type of the Data Channel to be opened
    /// and contains, if required the reliability parameter
    pub channel_type: DataChannelType,
    /// The priority of the Data Channel
    pub priority: u16,
    /// The name of the Data Channel as a UTF-8 encoded string.
    /// This may be an empty string.
    pub label: String,
    /// If this is an empty string the protocol is unspecified.
    /// If it is a non-empty string, it specifies a protocol registered in the
    /// 'WebSocket Subprotocol Name Registry' created in [RFC6455](https://tools.ietf.org/html/rfc6455).
    /// This string is UTF-8 encoded.
    pub protocol: String
}

impl DataChannelControlMessageOpen {
    /// Parsing the buffer as a `DataChannelControlMessageOpen`
    pub fn parse(buffer: &[u8]) -> Result<DataChannelControlMessageOpen> {
        let mut reader = Cursor::new(buffer);
        if reader.read_u8()? != DataChannelControlMessageType::Open.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "message is not an open request"));
        }

        let channel_type = reader.read_u8()?;
        let priority = reader.read_u16::<BigEndian>()?;
        let reliability_parameter = reader.read_u32::<BigEndian>()?;
        let label_length = reader.read_u16::<BigEndian>()? as usize;
        let protocol_length = reader.read_u16::<BigEndian>()? as usize;

        let mut label_buffer = Vec::new();
        label_buffer.resize(label_length, 0);
        reader.read_exact(&mut label_buffer)?;

        let mut protocol_buffer = Vec::new();
        protocol_buffer.resize(protocol_length, 0);
        reader.read_exact(&mut protocol_buffer)?;


        let channel_type = DataChannelType::from(channel_type, reliability_parameter)
            .ok_or(Error::new(ErrorKind::InvalidInput, "invalid data channel type"))?;

        Ok(DataChannelControlMessageOpen {
            channel_type,
            priority,
            label: String::from_utf8(label_buffer).map_err(|_| Error::new(ErrorKind::InvalidInput, "label isn't UTF-8"))?,
            protocol: String::from_utf8(protocol_buffer).map_err(|_| Error::new(ErrorKind::InvalidInput, "protocol isn't UTF-8"))?
        })
    }

    /// Returns the expected length in bytes
    pub fn expected_length(&self) -> usize {
        12 + self.label.len() + self.protocol.len()
    }

    /// Create/Write the data channel open packet to the target buffer
    pub fn write(&self, target: &mut [u8]) -> Result<usize> {
        let mut writer = Cursor::new(target);
        writer.write_u8(DataChannelControlMessageType::Open.value())?;
        writer.write_u8(self.channel_type.value())?;
        writer.write_u16::<BigEndian>(self.priority)?;
        writer.write_u32::<BigEndian>(self.channel_type.reliability_parameter())?;
        writer.write_u16::<BigEndian>(self.label.len() as u16)?;
        writer.write_u16::<BigEndian>(self.protocol.len() as u16)?;
        writer.write(self.label.as_bytes())?;
        writer.write(self.protocol.as_bytes())?;
        Ok(writer.position() as usize)
    }
}

/// Parsed DataChannel open ack control message as described in
/// https://tools.ietf.org/html/draft-ietf-rtcweb-data-protocol-08#section-5.2
#[derive(PartialEq, Debug)]
pub struct DataChannelControlMessageOpenAck { }

impl DataChannelControlMessageOpenAck {
    /// Parsing the buffer as a `DataChannelControlMessageOpen`
    pub fn parse(buffer: &[u8]) -> Result<DataChannelControlMessageOpenAck> {
        let mut rdr = Cursor::new(buffer);
        if rdr.read_u8()? != DataChannelControlMessageType::OpenAck.value() {
            return Err(Error::new(ErrorKind::InvalidInput, "message is not an open ack"));
        }

        Ok(DataChannelControlMessageOpenAck{})
    }

    /// Returns the expected length in bytes
    pub fn expected_length(&self) -> usize {
        1
    }

    /// Create/Write the data channel open ack packet to the target buffer
    pub fn write(&self, target: &mut [u8]) -> Result<usize> {
        let mut writer = Cursor::new(target);
        writer.write_u8(DataChannelControlMessageType::OpenAck.value())?;
        Ok(writer.position() as usize)
    }
}

/// This enum contains all registered DataChannel control message types as listed in
/// https://tools.ietf.org/html/draft-ietf-rtcweb-data-protocol-08#section-8.2.1
#[derive(PartialOrd, PartialEq, Debug, Copy, Clone, Eq)]
pub enum DataChannelControlMessageType {
    Open,
    OpenAck
}

impl DataChannelControlMessageType {
    /// Return the type as u8
    pub fn value(&self) -> u8 {
        match self {
            DataChannelControlMessageType::Open    => 0x03,
            DataChannelControlMessageType::OpenAck => 0x02,
        }
    }

    /// Parse the type from an u8.
    /// If the type does not match any known message type,
    /// `None` will be returned.
    fn from(value: u8) -> Option<Self> {
        match value {
            0x03 => Some(DataChannelControlMessageType::Open),
            0x02 => Some(DataChannelControlMessageType::OpenAck),
            _ => None
        }
    }
}

/// Container for all parsed DataChannel control messages
#[derive(PartialEq, Debug)]
pub enum DataChannelControlMessage {
    Open(DataChannelControlMessageOpen),
    OpenAck(DataChannelControlMessageOpenAck)
}

impl DataChannelControlMessage {
    /// Parse the buffer as a DataChannel control message
    pub fn parse(buffer: &[u8]) -> Result<DataChannelControlMessage> {
        if buffer.len() < 1 { return Err(Error::new(ErrorKind::UnexpectedEof, "missing control message type")); }

        let control_type = DataChannelControlMessageType::from(buffer[0])
            .ok_or(Error::new(ErrorKind::InvalidInput, "invalid control message type"))?;

        match control_type {
            DataChannelControlMessageType::Open => Ok(DataChannelControlMessage::Open(DataChannelControlMessageOpen::parse(buffer)?)),
            DataChannelControlMessageType::OpenAck => Ok(DataChannelControlMessage::OpenAck(DataChannelControlMessageOpenAck::parse(buffer)?))
        }
    }

    /// Calculate the expected message length in bytes
    pub fn expected_length(&self) -> usize {
        let payload_length = match self {
            DataChannelControlMessage::Open(payload) => payload.expected_length(),
            DataChannelControlMessage::OpenAck(payload) => payload.expected_length()
        };
        payload_length
    }

    /// Write the control message to the target buffer.
    /// On success it returns the written bytes.
    pub fn write(&self, target: &mut [u8]) -> Result<usize> {
        match self {
            DataChannelControlMessage::Open(payload) => payload.write(target),
            DataChannelControlMessage::OpenAck(payload) => payload.write(target)
        }
    }
}

/// This enum contains all registered DataChannel message types as listed in
/// https://tools.ietf.org/html/draft-ietf-rtcweb-data-channel-13#section-8
pub enum DataChannelMessageType {
    Control,
    String,
    Binary,
    StringEmpty,
    BinaryEmpty
}

impl DataChannelMessageType {
    /// Get the SCTP PPID (Packet Payload Identifier) value
    pub fn value(&self) -> u32 {
        match self {
            &DataChannelMessageType::Control     => 50,
            &DataChannelMessageType::String      => 51,
            &DataChannelMessageType::Binary      => 53,
            &DataChannelMessageType::StringEmpty => 56,
            &DataChannelMessageType::BinaryEmpty => 57,
        }
    }

    /// Parse the DataChannelMessageType from a given SCTP PPID (Packet Payload Identifier)
    fn from(value: u32) -> Option<Self> {
        match value {
            50 => Some(DataChannelMessageType::Control),
            51 => Some(DataChannelMessageType::String),
            53 => Some(DataChannelMessageType::Binary),
            56 => Some(DataChannelMessageType::StringEmpty),
            57 => Some(DataChannelMessageType::BinaryEmpty),
            _ => None
        }
    }
}

/// Container for all known DataChannel messages
#[derive(PartialEq, Debug)]
pub enum DataChannelMessage {
    Control(DataChannelControlMessage),
    String(String),
    Binary(Vec<u8>),
    StringEmpty(),
    BinaryEmpty()
}

impl DataChannelMessage {
    /// Parse a DataChannelMessage from the given buffer.
    /// The PPID is required to properly parse the payload.
    pub fn parse(payload: &[u8], ppid: u32) -> Result<DataChannelMessage> {
        let msg_type = DataChannelMessageType::from(ppid)
            .ok_or(Error::new(ErrorKind::InvalidInput, "invalid data channel message type"))?;

        match msg_type {
            DataChannelMessageType::Control => Ok(DataChannelMessage::Control(DataChannelControlMessage::parse(payload)?)),
            DataChannelMessageType::String => Ok(DataChannelMessage::String(String::from_utf8(Vec::from(payload)).map_err(|err| Error::new(ErrorKind::InvalidInput, err))?)),
            DataChannelMessageType::StringEmpty => Ok(DataChannelMessage::StringEmpty()),
            DataChannelMessageType::Binary => Ok(DataChannelMessage::Binary(Vec::from(payload))),
            DataChannelMessageType::BinaryEmpty => Ok(DataChannelMessage::BinaryEmpty())
        }
    }

    /// Get the `DataChannelMessageType` from this message.
    pub fn message_type(&self) -> DataChannelMessageType {
        match self {
            DataChannelMessage::Control(_) => DataChannelMessageType::Control,
            DataChannelMessage::String(_) => DataChannelMessageType::String,
            DataChannelMessage::StringEmpty() => DataChannelMessageType::StringEmpty,
            DataChannelMessage::Binary(_) => DataChannelMessageType::Binary,
            DataChannelMessage::BinaryEmpty() => DataChannelMessageType::BinaryEmpty,
        }
    }

    /// Calculate the expected packet length in bytes
    pub fn expected_length(&self) -> usize {
        let payload_length = match self {
            DataChannelMessage::Control(message) => message.expected_length(),
            DataChannelMessage::String(payload) => payload.len(),
            DataChannelMessage::StringEmpty() => 0usize,
            DataChannelMessage::Binary(payload) => payload.len(),
            DataChannelMessage::BinaryEmpty() => 0usize
        };

        payload_length
    }

    /// Write the DataChannel message to the target buffer.
    /// On success the written byte count will be returned.
    pub fn write(&self, mut buffer: &mut [u8]) -> Result<usize> {
        match self {
            DataChannelMessage::Control(message) => message.write(&mut buffer),
            DataChannelMessage::String(message) => { buffer.write_all(message.as_bytes())?; Ok(message.len()) },
            DataChannelMessage::StringEmpty() => Ok(0),
            DataChannelMessage::Binary(message) => { buffer.write_all(message)?; Ok(message.len()) },
            DataChannelMessage::BinaryEmpty() => Ok(0)
        }
    }
}

#[cfg(test)]
mod test {
    use crate::sctp::message::{DataChannelType, DataChannelControlMessageType, DataChannelControlMessageOpen, DataChannelControlMessageOpenAck};

    #[test]
    fn test_data_channel_type() {
        let values = vec![
            DataChannelType::Reliable,
            DataChannelType::ReliableUnordered,
            DataChannelType::PartialReliableRexmit(22),
            DataChannelType::PartialReliableRexmitUnordered(33),
            DataChannelType::PartialReliableTimed(12),
            DataChannelType::PartialReliableTimedUnordered(38)
        ];
        for value in values.iter() {
            assert_eq!(DataChannelType::from(value.value(), value.reliability_parameter()), Some(*value));
        }
    }

    #[test]
    fn test_data_channel_control_message() {
        let values = vec![
            DataChannelControlMessageType::Open,
            DataChannelControlMessageType::OpenAck
        ];
        for value in values.iter() {
            assert_eq!(DataChannelControlMessageType::from(value.value()), Some(*value));
        }
    }

    #[test]
    fn test_data_channel_open_message() {
        let message = DataChannelControlMessageOpen {
            label: String::from("Hello World"),
            protocol: String::from("Some protocol"),
            priority: 10,
            channel_type: DataChannelType::PartialReliableRexmit(22)
        };
        let mut buffer = [0u8; 100];
        let size = message.write(&mut buffer).expect("failed to encode opening message");
        let parsed = DataChannelControlMessageOpen::parse(&buffer[0..size])
            .expect("failed to parse message");
        assert_eq!(message, parsed);
    }

    #[test]
    fn test_data_channel_open_ack_message() {
        let message = DataChannelControlMessageOpenAck{};
        let mut buffer = [0u8; 2];
        let size = message.write(&mut buffer).expect("failed to encode open ack message");
        let parsed = DataChannelControlMessageOpenAck::parse(&buffer[0..size])
            .expect("failed to parse message");
        assert_eq!(message, parsed);
    }
}