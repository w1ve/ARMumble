//! Mumble TCP packet framing: encode and decode the `[type:u16][length:u32][payload]` wire format.

use bytes::{Buf, BufMut, BytesMut};
use prost::Message;

use crate::error::{Error, Result};
use crate::message::{ControlMessage, TcpMessageType};
use crate::proto::mumble_tcp;

/// Maximum allowed payload size (8 MiB, generous upper bound).
const MAX_PAYLOAD_SIZE: u32 = 8 * 1024 * 1024;

/// Header size: 2 bytes type + 4 bytes length.
pub const HEADER_SIZE: usize = 6;

/// Encode a [`ControlMessage`] into a framed byte buffer ready for the wire.
pub fn encode(msg: &ControlMessage) -> Result<Vec<u8>> {
    let (type_id, payload) = serialize_control_message(msg)?;
    let len = payload.len() as u32;

    let mut buf = Vec::with_capacity(HEADER_SIZE + payload.len());
    buf.put_u16(type_id);
    buf.put_u32(len);
    buf.extend_from_slice(&payload);
    Ok(buf)
}

/// Try to decode one complete frame from `buf`.
///
/// Returns `Ok(Some(msg))` if a full frame was available (consumed from `buf`),
/// `Ok(None)` if more data is needed, or `Err` on protocol errors. Frames of a
/// type this client does not use decode to [`ControlMessage::Unknown`].
pub fn decode(buf: &mut BytesMut) -> Result<Option<ControlMessage>> {
    if buf.len() < HEADER_SIZE {
        return Ok(None);
    }

    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    let payload_len = u32::from_be_bytes([buf[2], buf[3], buf[4], buf[5]]);

    if payload_len > MAX_PAYLOAD_SIZE {
        return Err(Error::InvalidState(format!(
            "payload too large: {payload_len} bytes"
        )));
    }

    let total = HEADER_SIZE + payload_len as usize;
    if buf.len() < total {
        return Ok(None);
    }

    buf.advance(HEADER_SIZE);
    let payload = buf.split_to(payload_len as usize);

    match deserialize_control_message(msg_type, &payload) {
        Err(Error::UnknownMessageType(id)) => Ok(Some(ControlMessage::Unknown(id))),
        other => other.map(Some),
    }
}

// -- Serialization helpers ------------------------------------------

pub(crate) fn serialize_control_message(msg: &ControlMessage) -> Result<(u16, Vec<u8>)> {
    use ControlMessage::*;

    let type_id = msg.type_id();
    let payload = match msg {
        Version(m) => m.encode_to_vec(),
        Authenticate(m) => m.encode_to_vec(),
        Ping(m) => m.encode_to_vec(),
        Reject(m) => m.encode_to_vec(),
        ServerSync(m) => m.encode_to_vec(),
        ChannelRemove(m) => m.encode_to_vec(),
        ChannelState(m) => m.encode_to_vec(),
        UserRemove(m) => m.encode_to_vec(),
        UserState(m) => m.encode_to_vec(),
        PermissionDenied(m) => m.encode_to_vec(),
        CryptSetup(m) => m.encode_to_vec(),
        VoiceTarget(m) => m.encode_to_vec(),
        PermissionQuery(m) => m.encode_to_vec(),
        CodecVersion(m) => m.encode_to_vec(),
        ServerConfig(m) => m.encode_to_vec(),
        PluginMessage(m) => m.encode_to_vec(),
        UdpTunnel(data) => data.clone(),
        Unknown(id) => {
            return Err(Error::InvalidState(format!("cannot send unknown message type {id}")));
        }
    };

    Ok((type_id, payload))
}

pub(crate) fn deserialize_control_message(type_id: u16, payload: &[u8]) -> Result<ControlMessage> {
    let msg_type = TcpMessageType::try_from(type_id)?;
    use TcpMessageType::*;

    let msg = match msg_type {
        Version => ControlMessage::Version(mumble_tcp::Version::decode(payload)?),
        UdpTunnel => ControlMessage::UdpTunnel(payload.to_vec()),
        Authenticate => ControlMessage::Authenticate(mumble_tcp::Authenticate::decode(payload)?),
        Ping => ControlMessage::Ping(mumble_tcp::Ping::decode(payload)?),
        Reject => ControlMessage::Reject(mumble_tcp::Reject::decode(payload)?),
        ServerSync => ControlMessage::ServerSync(mumble_tcp::ServerSync::decode(payload)?),
        ChannelRemove => ControlMessage::ChannelRemove(mumble_tcp::ChannelRemove::decode(payload)?),
        ChannelState => ControlMessage::ChannelState(mumble_tcp::ChannelState::decode(payload)?),
        UserRemove => ControlMessage::UserRemove(mumble_tcp::UserRemove::decode(payload)?),
        UserState => ControlMessage::UserState(mumble_tcp::UserState::decode(payload)?),
        PermissionDenied => ControlMessage::PermissionDenied(mumble_tcp::PermissionDenied::decode(payload)?),
        CryptSetup => ControlMessage::CryptSetup(mumble_tcp::CryptSetup::decode(payload)?),
        VoiceTarget => ControlMessage::VoiceTarget(mumble_tcp::VoiceTarget::decode(payload)?),
        PermissionQuery => ControlMessage::PermissionQuery(mumble_tcp::PermissionQuery::decode(payload)?),
        CodecVersion => ControlMessage::CodecVersion(mumble_tcp::CodecVersion::decode(payload)?),
        ServerConfig => ControlMessage::ServerConfig(mumble_tcp::ServerConfig::decode(payload)?),
        PluginMessage => ControlMessage::PluginMessage(mumble_tcp::PluginMessage::decode(payload)?),
    };
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(msg: &ControlMessage) -> ControlMessage {
        let bytes = encode(msg).expect("encode");
        let mut buf = BytesMut::from(&bytes[..]);
        let out = decode(&mut buf).expect("decode").expect("complete frame");
        assert!(buf.is_empty());
        out
    }

    #[test]
    fn roundtrip_ping() {
        let m = ControlMessage::Ping(mumble_tcp::Ping { timestamp: Some(1234), ..Default::default() });
        match roundtrip(&m) {
            ControlMessage::Ping(p) => assert_eq!(p.timestamp, Some(1234)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn roundtrip_user_state_and_server_config() {
        let us = ControlMessage::UserState(mumble_tcp::UserState {
            session: Some(4),
            channel_id: Some(2),
            listening_channel_add: vec![9],
            ..Default::default()
        });
        match roundtrip(&us) {
            ControlMessage::UserState(u) => {
                assert_eq!((u.session, u.channel_id), (Some(4), Some(2)));
                assert_eq!(u.listening_channel_add, vec![9]);
            }
            other => panic!("{other:?}"),
        }
        let sc = ControlMessage::ServerConfig(mumble_tcp::ServerConfig {
            max_bandwidth: Some(72_000),
            ..Default::default()
        });
        match roundtrip(&sc) {
            ControlMessage::ServerConfig(c) => assert_eq!(c.max_bandwidth, Some(72_000)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn roundtrip_udp_tunnel() {
        match roundtrip(&ControlMessage::UdpTunnel(vec![1, 2, 3])) {
            ControlMessage::UdpTunnel(d) => assert_eq!(d, vec![1, 2, 3]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn partial_frames_wait_for_more() {
        let bytes = encode(&ControlMessage::Ping(mumble_tcp::Ping::default())).expect("encode");
        let mut buf = BytesMut::from(&bytes[..bytes.len() - 1]);
        assert!(decode(&mut buf).expect("ok").is_none());
        let mut empty = BytesMut::new();
        assert!(decode(&mut empty).expect("ok").is_none());
    }

    #[test]
    fn multiple_frames_decode_in_order() {
        let mut wire = Vec::new();
        wire.extend(encode(&ControlMessage::Ping(mumble_tcp::Ping::default())).expect("e"));
        wire.extend(encode(&ControlMessage::UdpTunnel(vec![7])).expect("e"));
        let mut buf = BytesMut::from(&wire[..]);
        assert!(matches!(decode(&mut buf), Ok(Some(ControlMessage::Ping(_)))));
        assert!(matches!(decode(&mut buf), Ok(Some(ControlMessage::UdpTunnel(_)))));
        assert!(matches!(decode(&mut buf), Ok(None)));
    }

    #[test]
    fn oversized_payload_is_rejected() {
        let mut buf = BytesMut::from(&[0_u8, 3, 0xFF, 0xFF, 0xFF, 0xFF][..]);
        assert!(decode(&mut buf).is_err());
    }

    #[test]
    fn text_and_extension_frames_are_skipped_not_fatal() {
        // A TextMessage (11) and a Fancy extension (131) followed by a Ping.
        let mut wire = vec![0_u8, 11, 0, 0, 0, 2, 0x0A, 0x00];
        wire.extend([0_u8, 131, 0, 0, 0, 0]);
        wire.extend(encode(&ControlMessage::Ping(mumble_tcp::Ping::default())).expect("e"));
        let mut buf = BytesMut::from(&wire[..]);
        assert!(matches!(decode(&mut buf), Ok(Some(ControlMessage::Unknown(11)))));
        assert!(matches!(decode(&mut buf), Ok(Some(ControlMessage::Unknown(131)))));
        assert!(matches!(decode(&mut buf), Ok(Some(ControlMessage::Ping(_)))));
    }

    #[test]
    fn header_format_is_type_then_length() {
        let bytes = encode(&ControlMessage::UdpTunnel(vec![9, 9])).expect("e");
        assert_eq!(&bytes[..6], &[0, 1, 0, 0, 0, 2]);
    }
}
