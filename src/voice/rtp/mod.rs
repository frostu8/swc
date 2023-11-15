//! Low-level RTP protocol types.

pub mod error;
mod crypto;

pub use crypto::{EncryptionMode, Encryptor};
pub use error::Error;
use tracing::instrument;

use std::fmt::{self, Debug, Display, Formatter};
use std::net::{AddrParseError, IpAddr, SocketAddr};
use std::str::Utf8Error;

use tokio::net::UdpSocket;

use super::constants::{MONO_FRAME_SIZE, VOICE_PACKET_MAX};

use xsalsa20poly1305::TAG_SIZE;

/// A socket for RTP packets.
#[derive(Debug)]
pub struct Socket {
    udp: UdpSocket,
    encryptor: Encryptor,

    sequence: u16,
    timestamp: u32,
    ssrc: u32,
}

impl Socket {
    /// Creates a new `Socket`.
    pub fn new(udp: UdpSocket, ssrc: u32, encryptor: Encryptor) -> Socket {
        Socket {
            udp,
            encryptor,
            sequence: 0,
            timestamp: 0,
            ssrc,
        }
    }

    /// Sends a packet over the socket, filling in its metadata and then
    /// encrypting it.
    #[inline]
    pub async fn send<T>(&mut self, packet: &mut Packet<T>) -> Result<(), Error>
    where
        T: AsRef<[u8]> + AsMut<[u8]>,
    {
        // set packet metadata
        packet.set_sequence(self.sequence);
        packet.set_timestamp(self.timestamp);
        packet.set_ssrc(self.ssrc);

        //debug!("packet; sequence={}, timestamp={}, ssrc={}", self.sequence, self.timestamp, self.ssrc);

        // update metadata for next packet
        self.sequence = self.sequence.overflowing_add(1).0;
        self.timestamp = self.timestamp.overflowing_add(MONO_FRAME_SIZE as u32).0;

        // encrypt packet
        self.encryptor.encrypt(packet).map_err(Error::Encrypt)?;

        // send packet
        self.udp.send(packet.as_ref()).await.map_err(Error::Io)?;

        Ok(())
    }

    /// The ssrc of the socket.
    pub fn ssrc(&self) -> u32 {
        self.ssrc
    }
}

/// RTP packet.
///
/// Acts as a buffer where packets can be made and sent across the internet.
/// `Packet`'s [`Default`] implementation returns a `Packet` initialized with a
/// `[u8; crate::constants::VOICE_PACKET_MAX]`.
pub struct Packet<T> {
    pkt: T,
    payload_len: usize,
}

impl<T> Packet<T> {
    /// The size of the RTP packet header.
    ///
    /// This also includes space for the Poly1305 tag at the front.
    pub const HEADER_LEN: usize = 12 + TAG_SIZE;

    /// The payload len of the RTP packet.
    ///
    /// This is an intrinsic property of the packet, but is tracked because the
    /// backing buffer is never resized to fit the actual payload of the packet.
    /// In fact, this must be managed manually.
    pub fn payload_len(&self) -> usize {
        self.payload_len
    }
}

impl<T> Packet<T>
where
    T: AsRef<[u8]>,
{
    /// Sets the payload len of the RTP packet.
    ///
    /// This is an intrinsic property of the packet, but is tracked because the
    /// backing buffer is never resized to fit the actual payload of the packet.
    /// In fact, this must be managed manually.
    ///
    /// # Panics
    /// Panics if `payload_len + HEADER_LEN` is greater than what the backing
    /// buffer can hold.
    pub fn set_payload_len(&mut self, payload_len: usize) {
        assert!(self.pkt.as_ref().len() >= Packet::<()>::HEADER_LEN + payload_len);

        self.payload_len = payload_len;
    }

    /// Returns a reference to the payload.
    pub fn payload(&self) -> &[u8] {
        &self.pkt.as_ref()[Self::HEADER_LEN..]
    }

    fn header(&self) -> &[u8] {
        &self.pkt.as_ref()[..Self::HEADER_LEN]
    }
}

impl<T> Packet<T>
where
    T: AsMut<[u8]>,
{
    /// Creates a new RTP packet using a container, and initializes the buffer.
    ///
    /// # Panics
    /// Panics if the backing buffer's length is less than
    /// [`Packet::HEADER_LEN`].
    pub fn new(mut pkt: T) -> Packet<T> {
        {
            let pkt = pkt.as_mut();
            assert!(pkt.len() >= Packet::<()>::HEADER_LEN);

            // we really only need to follow discord's spec
            // setup header
            pkt[0] = 0x80;
            pkt[1] = 0x78;
        }

        Packet {
            pkt,
            payload_len: 0,
        }
    }

    /// Sets the sequence number of the RTP packet.
    pub fn set_sequence(&mut self, sequence: u16) {
        (&mut self.pkt.as_mut()[2..4]).copy_from_slice(&sequence.to_be_bytes());
    }

    /// Sets the timestamp of the RTP packet.
    pub fn set_timestamp(&mut self, timestamp: u32) {
        (&mut self.pkt.as_mut()[4..8]).copy_from_slice(&timestamp.to_be_bytes());
    }

    /// Sets the SSRC of the RTP packet.
    pub fn set_ssrc(&mut self, ssrc: u32) {
        (&mut self.pkt.as_mut()[8..12]).copy_from_slice(&ssrc.to_be_bytes());
    }

    /// Returns a mutable reference to the Poly1305 tag.
    pub fn tag_mut(&mut self) -> &mut [u8] {
        &mut self.pkt.as_mut()[12..12 + TAG_SIZE]
    }

    /// Returns a mutable reference to the rest of the buffer after the header.
    pub fn payload_mut(&mut self) -> &mut [u8] {
        &mut self.pkt.as_mut()[Self::HEADER_LEN..]
    }
}

impl<T> AsRef<[u8]> for Packet<T>
where
    T: AsRef<[u8]>,
{
    fn as_ref(&self) -> &[u8] {
        &self.pkt.as_ref()[0..Self::HEADER_LEN + self.payload_len]
    }
}

impl<T> Debug for Packet<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("Packet(_)")
    }
}

impl Default for Packet<[u8; VOICE_PACKET_MAX]> {
    fn default() -> Packet<[u8; VOICE_PACKET_MAX]> {
        Packet::new([0u8; VOICE_PACKET_MAX])
    }
}

/// Discord IP discovery.
///
/// Accepts a UDP socket connected to a Discord endpoint. **While the client is
/// waiting for a UDP response, unrelated packets will throw errors.**
#[instrument]
pub async fn ip_discovery(udp: &UdpSocket, ssrc: u32) -> Result<SocketAddr, IpDiscoveryError> {
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
                f,
                "invalid header, expected 00 02 00 46, got {:02X} {:02X} {:02X} {:02X}",
                b1, b2, b3, b4,
            ),
            IpDiscoveryError::InvalidSsrc(exp, got) => {
                write!(f, "invalid ssrc, expected {}, got {}", exp, got,)
            }
            IpDiscoveryError::InvalidAddrUtf8(err) => {
                write!(f, "address has invalid utf8: {}", err,)
            }
            IpDiscoveryError::InvalidAddr(err) => write!(f, "address is badly formed: {}", err,),
            IpDiscoveryError::InvalidSize(size) => {
                write!(f, "packet is invalid size: {} bytes", size)
            }
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
