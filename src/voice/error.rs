//! Voice errors.

use super::{source, rtp, ws};

use std::fmt::{self, Display, Formatter};
use std::error::Error as StdError;

/// Any error that can occur with voice interactions.
#[derive(Debug)]
pub enum Error {
    /// An error occured in the websocket.
    Ws(ws::Error),
    /// An error occured in the RTP socket.
    Rtp(rtp::Error),
    /// An error occured in the audio source encoding.
    Audio(source::Error),
    /// The gateway closed unexpectedly.
    GatewayClosed,
    /// An operation timed out.
    Timeout,
    /// The bot was unable to join the specified channel.
    CannotJoin,
    /// The bot was disconnected from the channel.
    Disconnected,
}

impl From<ws::Error> for Error {
    fn from(e: ws::Error) -> Error {
        Error::Ws(e)
    }
}

impl From<rtp::Error> for Error {
    fn from(e: rtp::Error) -> Error {
        Error::Rtp(e)
    }
}

impl From<source::Error> for Error {
    fn from(e: source::Error) -> Error {
        Error::Audio(e)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Error::Ws(err) => Display::fmt(err, f),
            Error::Rtp(err) => Display::fmt(err, f),
            Error::Audio(err) => Display::fmt(err, f),
            Error::GatewayClosed => f.write_str("gateway closed unexpected"),
            Error::Timeout => f.write_str("operation timed out"),
            Error::CannotJoin => f.write_str("unable to join Discord channel"),
            Error::Disconnected => f.write_str("bot disconnected from channel"),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Error::Ws(err) => Some(err),
            Error::Rtp(err) => Some(err),
            Error::Audio(err) => Some(err),
            _ => None,
        }
    }
}
