//! Packet encryption and decryption.

use super::Packet;

use rand::{
    rngs::{OsRng, StdRng},
    Rng, RngCore, SeedableRng,
};

use std::fmt::{self, Debug, Formatter};

use xsalsa20poly1305::{
    aead::{self, AeadInPlace, KeyInit},
    XSalsa20Poly1305, NONCE_SIZE,
};

/// Crypto mode for [`Encryptor`].
pub enum EncryptionMode {
    /// The nonce bytes are the RTP header + 12 `\0` bytes.
    Normal,
    /// The nonce bytes are 24 bytes randomly generated and placed at the end of
    /// the packet.
    Suffix,
    /// The nonce bytes are 4 bytes incremented by 1 for each packet, and placed
    /// at the end of the packet. The rest of the nonce is 20 '\0' bytes.
    Lite,
}

/// Encrypts outgoing packets using [`xsalsa20poly1305`].
pub struct Encryptor {
    aead: XSalsa20Poly1305,
    state: EncryptorState,
}

enum EncryptorState {
    Normal,
    Suffix(StdRng),
    Lite(u32),
}

impl Encryptor {
    /// Creates a new encryptor from a secret key and an encryption mode.
    ///
    /// # Panics
    /// Panics if `mode` is [`EncryptionMode::Other`].
    pub fn new(mode: EncryptionMode, secret_key: [u8; 32]) -> Encryptor {
        Encryptor {
            aead: XSalsa20Poly1305::new_from_slice(&secret_key)
                .expect("32-bytes enforced by compiler"),
            state: match mode {
                EncryptionMode::Normal => EncryptorState::Normal,
                EncryptionMode::Suffix => EncryptorState::Suffix(StdRng::from_entropy()),
                EncryptionMode::Lite => EncryptorState::Lite(OsRng.gen()),
            },
        }
    }

    /// Encrypts packet in-place, updating any necessary values.
    pub fn encrypt<T>(&mut self, pkt: &mut Packet<T>) -> Result<(), aead::Error>
    where
        T: AsRef<[u8]> + AsMut<[u8]>,
    {
        match &mut self.state {
            EncryptorState::Normal => {
                // use the packet header as a nonce
                let mut nonce = [0u8; NONCE_SIZE];
                (&mut nonce[0..Packet::<T>::HEADER_LEN]).copy_from_slice(pkt.header());

                // encrypt
                let payload_len = pkt.payload_len();
                let tag = self.aead.encrypt_in_place_detached(
                    &nonce.into(),
                    b"",
                    &mut pkt.payload_mut()[..payload_len],
                )?;

                pkt.tag_mut().copy_from_slice(&tag[..]);

                // no need to finalize anything here; we're done.
                Ok(())
            }
            EncryptorState::Suffix(rng) => {
                // generate a new nonce
                let mut nonce = [0u8; NONCE_SIZE];
                rng.fill_bytes(&mut nonce);

                // encrypt
                let payload_len = pkt.payload_len();
                let tag = self.aead.encrypt_in_place_detached(
                    &nonce.into(),
                    b"",
                    &mut pkt.payload_mut()[..payload_len],
                )?;

                pkt.tag_mut().copy_from_slice(&tag[..]);

                // append nonce to the end
                (&mut pkt.payload_mut()[payload_len..payload_len + NONCE_SIZE])
                    .copy_from_slice(&nonce);
                pkt.set_payload_len(payload_len + NONCE_SIZE);

                Ok(())
            }
            EncryptorState::Lite(next_nonce) => {
                // get nonce and increment
                let mut nonce = [0u8; NONCE_SIZE];
                (&mut nonce[0..4]).copy_from_slice(&next_nonce.to_be_bytes());
                *next_nonce = next_nonce.overflowing_add(1).0;

                // encrypt
                let payload_len = pkt.payload_len();
                let tag = self.aead.encrypt_in_place_detached(
                    &nonce.into(),
                    b"",
                    &mut pkt.payload_mut()[..payload_len],
                )?;

                pkt.tag_mut().copy_from_slice(&tag[..]);

                // append nonce to the end
                (&mut pkt.payload_mut()[payload_len..payload_len + 4])
                    .copy_from_slice(&nonce[0..4]);
                pkt.set_payload_len(payload_len + 4);

                Ok(())
            }
        }
    }
}

impl Debug for Encryptor {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str("Encryptor(_)")
    }
}
