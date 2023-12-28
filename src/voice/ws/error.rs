//! Connection error.

use std::fmt::{self, Debug, Display, Formatter};
use tungstenite::error::{Error as WsError, ProtocolError as WsProtocolError};
use tungstenite::protocol::frame::{coding::CloseCode, CloseFrame};

use super::rtp::IpDiscoveryError;

/// Connection error.
#[derive(Debug)]
pub enum Error {
    Api(ApiError),
    Closed(Option<CloseFrame<'static>>),
    Protocol(ProtocolError),
    Ws(WsError),
    Io(std::io::Error),
    IpDiscovery(IpDiscoveryError),
}

impl Error {
    /// Checks if the error was a result of being disconnected gracefully.
    pub fn disconnected(&self) -> bool {
        match self {
            Error::Api(err) => matches!(err.code, Code::Disconnected),
            _ => false,
        }
    }

    /// Checks if we can safely resume after an error.
    pub fn can_resume(&self) -> bool {
        match self {
            Error::Api(err) => matches!(err.code, Code::VoiceServerCrashed),
            Error::Ws(WsError::Protocol(p)) => {
                matches!(p, WsProtocolError::ResetWithoutClosingHandshake)
            }
            _ => false,
        }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Error::Api(err) => Display::fmt(err, f),
            Error::Ws(err) => Display::fmt(err, f),
            Error::Io(err) => Display::fmt(err, f),
            Error::Closed(err) => Debug::fmt(err, f),
            Error::IpDiscovery(err) => Display::fmt(err, f),
            Error::Protocol(err) => Display::fmt(err, f),
        }
    }
}

impl From<tungstenite::error::Error> for Error {
    fn from(err: tungstenite::error::Error) -> Error {
        match err {
            tungstenite::error::Error::Io(err) => Error::Io(err),
            err => Error::Ws(err),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Error {
        Error::Io(err)
    }
}

impl From<IpDiscoveryError> for Error {
    fn from(err: IpDiscoveryError) -> Error {
        Error::IpDiscovery(err)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Api(err) => Some(err),
            Error::Ws(err) => Some(err),
            Error::Protocol(err) => Some(err),
            _ => None,
        }
    }
}

/// Websocket protocol error.
#[derive(Debug)]
pub enum ProtocolError {
    /// Payload failed to decode.
    ///
    /// Contains the original payload.
    Deser(serde_json::Error, String),
    /// Payload failed to encode
    Ser(serde_json::Error),
    /// The server returned an unsupported encryption mode.
    UnsupportedEncryptionMode(super::payload::EncryptionMode),
    /// The server returned a payload without a valid opcode.
    MissingOpcode,
}

impl Display for ProtocolError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            ProtocolError::Deser(err, msg) => {
                write!(f, "failed to parse message \"{}\": {}", msg, err)
            }
            ProtocolError::Ser(err) => {
                write!(f, "failed to create json: {}", err)
            }
            ProtocolError::UnsupportedEncryptionMode(mode) => {
                write!(f, "unsupported encryption mode \"{}\"", mode)
            }
            ProtocolError::MissingOpcode => {
                write!(f, "payload missing opcode")
            }
        }
    }
}

impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProtocolError::Deser(err, _) => Some(err),
            ProtocolError::Ser(err) => Some(err),
            _ => None,
        }
    }
}

/// Api error.
#[derive(Clone, Debug)]
pub struct ApiError {
    pub code: Code,
    pub message: String,
}

impl Display for ApiError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "#{}: {}", self.code as u16, self.message)
    }
}

impl std::error::Error for ApiError {}

/// Api error code.
#[derive(Clone, Copy, Debug)]
#[repr(u16)]
pub enum Code {
    UnknownOpcode = 4001,
    BadPayload = 4002,
    NotAuthenticated = 4003,
    AuthenticationFailed = 4004,
    AlreadyAuthenticated = 4005,
    InvalidSession = 4006,
    SessionTimeout = 4009,
    ServerNotFound = 4011,
    UnknownProtocol = 4012,
    Disconnected = 4014,
    VoiceServerCrashed = 4015,
    UnknownEncryption = 4016,
}

impl Code {
    pub fn from_code(code: CloseCode) -> Option<Code> {
        match code.into() {
            4001 => Some(Code::UnknownOpcode),
            4002 => Some(Code::BadPayload),
            4003 => Some(Code::NotAuthenticated),
            4004 => Some(Code::AuthenticationFailed),
            4005 => Some(Code::AlreadyAuthenticated),
            4006 => Some(Code::InvalidSession),
            4009 => Some(Code::SessionTimeout),
            4011 => Some(Code::ServerNotFound),
            4012 => Some(Code::UnknownProtocol),
            4014 => Some(Code::Disconnected),
            4015 => Some(Code::VoiceServerCrashed),
            4016 => Some(Code::UnknownEncryption),
            _ => None,
        }
    }
}
