//! Websocket error.

use tungstenite::protocol::frame::{CloseFrame, coding::CloseCode};
use std::fmt::{self, Debug, Display, Formatter};

/// Websocket error.
#[derive(Debug)]
pub enum Error {
    Api(ApiError),
    Closed(Option<CloseFrame<'static>>),
    Ws(tungstenite::error::Error),
    Json(serde_json::Error),
    MissingOpcode,
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Error::Api(err) => Display::fmt(err, f),
            Error::Ws(err) => Display::fmt(err, f),
            Error::Closed(err) => Debug::fmt(err, f),
            Error::Json(err) => Display::fmt(err, f),
            Error::MissingOpcode => f.write_str("missing opcode"),
        }
    }
}

impl From<tungstenite::error::Error> for Error {
    fn from(e: tungstenite::error::Error) -> Error {
        Error::Ws(e)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Api(err) => Some(err),
            Error::Ws(err) => Some(err),
            Error::Json(err) => Some(err),
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

impl ApiError {
    /// Checks if the error was a normal disconnection error.
    pub fn disconnected(&self) -> bool {
        matches!(self.code, Code::Disconnected)
    }
}

impl Display for ApiError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "#{:?}: {}", self.code, self.message)
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
