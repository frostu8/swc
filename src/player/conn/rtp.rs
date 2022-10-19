//! Low-level RTP protocol types.

use tokio::net::UdpSocket;

use std::net::{SocketAddr, IpAddr, AddrParseError};
use std::fmt::{self, Display, Formatter};
use std::str::Utf8Error;

use serde::ser::{Serialize, Serializer};
use serde::de::{self, Deserialize, Deserializer, Visitor};

/// Encryption scheme.
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

/// Discord IP discovery.
///
/// Accepts a UDP socket connected to a Discord endpoint. **While the client is
/// waiting for a UDP response, unrelated packets will throw errors.**
pub async fn ip_discovery(
    udp: &UdpSocket,
    ssrc: u32,
) -> Result<SocketAddr, IpDiscoveryError> {
    const REQ_HEADER: &[u8] = &[0x00, 0x01, 0x00, 0x46];
    const RES_HEADER: &[u8] = &[0x00, 0x02, 0x00, 0x46];

    // create IP discovery packet
    let mut buf = [0u8; 74];
    (&mut buf[..4]).copy_from_slice(REQ_HEADER);
    (&mut buf[4..8]).copy_from_slice(&ssrc.to_be_bytes());

    // send over udp socket
    udp.send(&buf).await.map_err(IpDiscoveryError::Io)?;

    // wait for response
    match udp.recv(&mut buf).await {
        Ok(size) if size == 74 => {
            // check header
            if &buf[..4] != RES_HEADER {
                let mut header = [0u8; 4];
                header.copy_from_slice(&buf[..4]);
                return Err(IpDiscoveryError::InvalidHeader(header));
            }

            // check ssrc
            let mut pkt_ssrc = [0u8; 4];
            pkt_ssrc.copy_from_slice(&buf[4..8]);
            let pkt_ssrc = u32::from_be_bytes(pkt_ssrc);

            if pkt_ssrc != ssrc {
                return Err(IpDiscoveryError::InvalidSsrc(ssrc, pkt_ssrc));
            }

            // get port
            let mut port = [0u8; 2];
            port.copy_from_slice(&buf[72..74]);
            let port = u16::from_be_bytes(port);

            // get address
            let addr = &buf[8..72];
            let addr_end = addr.iter().position(|&x| x == 0).unwrap_or(64);

            match std::str::from_utf8(&buf[8..8 + addr_end]) {
                Ok(addr) => match addr.parse::<IpAddr>() {
                    Ok(addr) => Ok((addr, port).into()),
                    Err(err) => Err(IpDiscoveryError::InvalidAddr(err)),
                },
                Err(err) => Err(IpDiscoveryError::InvalidAddrUtf8(err)),
            }
        }
        Ok(size) => Err(IpDiscoveryError::InvalidSize(size)),
        Err(err) => Err(IpDiscoveryError::Io(err)),
    }
}

/// An error that is returned from [`ip_discovery`].
#[derive(Debug)]
pub enum IpDiscoveryError {
    /// The header is badly formed.
    InvalidHeader([u8; 4]),
    /// The SSRC does not match.
    InvalidSsrc(u32, u32),
    /// The address is not made of valid UTF-8.
    InvalidAddrUtf8(Utf8Error),
    /// The address is badly formed.
    InvalidAddr(AddrParseError),
    /// Packet is invalid size.
    InvalidSize(usize),
    /// IO error.
    Io(std::io::Error),
}

impl Display for IpDiscoveryError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            IpDiscoveryError::InvalidHeader([b1, b2, b3, b4]) => write!(
                f, "invalid header, expected 00 02 00 46, got {:02X} {:02X} {:02X} {:02X}", 
                b1, b2, b3, b4,
            ),
            IpDiscoveryError::InvalidSsrc(exp, got) => write!(
                f, "invalid ssrc, expected {}, got {}", exp, got,
            ),
            IpDiscoveryError::InvalidAddrUtf8(err) => write!(
                f, "address has invalid utf8: {}", err,
            ),
            IpDiscoveryError::InvalidAddr(err) => write!(
                f, "address is badly formed: {}", err,
            ),
            IpDiscoveryError::InvalidSize(size) => write!(
                f, "packet is invalid size: {} bytes", size
            ),
            IpDiscoveryError::Io(err) => write!(f, "io: {}", err),
        }
    }
}

impl std::error::Error for IpDiscoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IpDiscoveryError::InvalidAddr(err) => Some(err),
            IpDiscoveryError::Io(err) => Some(err),
            _ => None,
        }
    }
}

