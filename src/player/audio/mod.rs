//! Audio structs and helpers.

mod queue;

pub use queue::Queue;

use super::constants::{SAMPLE_RATE, STEREO_FRAME_SIZE};

use tokio::process::{Child, Command};
use tokio::io::{AsyncRead, AsyncReadExt, BufReader, AsyncBufReadExt};

use std::process::Stdio;
use std::fmt::{self, Display, Formatter};

use twilight_model::channel::embed::{Embed, EmbedAuthor, EmbedThumbnail};

use serde::Deserialize;

use opus::{Application, Encoder, Channels};

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
                .await
                .map_err(Error::Io)?;

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
            let len = self.coder.encode_float(&self.buf[..self.buf_len], buf).map_err(Error::Codec)?;
            self.buf_len = 0;
            Ok(len)
        } else {
            Ok(0)
        }
    }

    /// Kills the processes associated with the `Source`.
    pub async fn kill(&mut self) -> Result<(), Error> {
        self.ytdl.kill().await.map_err(Error::Io)?;
        self.ffmpeg.kill().await.map_err(Error::Io)?;
        Ok(())
    }

    /// Creates a new `Source` from a `ytdl` id.
    pub async fn new(id: &str) -> Result<Source, Error> {
        let mut ytdl = Command::new("youtube-dl")
            .args(&[
                "-f",
                "webm[abr>0]/bestaudio/best",
                "-R",
                "infinite",
                "-q",
                id,
                "-o",
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(Error::Io)?;

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
            .spawn()
            .map_err(Error::Io)?;

        Ok(Source {
            ytdl,
            ffmpeg,
            coder: Encoder::new(SAMPLE_RATE as u32, Channels::Stereo, Application::Audio).map_err(Error::Codec)?,
            buf: [0f32; STEREO_FRAME_SIZE],
            buf_len: 0,
        })
    }
}

/// A single entry in the queue.
#[derive(Debug, Clone)]
pub struct Track {
    id: String,
    title: String,
    author: Author,
    thumbnail_url: Option<String>,
    url: Option<String>,
}

/// The author of the track.
#[derive(Debug, Clone)]
struct Author {
    /// The name of the author.
    pub name: String,
    /// A hyperlink to the author's page.
    pub url: Option<String>,
}

impl Track {
    /// Creates a detailed Discord embed from the `Track`.
    ///
    /// ```ignore
    /// let embed = Embed {
    ///     description: String::from("added to queue"),
    ///     ..track.to_embed()
    /// };
    /// ```
    pub fn to_embed(&self) -> Embed {
        let Track { url, title, author, thumbnail_url, .. } = self.clone();

        Embed {
            author: Some(EmbedAuthor {
                name: author.name,
                url: author.url,
                icon_url: None,
                proxy_icon_url: None,
            }),
            // TODO: color
            color: Some(0xEE1428),
            description: None,
            fields: Vec::new(),
            footer: None,
            image: None,
            kind: String::from("rich"),
            provider: None,
            title: Some(title),
            timestamp: None,
            thumbnail: thumbnail_url
                .map(|url| EmbedThumbnail {
                    url: url,
                    height: None,
                    width: None,
                    proxy_url: None,
                }),
            url,
            video: None,
        }
    }

    /// Opens an audio stream to the track.
    pub async fn open(&self) -> Result<Source, Error> {
        Source::new(&self.id).await
    }

    /// The url of the track.
    pub fn url(&self) -> Option<&str> {
        self.url.as_ref().map(|x| &**x)
    }

    /// The title of the track.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// A hyperlink to the thumbnail of the track.
    pub fn thumbnail_url(&self) -> Option<&str> {
        self.thumbnail_url.as_ref().map(|x| &**x)
    }

    /// The author's name.
    pub fn author_name(&self) -> &str {
        &self.author.name
    }

    /// A hyperlink to the author's page.
    pub fn author_url(&self) -> Option<&str> {
        self.author.url.as_ref().map(|x| &**x)
    }
}

/// A collection of tracks.
#[derive(Clone)]
pub struct Playlist {
    id: String,
    title: String,
    author: Author,
    url: String,
    entries: Vec<Track>,
}

impl Playlist {
    /// Creates a detailed Discord embed from the `Playlist`.
    ///
    /// ```ignore
    /// let embed = Embed {
    ///     description: String::from("added playlist to queue"),
    ///     ..track.to_embed()
    /// };
    /// ```
    pub fn to_embed(&self) -> Embed {
        let Playlist { url, title, author, .. } = self.clone();

        Embed {
            author: Some(EmbedAuthor {
                name: author.name,
                url: author.url,
                icon_url: None,
                proxy_icon_url: None,
            }),
            // TODO: color
            color: Some(0xEE1428),
            description: None,
            fields: Vec::new(),
            footer: None,
            image: None,
            kind: String::from("rich"),
            provider: None,
            title: Some(title),
            timestamp: None,
            thumbnail: None,
            url: Some(url),
            video: None,
        }
    }

    /// The url of the playlist.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The title of the playlist.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The author's name.
    pub fn author_name(&self) -> &str {
        &self.author.name
    }

    /// A hyperlink to the author's page.
    pub fn author_url(&self) -> Option<&str> {
        self.author.url.as_ref().map(|x| &**x)
    }

    /// The entries in the playlist.
    pub fn entries(&self) -> &[Track] {
        &self.entries
    }

    /// Unwraps the entries in the playlist.
    pub fn into_entries(self) -> Vec<Track> {
        self.entries
    }
}

/// A ytdl query.
pub enum Query {
    // A single track.
    Track(Track),
    // A playlist.
    Playlist(Playlist),
}

impl Query {
    /// Queries `youtube-dl` with the provided url.
    pub async fn new(url: &str) -> Result<Query, Error> {
        // create process
        let mut ytdl = Command::new("youtube-dl")
            .args(&["--yes-playlist", "--flat-playlist", "-J", url])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(Error::Io)?;

        let stdout = ytdl.stdout.take().unwrap();
        let stderr = ytdl.stderr.take().unwrap();

        async fn read_to_end(
            mut stream: impl AsyncRead + Unpin,
        ) -> Result<String, std::io::Error> {
            let mut out = String::new();
            stream.read_to_string(&mut out).await.map(|_| out)
        }

        // wait for it to finish
        let (_exit, out, err) = tokio::try_join!(
            ytdl.wait(),
            read_to_end(stdout),
            YtdlError::from_ytdl(stderr),
        )
            .map_err(Error::Io)?;

        if let Some(err) = err {
            Err(Error::Ytdl(err))
        } else {
            // determine if this is a playlist
            if output_is_playlist(&out) {
                // parse
                let q = serde_json::from_str::<YtdlPlaylist>(&out)
                    .map_err(Error::Json)?;

                Ok(Query::Playlist(q.into()))
            } else {
                // parse
                let q = serde_json::from_str::<YtdlQuery>(&out)
                    .map_err(Error::Json)?;

                Ok(Query::Track(q.into()))
            }
        }
    }
}

fn output_is_playlist(out: &str) -> bool {
    if let Some(from) = out.find(r#""_type":"#) {
        let from = from + 8;
        match out[from..].find(&[',', '}'] as &[_]) {
            Some(to) if out[from..from + to].trim() == r#""playlist""# => true,
            Some(to) => {
                info!("{}", out[from..from + to].trim());
                false
            }
            _ => false,
        }
    } else {
        false
    }
}

#[derive(Deserialize)]
struct YtdlQuery {
    id: String,
    title: String,
    uploader: String,
    #[serde(default)]
    webpage_url: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    uploader_url: Option<String>,
}

impl From<YtdlQuery> for Track {
    fn from(q: YtdlQuery) -> Track {
        Track {
            id: q.id,
            title: q.title,
            author: Author {
                name: q.uploader,
                url: q.uploader_url,
            },
            thumbnail_url: q.thumbnail,
            url: q.webpage_url,
        }
    }
}

#[derive(Deserialize)]
struct YtdlPlaylist {
    id: String,
    title: String,
    uploader: String,
    uploader_url: String,
    webpage_url: String,
    entries: Vec<YtdlQuery>,
}

impl From<YtdlPlaylist> for Playlist {
    fn from(q: YtdlPlaylist) -> Playlist {
        Playlist {
            id: q.id,
            title: q.title,
            url: q.webpage_url,
            author: Author {
                name: q.uploader,
                url: Some(q.uploader_url),
            },
            entries: q.entries.into_iter().map(Into::into).collect(),
        }
    }
}

/// An error for reading and parsing audio tracks.
#[derive(Debug)]
pub enum Error {
    /// Resulting JSON failed to decode.
    Json(serde_json::Error),
    /// Io error.
    Io(std::io::Error),
    /// Codec error.
    Codec(opus::Error),
    /// Error from `youtube-dl`.
    Ytdl(YtdlError),
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Error::Json(err) => Display::fmt(err, f),
            Error::Io(err) => Display::fmt(err, f),
            Error::Codec(err) => Display::fmt(err, f),
            Error::Ytdl(err) => Display::fmt(err, f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Json(err) => Some(err),
            Error::Io(err) => Some(err),
            Error::Codec(err) => Some(err),
            Error::Ytdl(err) => Some(err),
        }
    }
}

/// An error from a `youtube-dl` command.
#[derive(Debug)]
pub struct YtdlError {
    message: String,
}

impl YtdlError {
    /// Reads an error from a stream.
    ///
    /// If an error is not found, returns `None`.
    ///
    /// `youtube-dl` error codes are meaningless, so this is the only way we can
    /// get a message from `youtube-dl`.
    pub async fn from_ytdl<T>(stream: T) -> Result<Option<YtdlError>, std::io::Error>
    where
        T: AsyncRead + Unpin,
    {
        // youtube-dl stderr looks like this:
        // WARNING: warning
        // ERROR: error <-- this is what we want
        const ERROR_PREFIX: &'static str = "ERROR:";

        let stream = BufReader::new(stream);

        let mut lines = stream.lines();
        while let Some(line) = lines.next_line().await? {
            if line.starts_with(ERROR_PREFIX) {
                return Ok(Some(YtdlError {
                    message: line[ERROR_PREFIX.len()..].trim().to_owned(),
                }));
            }
        }

        Ok(None)
    }

    /// The message of the error.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for YtdlError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for YtdlError {}
