//! Multithreaded, shared music queues based on `youtube-dl`.

use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::io::{AsyncRead, AsyncReadExt};

use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use std::collections::VecDeque;
use std::process::Stdio;

use twilight_model::channel::embed::{Embed, EmbedAuthor, EmbedThumbnail};

use serde::Deserialize;

use super::audio::{Source as AudioSource, Error as AudioError};

use crate::ytdl;

/// A multithreaded, shared music queue.
///
/// [`Queue::next`]doesn't actually remove the source. Instead, the source moves
/// into the graveyard of the queue. If the graveyard gets too large (determined
/// by `keep_count` of [`Queue::new`]), it is then disposed of. This is to
/// support quick backtracking in the queue.
pub struct Queue(Arc<QueueRef>);

struct QueueRef {
    queue: RwLock<VecDeque<Source>>,
    // this is the index **after** the currently playing song
    head: AtomicUsize,

    // amount to keep in the graveyard
    keep_count: usize,
}

impl Queue {
    /// Creates a new, empty `Queue`.
    pub fn new(keep_count: usize) -> Queue {
        Queue(Arc::new(QueueRef {
            queue: RwLock::new(VecDeque::new()),
            keep_count,
            head: AtomicUsize::new(0),
        }))
    }

    /// Adds a new source to the back queue.
    ///
    /// Write-locks the queue.
    pub async fn push(&self, source: Source) {
        self.0.queue.write().await.push_back(source)
    }

    /// Advances the queue, returning the [`Source`] that was pushed into the
    /// graveyard.
    ///
    /// Write-locks the queue.
    pub async fn next(&self) -> Option<Source> {
        // this write lock ensures that we are the only thread with mutable
        // access, and any operations to the head are stable
        let mut queue = self.0.queue.write().await;

        let head = self.0.head.load(Ordering::Acquire);

        let source = match queue.get(head) {
            Some(source) => source.clone(),
            None => return None,
        };

        if head >= self.0.keep_count {
            // remove the last song in the graveyard, effectively moving the
            // queue relative to the head
            queue.pop_front();
        } else {
            // move the head relative to the queue
            self.0.head.store(head + 1, Ordering::Release);
        }

        // return source
        Some(source)
    }

    /// Backs up the queue, returning the [`Source`] now pulled out of the
    /// graveyard.
    ///
    /// Write-locks the queue.
    pub async fn prev(&self) -> Option<Source> {
        // this write lock ensures that we are the only thread with mutable
        // access, and any operations to the head are stable
        let queue = self.0.queue.write().await;

        let head = self.0.head.load(Ordering::Acquire);
        
        if head > 0 {
            // move the head relative to the queue
            self.0.head.store(head - 1, Ordering::Release);

            // head <= queue.len()
            Some(queue[head - 1].clone())
        } else {
            None
        }
    }
}

impl Default for Queue {
    fn default() -> Queue {
        Queue::new(15)
    }
}

/// A single entry in the queue.
#[derive(Debug, Clone)]
pub struct Source {
    url: String,
    title: String,
    author: Author,
    thumbnail_url: String,
}

/// The author of the source.
#[derive(Debug, Clone)]
struct Author {
    /// The name of the author.
    pub name: String,
    /// A hyperlink to the author's page.
    pub url: String,
}

impl Source {
    /// Creates a source from a url passed to `youtube-dl`.
    pub async fn ytdl(url: &str) -> Result<Source, Error> {
        // create process
        let mut ytdl = Command::new("ytdl")
            .args(&["-j", url])
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
            ytdl::Error::from_ytdl(stderr),
        )
            .map_err(Error::Io)?;

        if let Some(err) = err {
            Err(Error::Ytdl(err))
        } else {
            // parse
            let q = serde_json::from_str::<YtdlQuery>(&out)
                .expect("valid json from youtube-dl");

            Ok(Source {
                url: q.webpage_url,
                title: q.title,
                author: Author {
                    name: q.uploader,
                    url: q.uploader_url,
                },
                thumbnail_url: q.thumbnail,
            })
        }
    }

    /// Creates a detailed Discord embed from the `Source`.
    ///
    /// ```ignore
    /// let embed = Embed {
    ///     description: String::from("added to queue"),
    ///     ..source.to_embed()
    /// };
    /// ```
    pub fn to_embed(&self) -> Embed {
        let Source { url, title, author, thumbnail_url } = self.clone();

        Embed {
            author: Some(EmbedAuthor {
                name: author.name,
                url: Some(author.url),
                icon_url: None,
                proxy_icon_url: None,
            }),
            // TODO: color
            color: Some(0xFFFFFF),
            description: None,
            fields: Vec::new(),
            footer: None,
            image: None,
            kind: String::from("rich"),
            provider: None,
            title: Some(title),
            timestamp: None,
            thumbnail: Some(EmbedThumbnail {
                url: thumbnail_url,
                height: None,
                width: None,
                proxy_url: None,
            }),
            url: Some(url),
            video: None,
        }
    }

    /// Opens an audio stream to the source.
    pub async fn to_audio(&self) -> Result<AudioSource, AudioError> {
        AudioSource::new(&self.url).await
    }

    /// The url of the source.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The title of the source.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// A hyperlink to the thumbnail of the source.
    pub fn thumbnail_url(&self) -> &str {
        &self.thumbnail_url
    }

    /// The author's name.
    pub fn author_name(&self) -> &str {
        &self.author.name
    }

    /// A hyperlink to the author's page.
    pub fn author_url(&self) -> &str {
        &self.author.url
    }
}

/// An error for reading sources.
pub enum Error {
    /// Resulting JSON failed to decode.
    Json(serde_json::Error),
    /// Io error.
    Io(std::io::Error),
    /// Error from `youtube-dl`.
    Ytdl(ytdl::Error),
}

#[derive(Deserialize)]
struct YtdlQuery {
    webpage_url: String,
    title: String,
    thumbnail: String,
    uploader: String,
    uploader_url: String,
}
