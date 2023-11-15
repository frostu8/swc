//! Types helpful for interacting with the youtube-dl command line.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::process::Command;

use std::process::Stdio;
use std::fmt::{self, Display, Formatter};
use std::sync::OnceLock;

use twilight_model::channel::message::embed::{
    Embed, EmbedAuthor, EmbedThumbnail,
};

use serde::Deserialize;

use tracing::instrument;

//use crate::voice::{Source, source::Error as SourceError};

static YTDL_EXECUTABLE: OnceLock<String> = OnceLock::new();

/// The `youtube-dl` executable.
pub fn ytdl_executable() -> &'static str {
    YTDL_EXECUTABLE.get().expect("ytdl executable initialized at startup")
}

pub fn init_ytdl_executable<F>(f: F) -> &'static str
where
    F: FnOnce() -> String
{
    YTDL_EXECUTABLE.get_or_init(f)
}

/// The result of a `youtube-dl` query.
#[derive(Debug)]
pub enum Query {
    /// A track was found.
    Track(Track),
    /// A playlist was found.
    Playlist(Playlist),
}

impl Query {
    /// Queries `youtube-dl` with the provided string.
    ///
    /// Also produces a [`Source`] if the result is a [`Track`].
    ///
    /// # Warning
    /// Because yt-dlp has to do some heavy networking overhead, this is a very
    /// slow operation, and has a tendency to time things out. Offload this
    /// work to a new async task and communicate the completion of the task
    /// through message passing.
    #[instrument(name = "Query::query")]
    pub async fn query(query: &str) -> Result<Query, QueryError> {
        let mut ytdl = Command::new(ytdl_executable())
            .args(&["--yes-playlist", "--flat-playlist", "-J", query])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(QueryError::Io)?;

        let stdout = ytdl.stdout.take().unwrap();
        let stderr = ytdl.stderr.take().unwrap();

        async fn read_to_end(
            mut stream: impl AsyncRead + Unpin,
        ) -> Result<String, std::io::Error> {
            let mut out = String::new();
            stream.read_to_string(&mut out).await.map(|_| out)
        }

        // wait for the query to finish
        let (_exit, out, err) = tokio::try_join!(
            ytdl.wait(),
            read_to_end(stdout),
            YtdlError::from_ytdl(BufReader::new(stderr)),
        )
            .map_err(QueryError::Io)?;

        if let Some(err) = err {
            Err(QueryError::Ytdl(err))
        } else {
            if output_is_playlist(&out) {
                Query::playlist_from_json(&out)
            } else {
                // not a playlist, or an error occured
                Query::track_from_json(&out)
            }
        }
    }

    fn playlist_from_json(
        json: &str,
    ) -> Result<Query, QueryError> {
        // parse json data
        #[derive(Deserialize)]
        struct YtdlPlaylist {
            title: String,
            uploader: String,
            #[serde(default)]
            uploader_url: Option<String>,
            webpage_url: String,
            #[serde(default)]
            thumbnail: Option<String>,
            entries: Vec<YtdlQuery>,
        }

        let YtdlPlaylist {
            title,
            uploader,
            uploader_url,
            webpage_url,
            thumbnail,
            entries,
        } = serde_json::from_str(json).map_err(QueryError::Json)?;

        // create a playlist as the result
        let playlist = Playlist {
            url: webpage_url,
            title,
            author: Author {
                name: uploader,
                url: uploader_url,
            },
            thumbnail_url: thumbnail,
            tracks: entries
                .into_iter()
                // skip privated videos (wtf)
                .filter_map(|entry| entry.try_into().ok())
                .collect(),
        };

        Ok(Query::Playlist(playlist))
    }

    fn track_from_json(
        json: &str,
    ) -> Result<Query, QueryError> {
        // parse json data
        let track: YtdlQuery = serde_json::from_str(json)
            .map_err(QueryError::Json)?;

        track
            .try_into()
            .map(|track| Query::Track(track))
    }
}

#[derive(Deserialize)]
struct YtdlQuery {
    id: String,
    webpage_url: Option<String>,
    title: String,
    uploader: Option<String>,
    #[serde(default)]
    uploader_url: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    thumbnails: Option<Vec<YtdlThumbnail>>,
}

#[derive(Deserialize)]
struct YtdlThumbnail {
    url: String,
    height: u32,
    width: u32,
}

fn output_is_playlist(out: &str) -> bool {
    if let Some(from) = out.find(r#""_type":"#) {
        let from = from + 8;
        match out[from..].find(&[',', '}'] as &[_]) {
            Some(to) if out[from..from + to].trim() == r#""playlist""# => true,
            Some(_to) => false,
            _ => false,
        }
    } else {
        false
    }
}

/// A single `youtube-dl` track.
///
/// Produced from the output of a `youtube-dl` query.
#[derive(Clone, Debug)]
pub struct Track {
    /// A url which, when provided to `youtube-dl` should produce the same
    /// result.
    pub url: String,
    /// A visible title for a song.
    pub title: String,
    /// The author of the track.
    pub author: Author,
    /// The URL of the thumbnail of the track.
    pub thumbnail_url: Option<String>,
}

impl Track {
    /// Converts a `Track` to a readable embed.
    pub fn as_embed(&self) -> Embed {
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
            url: Some(url),
            video: None,
        }
    }
}

impl TryFrom<YtdlQuery> for Track {
    type Error = QueryError; 

    fn try_from(e: YtdlQuery) -> Result<Track, Self::Error> {
        let YtdlQuery {
            id,
            webpage_url,
            title,
            uploader,
            uploader_url,
            thumbnail,
            thumbnails,
        } = e;

        let url = match webpage_url {
            Some(url) => url,
            None => format!("https://www.youtube.com/watch?v={}", id),
        };

        // find thumbnail
        let thumbnail = thumbnail
            .or_else(|| thumbnails
                .unwrap_or_default()
                .into_iter()
                .reduce(|acc, t| {
                    if t.width > acc.width || t.height > acc.height {
                        t
                    } else {
                        acc
                    }
                })
                .map(|t| t.url));

        // create a track as the result
        Ok(Track {
            url,
            title,
            author: Author {
                name: uploader.ok_or_else(|| QueryError::PrivateVideo)?,
                url: uploader_url,
            },
            thumbnail_url: thumbnail,
        })
    }
}

/// Many `youtube-dl` tracks.
///
/// Produced from the output of a `youtube-dl` query.
#[derive(Clone, Debug)]
pub struct Playlist {
    /// A url which, when provided to `youtube-dl` should produce the same
    /// result.
    pub url: String,
    /// A visible title for the playlist.
    pub title: String,
    /// The author of the playlist.
    pub author: Author,
    /// The URL of the thumbnail of the playlist.
    pub thumbnail_url: Option<String>,
    /// The tracks of the playlist.
    pub tracks: Vec<Track>,
}

impl Playlist {
    /// Converts a `Playlist` to a readable embed.
    pub fn as_embed(&self) -> Embed {
        let Playlist {
            url,
            title,
            author,
            thumbnail_url,
            tracks,
            ..
        } = self.clone();

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
                .or_else(|| tracks
                    .iter()
                    .next()
                    .and_then(|t| t.thumbnail_url.clone()))
                .map(|url| EmbedThumbnail {
                    url: url,
                    height: None,
                    width: None,
                    proxy_url: None,
                }),
            url: Some(url),
            video: None,
        }
    }
}

/// An author of a track.
#[derive(Clone, Debug)]
pub struct Author {
    /// The name of the author.
    pub name: String,
    /// A URL to the author's channel.
    pub url: Option<String>,
}

/// An error that can occur querying `youtube-dl`.
#[derive(Debug)]
pub enum QueryError {
    /// There was an IO error.
    Io(std::io::Error),
    /// UTF8 error while processing input JSON.
    Utf8(std::str::Utf8Error),
    /// Serialization error while processing input JSON.
    Json(serde_json::Error),
    /// Ytdl produced an error.
    Ytdl(YtdlError),
    /// The video that was queried is private.
    PrivateVideo,
}

impl Display for QueryError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            QueryError::Io(err) => Display::fmt(err, f),
            QueryError::Utf8(err) => Display::fmt(err, f),
            QueryError::Json(err) => Display::fmt(err, f),
            QueryError::Ytdl(err) => Display::fmt(err, f),
            QueryError::PrivateVideo => f.write_str(
                "query result is privated or otherwise not visible",
            ),
        }
    }
}

impl std::error::Error for QueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            QueryError::Io(err) => Some(err),
            QueryError::Utf8(err) => Some(err),
            QueryError::Json(err) => Some(err),
            QueryError::Ytdl(err) => Some(err),
            _ => None,
        }
    }
}

/// An error from a `youtube-dl` command.
#[derive(Debug)]
pub struct YtdlError {
    message: String,
}

impl YtdlError {
    /// Reads an error from a stream, most likely stdout of a `youtube-dl`
    /// process.
    ///
    /// If an error is not found, returns `None`.
    ///
    /// `youtube-dl` error codes are meaningless, so this is the only way we can
    /// get a message from `youtube-dl`.
    pub async fn from_ytdl<T>(stderr: T) -> Result<Option<YtdlError>, std::io::Error>
    where
        T: AsyncBufRead + Unpin,
    {
        // youtube-dl stderr looks like this:
        // WARNING: warning
        // ERROR: error <-- this is what we want
        const ERROR_PREFIX: &'static str = "ERROR:";

        let mut lines = stderr.lines();
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

