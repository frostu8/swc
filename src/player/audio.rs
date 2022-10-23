//! Audio management.

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

use std::fmt::{self, Display, Formatter};
use std::process::Stdio;

use crate::constants::{SAMPLE_RATE, STEREO_FRAME_SIZE};

use opus::{Application, Channels, Encoder};

/// A ytdl audio source.
///
/// Encodes PCM32f @ 48000kHz into Opus-encoded audio. It's better to leave most
/// of the coding to ffmpeg, or another process, and that's what this does.
pub struct Source {
    ytdl: Child,
    ffmpeg: Child,

    coder: Encoder,
    buf: [f32; STEREO_FRAME_SIZE],
    buf_len: usize,
}

impl Source {
    /// Reads the next Opus packet into the buffer.
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        loop {
            let len = self
                .ffmpeg
                .stdout
                .as_mut()
                .unwrap()
                .read(bytemuck::cast_slice_mut(&mut self.buf[self.buf_len..]))
                .await?;

            if len > 0 {
                self.buf_len += len / std::mem::size_of::<f32>();
                if self.buf_len >= self.buf.len() {
                    break;
                }
            } else {
                return Ok(0);
            }
        }

        if self.buf_len > 0 {
            // encode
            let len = self.coder.encode_float(&self.buf[..self.buf_len], buf)?;
            self.buf_len = 0;
            Ok(len)
        } else {
            Ok(0)
        }
    }

    /// Kills the processes associated with the `Source`.
    pub async fn kill(&mut self) -> Result<(), Error> {
        self.ytdl.kill().await?;
        self.ffmpeg.kill().await?;
        Ok(())
    }

    /// Creates a new `Source` from a `ytdl` link.
    pub async fn new(url: &str) -> Result<Source, Error> {
        let mut ytdl = Command::new("youtube-dl")
            .args(&[
                "-f",
                "webm[abr>0]/bestaudio/best",
                "-R",
                "infinite",
                "-q",
                url,
                "-o",
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let ytdl_stdio: Stdio = ytdl.stdout.take().unwrap().try_into().unwrap();

        let ffmpeg = Command::new("ffmpeg")
            .args(&[
                "-i",
                "pipe:0",
                "-ac",
                "2",
                "-ar",
                "48000",
                "-f",
                "s16le",
                "-acodec",
                "pcm_f32le",
                "-loglevel",
                "quiet",
                "pipe:1",
            ])
            .stdin(ytdl_stdio)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        Ok(Source {
            ytdl,
            ffmpeg,
            coder: Encoder::new(SAMPLE_RATE as u32, Channels::Stereo, Application::Audio)?,
            buf: [0f32; STEREO_FRAME_SIZE],
            buf_len: 0,
        })
    }
}

/// Error type for [`Source`].
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Codec(opus::Error),
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Error {
        Error::Io(err)
    }
}

impl From<opus::Error> for Error {
    fn from(err: opus::Error) -> Error {
        Error::Codec(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Error::Io(err) => write!(f, "io: {}", err),
            Error::Codec(err) => write!(f, "coded: {}", err),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            Error::Codec(err) => Some(err),
        }
    }
}
