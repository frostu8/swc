//! A "simple" abstraction over the Discord voice API.
//!
//! This is designed to decouple all the gritty audio processing from the
//! higher level processing of queuing and dequeuing tracks.
//!
//! # Players
//! Voice gateway connectivity and UDP connectivity are all managed in a single
//! task that coordinates all movement on the voice connection.
//!
//! Communication to this task is abstracted away as a [`Player`]. This is a
//! message-based simple protocol that holds a voice gateway connection, with
//! the assistance of the main Discord gateway. **Each `Player` should only
//! ever associate with one gateway. Mixing gateways or shards will cause
//! catastrophic results and difficult-to-debug errors.**
//!
//! The actual encoding of sound data is done by [`Source`]. When a `Player` is
//! told to play a `Source`, it will first wait for the source to begin
//! producing data before sending an [`EventType::Playing`] event. `Player`s
//! will play a `Source` until the source runs out of data, and then it will
//! send the messages to indicate that the source has finished playing with
//! [`EventType::Stopped`].
//!
//! There are also methods to pause and resume audio playback. No events will
//! be produced when these methods are called. These are just simple ways to
//! pause audio playback until it needs to be resumed.
//!
//! # Caveats
//! This is poorly designed! Not my amazing voice protocol, the Discord voice
//! protocol is poorly designed! Why is the main gateway and voice gateway so
//! coupled like this, what is the benefit?!
//!
//! # The Rant
//! Click to [skip to the module definitions](#reexports).
//!
//! Okay, now that I've gotten this out of the way, I can finally address the
//! elephant in the room: there already exists a production-ready solution to
//! this problem, it's called [`songbird`][1].
//!
//! You are correct. I don't have an easy to explain reason why I didn't use
//! it. This isn't like the comparative wasteland that is the Bevy ecosystem.
//! However, I do have a couple not easy to explain reasons and I will gladly
//! detail them below.
//!
//! First, `songbird` is a textbook-standard example of scope creep. I get it,
//! this is from the same people that made [`serenity`][2], I understand that
//! `songbird` is meant to be a "batteries-included" experience for developers.
//! But there's an entire audio mixer and encoder packed into this monstrosity,
//! **along with** all the hacky solutions to sidestep the messy problems
//! Discord created with their Voice API. This is like if I asked for a song
//! for my game I'm making and you said "Oh, would you like an **entire
//! sound suite**, complete with a **soundtrack** and **sound effects** with
//! that?" For each voice connection, **5** separate processes run
//! concurrently, and for almost all use cases, the one that handles receiving
//! voice data is completely useless.
//!
//! Besides, `ffmpeg` can do everything `songbird`'s built in audio system can
//! do, and then several thousand other things. Almost every user is going to
//! use `ffmpeg` anyways, so they might as well use the filters on `ffmpeg` to
//! achieve the results they want. I'm not saying `songbird` *can't* have an
//! audio mixing system, but it would have been worth decoupling the connection
//! management from the audio mixing, or introducing a BYOE (bring your own
//! encoder) mentality. I know that Rust's async-await traits are in a rough
//! place right now, and the last thing you want to do is box several Opus
//! frames in a future every second, so I can understand why BYOE might be
//! difficult with the current ecosystem. But, it's too late, [we're already
//! sending giant Opus packets through tokio channels][1], so I don't think
//! boxing packets is a big issue.
//!
//! This solution is the complete opposite of that. The `voice` module only
//! understands how to communicate to the Discord API. To the module, the
//! contents of the audio are opaque. The [`source`] module is responsible for
//! all the audio processing, which is pretty simple because ffmpeg does most
//! of the heavy lifting. It is rather limiting that the `source` can only
//! understand `ffmpeg` calls and simple piping, but it's all me (and possibly
//! most Discord music bot writers) really need.
//!
//! Also, it feels good to write my own audio connection management. It's
//! sometimes enlightening to reinvent the wheel like this, even if it's
//! something convoluted like audio codecs and sound engineering. In fact, the
//! more convoluted and impossible it is to read related write ups, the more
//! enlightening the final result is.
//!
//! [1]: https://github.com/serenity-rs/songbird
//! [2]: https://github.com/serenity-rs/serenity
//! [3]: https://github.com/serenity-rs/songbird/blob/df8ee0ffcad03c26db489c356e183a7e1190b04c/src/driver/tasks/mixer.rs#L523-L527

pub mod constants;
pub mod error;
mod streamer;
pub mod rtp;
pub mod source;
pub mod ws;

pub use error::Error;
pub use source::Source;

use streamer::{Status, PacketStreamer};

use tracing::{error, debug, instrument, warn};

use std::sync::{atomic::{AtomicBool, Ordering}, Arc};

use rtp::Socket;
use ws::{payload::Speaking, Connection, Session};

use tokio::task::JoinHandle;
use tokio::sync::{
    RwLock, RwLockReadGuard,
    mpsc::{self, UnboundedReceiver, UnboundedSender},
};
use tokio::time::{Instant, Duration, timeout_at};

use twilight_model::{
    voice::VoiceState,
    gateway::payload::incoming::{VoiceServerUpdate, VoiceStateUpdate},
    id::{Id, marker::{GuildMarker, UserMarker}},
};

/// An audio sink that plays audio to a channel.
///
/// See the [module level documentation][1] for more information.
///
/// [1]: crate::voice
pub struct Player {
    task: JoinHandle<()>,

    state: Arc<PlayerState>,
    gateway_tx: UnboundedSender<GatewayEvent>,
    command_tx: UnboundedSender<Command>,
}

impl Player {
    /// Creates a new `Player`.
    ///
    /// You typically shouldn't need to manually construct this.
    pub fn new(
        user_id: impl Into<Id<UserMarker>>,
        guild_id: impl Into<Id<GuildMarker>>,
        event_tx: UnboundedSender<Event>,
    ) -> Player {
        let user_id = user_id.into();
        let guild_id = guild_id.into();

        let (gateway_tx, gateway_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        // TODO: initial state?
        let initial_state = VoiceState {
            channel_id: None,
            guild_id: Some(guild_id),
            user_id: user_id,
            deaf: false,
            mute: false,
            self_deaf: false,
            self_mute: false,
            self_stream: false,
            self_video: false,
            suppress: false,
            session_id: String::new(),
            member: None,
            request_to_speak_timestamp: None,
        };

        let state = Arc::new(PlayerState {
            user_id,
            guild_id,
            voice_state: RwLock::new(initial_state),
            playing: AtomicBool::default(),
            ready: AtomicBool::default(),
        });
        let state_clone = state.clone();

        // start player task
        let task = tokio::spawn(async move {
            let task = PlayerTask::new(
                state_clone,
                event_tx,
                gateway_rx,
                command_rx,
            ).await;

            match task {
                Ok(task) => task.run().await,
                Err(err) => error!(%err, "voice init error"),
            }
        });

        Player {
            task,
            gateway_tx,
            command_tx,
            state,
        }
    }

    /// Checks if the player is closed or dead.
    pub fn is_closed(&self) -> bool {
        self.task.is_finished()
    }

    /// Plays a new source.
    pub fn play(&self, source: Source) -> Result<(), PlayerClosed> {
        self.command_tx
            .send(Command::Play(source))
            .map_err(|_| PlayerClosed)
    }

    /// Pauses the currently playing source.
    pub fn pause(&self) -> Result<(), PlayerClosed> {
        self.command_tx
            .send(Command::Pause)
            .map_err(|_| PlayerClosed)
    }

    /// Resumes any currently playing source.
    pub fn resume(&self) -> Result<(), PlayerClosed> {
        self.command_tx
            .send(Command::Resume)
            .map_err(|_| PlayerClosed)
    }

    /// Stops any playing sources.
    pub fn stop(&self) -> Result<(), PlayerClosed> {
        self.command_tx
            .send(Command::Stop)
            .map_err(|_| PlayerClosed)
    }

    /// If the player is playing a sound.
    pub fn playing(&self) -> bool {
        self.state.playing.load(Ordering::Acquire)
    }

    /// The guild id of the player.
    pub fn guild_id(&self) -> Id<GuildMarker> {
        self.state.guild_id
    }

    /// Gets the voice state of the player.
    pub async fn voice_state(&self) -> Result<RwLockReadGuard<VoiceState>, PlayerClosed> {
        if self.is_closed() {
            Err(PlayerClosed)
        } else {
            Ok(self.state.voice_state.read().await)
        }
    }

    /// Sends a voice state update event to the player.
    ///
    /// You typically shouldn't call this manually.
    pub fn voice_state_update(&self, ev: Box<VoiceStateUpdate>) -> Result<(), PlayerClosed> {
        self.gateway_tx.send(GatewayEvent::VoiceStateUpdate(ev))
            .map_err(|_| PlayerClosed)
    }

    /// Sends a voice server update event to the player.
    ///
    /// You typically shouldn't call this manually.
    pub fn voice_server_update(&self, ev: VoiceServerUpdate) -> Result<(), PlayerClosed> {
        self.gateway_tx.send(GatewayEvent::VoiceServerUpdate(ev))
            .map_err(|_| PlayerClosed)
    }
}

/// An error for when a player is closed.
#[derive(Debug)]
pub struct PlayerClosed;

/// An event that a [`Player`] can produce.
#[derive(Debug)]
pub struct Event {
    pub guild_id: Id<GuildMarker>,
    pub kind: EventType,
}

#[derive(Debug)]
pub enum EventType {
    /// The player is ready to play a sound.
    Ready,
    /// The player has started a sound.
    Playing,
    /// The player stopped playing a sound.
    Stopped,
    /// The player has crashed with an error.
    Error(Error),
}

enum Command {
    Play(Source),
    Pause,
    Resume,
    Stop,
}

#[derive(Debug)]
enum GatewayEvent {
    VoiceStateUpdate(Box<VoiceStateUpdate>),
    VoiceServerUpdate(VoiceServerUpdate),
}

struct PlayerState {
    voice_state: RwLock<VoiceState>,
    playing: AtomicBool,
    ready: AtomicBool,

    user_id: Id<UserMarker>,
    guild_id: Id<GuildMarker>,
}

/// The task that runs behind each player.
struct PlayerTask {
    state: Arc<PlayerState>,
    gateway_rx: UnboundedReceiver<GatewayEvent>,
    command_rx: UnboundedReceiver<Command>,
    event_tx: UnboundedSender<Event>,
    
    ws: Connection,
    rtp: Socket,

    streamer: PacketStreamer,
}

impl PlayerTask {
    /// Creates a new `PlayerTask`.
    ///
    /// This will timeout with a [`Error::CannotJoin`] if 5 seconds pass
    /// without a connection.
    pub async fn new(
        state: Arc<PlayerState>,
        event_tx: UnboundedSender<Event>,
        mut gateway_rx: UnboundedReceiver<GatewayEvent>,
        command_rx: UnboundedReceiver<Command>,
    ) -> Result<PlayerTask, Error> {
        let deadline = Instant::now() + Duration::from_millis(5000);

        // begin initialization: wait for events
        let mut vstu: Option<Box<VoiceStateUpdate>> = None;
        let mut vseu: Option<VoiceServerUpdate> = None;

        while let Ok(Some(ev)) = timeout_at(deadline, gateway_rx.recv()).await {
            match ev {
                GatewayEvent::VoiceStateUpdate(ev) if ev.0.user_id == state.user_id => {
                    vstu = Some(ev);
                },
                GatewayEvent::VoiceServerUpdate(ev) => {
                    vseu = Some(ev);
                },
                _ => ()
            };

            if vstu.is_some() && vseu.is_some() {
                break;
            }
        }

        // establish session
        let session = if let (Some(vseu), Some(vstu)) = (vseu, vstu) {
            let res = Session {
                guild_id: state.guild_id,
                user_id: state.user_id,
                endpoint: vseu.endpoint.unwrap(),
                token: vseu.token,
                session_id: vstu.0.session_id.clone(),
            };

            // update state
            *state.voice_state.write().await = vstu.0;

            res
        } else {
            return Err(Error::CannotJoin);
        };

        let (ws, rtp) = timeout_at(deadline, Connection::connect(session))
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(Error::from)?;

        state.ready.store(true, Ordering::Release);

        let _ = event_tx.send(Event {
            guild_id: state.guild_id,
            kind: EventType::Ready,
        });

        Ok(PlayerTask {
            state,
            gateway_rx,
            command_rx,
            event_tx,

            ws,
            rtp,

            streamer: PacketStreamer::new(Duration::from_millis(200)),
        })
    }

    #[instrument(skip(self))]
    async fn reconnect(&mut self) -> Result<(), Error> {
        if self.state.voice_state.read().await.channel_id.is_none() {
            return Err(Error::Disconnected);
        }

        let deadline = Instant::now() + Duration::from_millis(5000);

        loop {
            match timeout_at(deadline, self.gateway_rx.recv()).await {
                Ok(Some(ev)) => match ev {
                    GatewayEvent::VoiceServerUpdate(vseu) => {
                        // server update; reconnect
                        self.voice_server_update(vseu).await?;
                        return Ok(());
                    }
                    GatewayEvent::VoiceStateUpdate(vstu) => {
                        if vstu.channel_id.is_none() {
                            return Err(Error::Disconnected);
                        }

                        // update state
                        *self.state.voice_state.write().await = vstu.0;
                    }
                }
                Ok(None) => {
                    return Err(Error::GatewayClosed);
                }
                Err(_) => {
                    return Err(Error::Timeout);
                }
            }
        }
    }

    #[instrument(skip(self))]
    async fn wait_for_gateway(&mut self) -> Result<(), Error> {
        warn!("disconnected; waiting for gateway");

        let deadline = Instant::now() + Duration::from_millis(5000);

        loop {
            match timeout_at(deadline, self.gateway_rx.recv()).await {
                Ok(Some(GatewayEvent::VoiceServerUpdate(vseu))) => {
                    // server update; reconnect
                    self.voice_server_update(vseu).await?;
                    return Ok(());
                }
                Ok(Some(GatewayEvent::VoiceStateUpdate(vstu))) if vstu.0.user_id == self.state.user_id => {
                    *self.state.voice_state.write().await = vstu.0;
                }
                Ok(Some(GatewayEvent::VoiceStateUpdate(_))) => (),
                Ok(None) => return Err(Error::GatewayClosed),
                Err(_) => return Err(Error::Timeout),
            }
        }
    }

    async fn set_playing(&mut self, playing: bool) {
        if self.state.playing.fetch_xor(playing, Ordering::Acquire) {
            self.state.playing.store(playing, Ordering::Release);
            let kind = if playing {
                EventType::Playing
            } else {
                EventType::Stopped
            };

            let _ = self.event_tx.send(Event {
                guild_id: self.state.guild_id,
                kind,
            });
        }
    }

    /// Runs the task, consuming it.
    ///
    /// **Do not call this on the main thread, as it will not terminate.**
    pub async fn run(mut self) {
        if let Err(err) = self.run_inner().await {
            let _ = self.event_tx.send(Event {
                guild_id: self.state.guild_id,
                kind: EventType::Error(err),
            });
        }

        // attempt do cleanup
        if let Some(mut source) = self.streamer.take_source() {
            if let Err(err) = source.close().await {
                error!(%err, "close source error");
            }
        }
    }

    #[instrument("player_loop", skip(self))]
    async fn run_inner(&mut self) -> Result<(), Error> {
        loop {
            tokio::select! {
                biased;

                // voice websocket event
                ev = self.ws.recv() => {
                    match ev {
                        Some(Ok(ev)) => {
                            // discard event
                            debug!("voice ev: {:?}", ev);
                        }
                        Some(Err(err)) if err.disconnected() => {
                            // normal disconnect event
                            // wait for gateway notification
                            self.wait_for_gateway().await?;
                        }
                        Some(Err(err)) if err.can_resume() => {
                            // normal disconnect event
                            // attempt to reconnect if the disconnect was a move
                            self.reconnect().await?;
                        }
                        Some(Err(err)) => return Err(Error::from(err)),
                        None => break,
                    }
                }
                // main gateway event
                ev = self.gateway_rx.recv() => {
                    match ev {
                        Some(GatewayEvent::VoiceServerUpdate(vseu)) => {
                            // server update; reconnect
                            self.voice_server_update(vseu).await?;
                        }
                        Some(GatewayEvent::VoiceStateUpdate(vstu)) if vstu.0.user_id == self.state.user_id => {
                            *self.state.voice_state.write().await = vstu.0;
                        }
                        Some(GatewayEvent::VoiceStateUpdate(_)) => (),
                        None => return Err(Error::GatewayClosed),
                    }
                }
                // control commands
                command = self.command_rx.recv() => {
                    match command {
                        Some(Command::Play(source)) => {
                            // close source to make sure we can start a new one
                            self.close_source().await?;

                            // start new source
                            //self.streamer.add_silence(5);
                            self.streamer.source(source);
                        }
                        Some(Command::Pause) => {
                            //self.set_playing(false).await?;
                        }
                        Some(Command::Resume) => {
                            //if self.streamer.has_source() {
                            //    self.set_playing(true).await?;
                            //}
                        }
                        Some(Command::Stop) => {
                            self.close_source().await?;
                        }
                        None => return Err(Error::GatewayClosed),
                    }
                }
                // streaming audio
                result = self.streamer.stream(&mut self.rtp) => {
                    // send speaking events
                    match result? {
                        Status::Started(ssrc) => {
                            self.ws.send(Speaking {
                                speaking: 1,
                                ssrc,
                                delay: Some(0),
                            })
                            .await?;

                            self.set_playing(true).await;
                        }
                        Status::Stopped(ssrc) => {
                            self.ws.send(Speaking {
                                speaking: 0,
                                ssrc,
                                delay: Some(0),
                            })
                            .await?;

                            if !self.streamer.has_source() {
                                self.set_playing(false).await;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn close_source(&mut self) -> Result<(), Error> {
        //self.set_playing(false).await?;

        // close source prematurely
        if let Some(mut source) = self.streamer.take_source() {
            source.close().await?;
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn voice_server_update(&mut self, vseu: VoiceServerUpdate) -> Result<(), Error> {
        let session = Session {
            guild_id: self.state.guild_id,
            user_id: self.state.user_id,
            endpoint: vseu.endpoint.unwrap(),
            token: vseu.token,
            session_id: self.ws.session().session_id.clone(),
        };

        let deadline = Instant::now() + Duration::from_millis(5000);
        (self.ws, self.rtp) = match timeout_at(deadline, Connection::connect(session)).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(err)) => return Err(Error::from(err)),
            Err(_) => return Err(Error::Timeout),
        };

        if self.streamer.is_streaming() {
            self.ws.send(Speaking {
                speaking: 1,
                ssrc: self.rtp.ssrc(),
                delay: Some(0),
            })
            .await?;
        }

        Ok(())
    }
}

