//! Websocket payloads.

use serde::{
    de::{
        self, value::U8Deserializer, Deserializer, DeserializeSeed, Visitor,
        MapAccess, Unexpected, IntoDeserializer, IgnoredAny,
    },
    ser::{Serializer, SerializeStruct as _},
    Serialize, Deserialize,
};
use serde_repr::{Serialize_repr, Deserialize_repr};
use std::fmt::{self, Formatter};
use twilight_model::id::{Id, marker::{GuildMarker, UserMarker}};

/// Websocket event.
#[derive(Debug)]
pub enum Event {
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
}

impl Event {
    /// Gets the opcode of the event.
    pub fn op(&self) -> OpCode {
        match self {
            Event::Identify(_) => OpCode::Identify,
            Event::SelectProtocol(_) => OpCode::SelectProtocol,
            Event::Ready(_) => OpCode::Ready,
            Event::Heartbeat(_) => OpCode::Heartbeat,
            Event::SessionDescription(_) => OpCode::SessionDescription,
            Event::Speaking(_) => OpCode::Speaking,
            Event::HeartbeatAck(_) => OpCode::HeartbeatAck,
            Event::Resume(_) => OpCode::Resume,
            Event::Hello(_) => OpCode::Hello,
            Event::Resumed => OpCode::Resumed,
        }
    }
}

impl Serialize for Event {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut event = serializer.serialize_struct("Event", 2)?;
        event.serialize_field("op", &self.op())?;

        match self {
            Event::Identify(ev) => event.serialize_field("d", ev)?,
            Event::SelectProtocol(ev) => event.serialize_field("d", ev)?,
            Event::Ready(ev) => event.serialize_field("d", ev)?,
            Event::Heartbeat(ev) => event.serialize_field("d", ev)?,
            Event::SessionDescription(ev) => event.serialize_field("d", ev)?,
            Event::Speaking(ev) => event.serialize_field("d", ev)?,
            Event::HeartbeatAck(ev) => event.serialize_field("d", ev)?,
            Event::Resume(ev) => event.serialize_field("d", ev)?,
            Event::Hello(ev) => event.serialize_field("d", ev)?,
            Event::Resumed => event.serialize_field("d", &None::<()>)?,
        };

        event.end()
    }
}

/// A deserializer for [`Event`].
pub struct EventDeserializer {
    op: u8,
}

impl EventDeserializer {
    /// Creates a new `EventDeserializer`.
    pub const fn new(op: u8) -> EventDeserializer {
        EventDeserializer {
            op,
        }
    }

    /// Scans the JSON payload for identification data.
    pub fn from_json(input: &str) -> Option<EventDeserializer> {
        Some(EventDeserializer {
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

impl<'de> DeserializeSeed<'de> for EventDeserializer {
    type Value = Event;

    fn deserialize<D>(self, deserializer: D) -> Result<Event, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            D,
            Op,
        }

        struct EventVisitor(u8);

        impl EventVisitor {
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

        impl<'de> Visitor<'de> for EventVisitor {
            type Value = Event;

            fn expecting(&self, f: &mut Formatter) -> fmt::Result {
                f.write_str("valid Event struct")
            }

            fn visit_map<V>(self, map: V) -> Result<Event, V::Error>
            where
                V: MapAccess<'de>,
            {
                let op_deser: U8Deserializer<V::Error> = self.0.into_deserializer();

                let op = OpCode::deserialize(op_deser).ok().ok_or_else(|| {
                    let unexpected = Unexpected::Unsigned(u64::from(self.0));

                    de::Error::invalid_value(unexpected, &"an opcode")
                })?;

                match op {
                    OpCode::Identify => self.get_d(map).map(Event::Identify),
                    OpCode::SelectProtocol => self.get_d(map).map(Event::SelectProtocol),
                    OpCode::Ready => self.get_d(map).map(Event::Ready),
                    OpCode::Heartbeat => self.get_d(map).map(Event::Heartbeat),
                    OpCode::SessionDescription => self.get_d(map).map(Event::SessionDescription),
                    OpCode::Speaking => self.get_d(map).map(Event::Speaking),
                    OpCode::HeartbeatAck => self.get_d(map).map(Event::HeartbeatAck),
                    OpCode::Resume => self.get_d(map).map(Event::Resume),
                    OpCode::Hello => self.get_d(map).map(Event::Hello),
                    OpCode::Resumed => Ok(Event::Resumed),
                }
            }
        }
        
        deserializer.deserialize_struct("Event", &["d"], EventVisitor(self.op))
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
    pub mode: String,
}

/// The `READY` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct Ready {
    pub ssrc: u32,
    pub ip: String,
    pub port: u16,
    pub modes: Vec<String>,
}

/// The `SESSION_DESCRIPTION` payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct SessionDescription {
    pub mode: String,
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

