//! RTP/UDP errors.

use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};

use xsalsa20poly1305::aead::Error as CryptoError;

/// An error type for RTP/UDP interactions.
#[derive(Debug)]
pub enum Error {
    /// Io error.
    Io(std::io::Error),
    /// Failed to encrypt a packet.
    Encrypt(CryptoError),
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Error::Io(error) => Display::fmt(error, f),
            Error::Encrypt(error) => Display::fmt(error, f),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Error::Io(error) => Some(error),
            _ => None,
        }
    }
}
