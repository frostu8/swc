//! Websocket payloads.

use serde::{
    de::{
        self, value::U8Deserializer, DeserializeSeed, Deserializer, IgnoredAny, IntoDeserializer,
        MapAccess, Unexpected, Visitor,
    },
    ser::{SerializeStruct as _, Serializer},
    Deserialize, Serialize,
};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::fmt::{self, Debug, Display, Formatter};
use twilight_model::id::{
    marker::{GuildMarker, UserMarker},
    Id,
};
use serde_json::Value;

/// Raw gateway event.
#[derive(Debug)]
pub enum GatewayEvent {
    Identify(Identify),
    SelectProtocol(SelectProtocol),
    Ready(Ready),
    Heartbeat(Heartbeat),
    SessionDescription(SessionDescription),
    Speaking(Speaking),
    HeartbeatAck(HeartbeatAck),
    Resume(Resume),
    Hello(Hello),
    Resumed,
    ClientConnect(ClientConnect),
    ClientDisconnect(ClientDisconnect),
}

#[derive(Debug, Deserialize_repr, Serialize_repr)]
#[repr(u8)]
pub enum OpCode {
    Identify = 0,
    SelectProtocol = 1,
    Ready = 2,
    Heartbeat = 3,
    SessionDescription = 4,
    Speaking = 5,
    HeartbeatAck = 6,
    Resume = 7,
    Hello = 8,
    Resumed = 9,
    ClientConnect = 12,
    ClientDisconnect = 13,
}

impl GatewayEvent {
    /// Gets the opcode of the event.
    pub fn op(&self) -> OpCode {
        match self {
            GatewayEvent::Identify(_) => OpCode::Identify,
            GatewayEvent::SelectProtocol(_) => OpCode::SelectProtocol,
            GatewayEvent::Ready(_) => OpCode::Ready,
            GatewayEvent::Heartbeat(_) => OpCode::Heartbeat,
            GatewayEvent::SessionDescription(_) => OpCode::SessionDescription,
            GatewayEvent::Speaking(_) => OpCode::Speaking,
            GatewayEvent::HeartbeatAck(_) => OpCode::HeartbeatAck,
            GatewayEvent::Resume(_) => OpCode::Resume,
            GatewayEvent::Hello(_) => OpCode::Hello,
            GatewayEvent::Resumed => OpCode::Resumed,
            GatewayEvent::ClientConnect(_) => OpCode::ClientConnect,
            GatewayEvent::ClientDisconnect(_) => OpCode::ClientDisconnect,
        }
    }
}

impl Serialize for GatewayEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut event = serializer.serialize_struct("GatewayEvent", 2)?;
        event.serialize_field("op", &self.op())?;

        match self {
            GatewayEvent::Identify(ev) => event.serialize_field("d", ev)?,
            GatewayEvent::SelectProtocol(ev) => event.serialize_field("d", ev)?,
            GatewayEvent::Ready(ev) => event.serialize_field("d", ev)?,
            GatewayEvent::Heartbeat(ev) => event.serialize_field("d", ev)?,
            GatewayEvent::SessionDescription(ev) => event.serialize_field("d", ev)?,
            GatewayEvent::Speaking(ev) => event.serialize_field("d", ev)?,
            GatewayEvent::HeartbeatAck(ev) => event.serialize_field("d", ev)?,
            GatewayEvent::Resume(ev) => event.serialize_field("d", ev)?,
            GatewayEvent::Hello(ev) => event.serialize_field("d", ev)?,
            GatewayEvent::Resumed => event.serialize_field("d", &None::<()>)?,
            GatewayEvent::ClientConnect(ev) => event.serialize_field("d", ev)?,
            GatewayEvent::ClientDisconnect(ev) => event.serialize_field("d", ev)?,
        };

        event.end()
    }
}

/// A deserializer for [`GatewayEvent`].
pub struct GatewayEventDeserializer {
    op: u8,
}

impl GatewayEventDeserializer {
    /// Creates a new `GatewayEventDeserializer`.
    pub const fn new(op: u8) -> GatewayEventDeserializer {
        GatewayEventDeserializer { op }
    }

    /// Scans the JSON payload for identification data.
    pub fn from_json(input: &str) -> Option<GatewayEventDeserializer> {
        Some(GatewayEventDeserializer {
            op: Self::find_opcode(input)?,
        })
    }

    fn find_opcode(input: &str) -> Option<u8> {
        let from = input.find(r#""op":"#)? + 5;
        let to = input.get(from..)?.find(&[',', '}'] as &[_])?;

        let result = input.get(from..from + to)?.trim();
        result.parse().ok()
    }
}

impl<'de> DeserializeSeed<'de> for GatewayEventDeserializer {
    type Value = GatewayEvent;

    fn deserialize<D>(self, deserializer: D) -> Result<GatewayEvent, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            D,
            Op,
        }

        struct GatewayEventVisitor(u8);

        impl GatewayEventVisitor {
            fn get_d<'de, T, V>(self, mut map: V) -> Result<T, V::Error>
            where
                V: MapAccess<'de>,
                T: Deserialize<'de>,
            {
                loop {
                    let key = match map.next_key() {
                        Ok(Some(key)) => key,
                        Ok(None) => break,
                        Err(_msg) => {
                            map.next_value::<IgnoredAny>()?;
                            continue;
                        }
                    };

                    match key {
                        Field::D => return map.next_value::<T>(),
                        Field::Op => map.next_value::<IgnoredAny>()?,
                    };
                }

                Err(de::Error::missing_field("d"))
            }
        }

        impl<'de> Visitor<'de> for GatewayEventVisitor {
            type Value = GatewayEvent;

            fn expecting(&self, f: &mut Formatter) -> fmt::Result {
                f.write_str("valid GatewayEvent struct")
            }

            fn visit_map<V>(self, map: V) -> Result<GatewayEvent, V::Error>
            where
                V: MapAccess<'de>,
            {
                let op_deser: U8Deserializer<V::Error> = self.0.into_deserializer();

                let op = OpCode::deserialize(op_deser).ok().ok_or_else(|| {
                    let unexpected = Unexpected::Unsigned(u64::from(self.0));

                    de::Error::invalid_value(unexpected, &"an opcode")
                })?;

                match op {
                    OpCode::Identify => self.get_d(map).map(GatewayEvent::Identify),
                    OpCode::SelectProtocol => self.get_d(map).map(GatewayEvent::SelectProtocol),
                    OpCode::Ready => self.get_d(map).map(GatewayEvent::Ready),
                    OpCode::Heartbeat => self.get_d(map).map(GatewayEvent::Heartbeat),
                    OpCode::SessionDescription => {
                        self.get_d(map).map(GatewayEvent::SessionDescription)
                    }
                    OpCode::Speaking => self.get_d(map).map(GatewayEvent::Speaking),
                    OpCode::HeartbeatAck => self.get_d(map).map(GatewayEvent::HeartbeatAck),
                    OpCode::Resume => self.get_d(map).map(GatewayEvent::Resume),
                    OpCode::Hello => self.get_d(map).map(GatewayEvent::Hello),
                    OpCode::Resumed => self.get_d::<Option::<Value>, _>(map).map(|_| GatewayEvent::Resumed),
                    OpCode::ClientConnect => self.get_d(map).map(GatewayEvent::ClientConnect),
                    OpCode::ClientDisconnect => self.get_d(map).map(GatewayEvent::ClientDisconnect),
                }
            }
        }

        deserializer.deserialize_struct("GatewayEvent", &["d"], GatewayEventVisitor(self.op))
    }
}

/// The `IDENTIFY` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct Identify {
    #[serde(rename = "server_id")]
    pub guild_id: Id<GuildMarker>,
    pub user_id: Id<UserMarker>,
    pub session_id: String,
    pub token: String,
}

/// The `SELECT_PROTOCOL` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct SelectProtocol {
    pub protocol: String,
    pub data: SelectProtocolData,
}

/// Data of [`SelectProtocol`].
#[derive(Debug, Deserialize, Serialize)]
pub struct SelectProtocolData {
    pub address: String,
    pub port: u16,
    pub mode: EncryptionMode,
}

/// The `READY` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct Ready {
    pub ssrc: u32,
    pub ip: String,
    pub port: u16,
    pub modes: Vec<EncryptionMode>,
}

/// The `SESSION_DESCRIPTION` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct SessionDescription {
    pub mode: EncryptionMode,
    pub secret_key: [u8; 32],
}

/// The `SPEAKING` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct Speaking {
    pub speaking: u8,
    #[serde(default)]
    pub delay: Option<u32>,
    pub ssrc: u32,
}

/// The `HEARTBEAT` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct Heartbeat(pub u64);

/// The `HEARTBEAT_ACK` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct HeartbeatAck(pub u64);

/// The `RESUME` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct Resume {
    #[serde(rename = "server_id")]
    pub guild_id: Id<GuildMarker>,
    pub session_id: String,
    pub token: String,
}

/// The `HELLO` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct Hello {
    pub heartbeat_interval: f32,
}

/// The `CLIENT_CONNECT` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct ClientConnect {
    pub audio_ssrc: u32,
    pub user_id: Id<UserMarker>,
    pub video_ssrc: u32,
}

/// The `CLIENT_DISCONNECT` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct ClientDisconnect {
    pub user_id: Id<UserMarker>,
}

/// Discord encryption scheme.
///
/// See [discord docs][1] for more info.
///
/// [1]: https://discord.com/developers/docs/topics/voice-connections#establishing-a-voice-udp-connection-encryption-modes
#[derive(Clone, Debug, PartialEq)]
pub enum EncryptionMode {
    /// The nonce bytes are the RTP header
    Normal,
    /// The nonce bytes are 24-bytes appended to the payload of the RTP packet.
    ///
    /// Nonce generated randomly.
    Suffix,
    /// The nonce bytes are 4-bytes appended to the payload of the RTP packet.
    ///
    /// Nonce generated incrementally.
    Lite,
    /// Other encryption modes supported by discord, but not by this library.
    Other(String),
}

impl EncryptionMode {
    const NORMAL_STR: &'static str = "xsalsa20_poly1305";
    const SUFFIX_STR: &'static str = "xsalsa20_poly1305_suffix";
    const LITE_STR: &'static str = "xsalsa20_poly1305_lite";

    /// Returns the string representation of the mode.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Normal => Self::NORMAL_STR,
            Self::Suffix => Self::SUFFIX_STR,
            Self::Lite => Self::LITE_STR,
            Self::Other(s) => s,
        }
    }
}

impl Display for EncryptionMode {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for EncryptionMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EncryptionMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct EncryptionModeVisitor;

        impl<'de> Visitor<'de> for EncryptionModeVisitor {
            type Value = EncryptionMode;

            fn expecting(&self, f: &mut Formatter) -> fmt::Result {
                f.write_str("a valid EncryptionMode")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match v {
                    EncryptionMode::NORMAL_STR => Ok(EncryptionMode::Normal),
                    EncryptionMode::SUFFIX_STR => Ok(EncryptionMode::Suffix),
                    EncryptionMode::LITE_STR => Ok(EncryptionMode::Lite),
                    v => Ok(EncryptionMode::Other(v.to_owned())),
                }
            }
        }

        deserializer.deserialize_str(EncryptionModeVisitor)
    }
}

mod tests {
    use super::*;

    #[test]
    fn test_opcode_resume()
    {
        const PAYLOAD: &'static str = r#"{"op":9,"d":null}"#;

        let event = GatewayEventDeserializer::from_json(&PAYLOAD)
            .unwrap();

        let mut json = serde_json::Deserializer::from_str(&PAYLOAD);

        let event = event
            .deserialize(&mut json)
            .unwrap();

        assert!(matches!(event, GatewayEvent::Resumed));
    }
}
