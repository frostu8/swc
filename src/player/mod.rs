//! Provides the audio [`Player`] and the player manager [`Manager`].

pub mod audio;
pub mod constants;
pub mod commands;
mod conn;
mod manager;

pub use manager::Manager;

use conn::{payload::Speaking, Connection, RtpPacket, RtpSocket, Session};
use audio::{Source, Queue};
use commands::{Command, CommandType};
use constants::{TIMESTEP_LENGTH, VOICE_PACKET_MAX};

use anyhow::{bail, Context as _, Error};

use tokio::sync::{RwLock, mpsc::{self, UnboundedReceiver, UnboundedSender}};
use tokio::time::{sleep_until, timeout_at, Duration, Instant};

use std::sync::Arc;

use twilight_model::{
    gateway::payload::incoming::{VoiceServerUpdate, VoiceStateUpdate},
    channel::embed::Embed,
    //http::interaction::{InteractionResponse, InteractionResponseType, InteractionResponseData},
    voice::VoiceState,
    id::{
        marker::{GuildMarker, ChannelMarker, UserMarker},
        Id,
    },
};
use twilight_http::Client;

/// A music player for a specific guild.
#[derive(Clone)]
pub struct Player {
    guild_id: Id<GuildMarker>,
    state: Arc<RwLock<VoiceState>>,

    gateway_tx: UnboundedSender<GatewayEvent>,
    commands_tx: UnboundedSender<Command>,
}

impl Player {
    /// Creates a new player.
    pub async fn new(
        http_client: Arc<Client>,
        user_id: impl Into<Id<UserMarker>>,
        guild_id: impl Into<Id<GuildMarker>>,
        state: VoiceState,
    ) -> Player {
        let user_id = user_id.into();
        let guild_id = guild_id.into();

        let (gateway_tx, gateway_rx) = mpsc::unbounded_channel();
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();

        let state = Arc::new(RwLock::new(state));
        let state_clone = state.clone();

        // spawn player task
        tokio::spawn(async move {
            let fut = PlayerState::new(
                http_client,
                user_id,
                guild_id,
                state_clone,
                gateway_rx,
                commands_rx,
            );

            match tokio::time::timeout(Duration::from_millis(5000), fut).await {
                Ok(Ok(player)) => match player.run().await {
                    Ok(()) => (),
                    Err(err) => error!("{:?}", err),
                },
                Ok(Err(err)) => error!("{:?}", err),
                Err(_) => error!("player initialization timed out after 5000ms!"),
            }
        });

        // create Player
        Player {
            guild_id,
            state,
            gateway_tx,
            commands_tx,
        }
    }

    /// The guild id of the player.
    pub fn guild_id(&self) -> Id<GuildMarker> {
        self.guild_id
    }

    /// The channel the player is in.
    pub async fn channel_id(&self) -> Id<ChannelMarker> {
        self.state.read().await.channel_id.unwrap()
    }

    /// Check if the player is closed.
    pub fn is_closed(&self) -> bool {
        self.gateway_tx.is_closed()
    }

    /// Sends a command to the player.
    ///
    /// # Panics
    /// Panics if an error was returned last call and the `Player` is used
    /// again.
    pub fn command(&self, command: Command) -> Result<(), Error> {
        self.commands_tx
            .send(command)
            .map_err(|_| anyhow::anyhow!("player closed"))
    }

    /// Updates the player's state with a [`VoiceStateUpdate`] event.
    ///
    /// # Panics
    /// Panics if an error was returned last call and the `Player` is used
    /// again.
    pub fn voice_state_update(&self, ev: Box<VoiceStateUpdate>) -> Result<(), Error> {
        self.gateway_tx
            .send(GatewayEvent::VoiceStateUpdate(ev))
            .map_err(|_| anyhow::anyhow!("player closed"))
    }

    /// Updates the player's state with a [`VoiceServerUpdate`] event.
    ///
    /// # Panics
    /// Panics if an error was returned last call and the `Player` is used
    /// again.
    pub fn voice_server_update(&self, ev: VoiceServerUpdate) -> Result<(), Error> {
        self.gateway_tx
            .send(GatewayEvent::VoiceServerUpdate(ev))
            .map_err(|_| anyhow::anyhow!("player closed"))
    }
}

struct PlayerState {
    http_client: Arc<Client>,
    guild_id: Id<GuildMarker>,
    user_id: Id<UserMarker>,

    state: Arc<RwLock<VoiceState>>,
    gateway: UnboundedReceiver<GatewayEvent>,
    commands: UnboundedReceiver<Command>,

    ws: Connection,
    rtp: RtpSocket,

    queue: Queue,
    stream: Option<RtpStreamer>,
}

#[derive(Debug)]
enum GatewayEvent {
    VoiceStateUpdate(Box<VoiceStateUpdate>),
    VoiceServerUpdate(VoiceServerUpdate),
}

impl PlayerState {
    /// Creates a new `PlayerState`.
    pub async fn new(
        http_client: Arc<Client>,
        user_id: Id<UserMarker>,
        guild_id: Id<GuildMarker>,
        state: Arc<RwLock<VoiceState>>,
        mut gateway: UnboundedReceiver<GatewayEvent>,
        commands: UnboundedReceiver<Command>,
    ) -> Result<PlayerState, Error> {
        // begin initialization: wait for events
        let mut vstu: Option<Box<VoiceStateUpdate>> = None;
        let mut vseu: Option<VoiceServerUpdate> = None;

        debug!("waiting for VoiceStateUpdate and VoiceServerUpdate...");

        while let Some(ev) = gateway.recv().await {
            match ev {
                GatewayEvent::VoiceStateUpdate(ev) if ev.0.user_id == user_id => {
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
                guild_id,
                user_id,
                endpoint: vseu.endpoint.unwrap(),
                token: vseu.token,
                session_id: vstu.0.session_id.clone(),
            };

            // update state
            *state.write().await = vstu.0;

            res
        } else {
            bail!("got no response for voice state update");
        };

        debug!("got new session; establishing ws connection...");

        let (mut ws, rtp) = Connection::connect(session)
            .await
            .context("failed to connect to voice websocket")?;

        ws.send(Speaking {
            speaking: 1,
            ssrc: rtp.ssrc(),
            delay: Some(0),
        })
        .await
        .unwrap();

        Ok(PlayerState {
            http_client,
            user_id,
            guild_id,

            state,
            gateway,
            commands,

            ws,
            rtp,

            queue: Queue::new(5),
            stream: None,
        })
    }

    /// Begins handling requests and audio on this thread.
    ///
    /// This should be called as a parameter to [`tokio::spawn`].
    pub async fn run(mut self) -> Result<(), Error> {
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
                            debug!("disconnected");
                            break;
                        }
                        Some(Err(err)) => return Err(err).context("voice websocket closed"),
                        None => break,
                    }
                }
                // main gateway event
                ev = self.gateway.recv() => {
                    match ev {
                        Some(GatewayEvent::VoiceServerUpdate(vseu)) => {
                            // server update; reconnect
                            self.voice_server_update(vseu).await?;
                        }
                        Some(GatewayEvent::VoiceStateUpdate(vstu)) => {
                            *self.state.write().await = vstu.0;
                        }
                        None => bail!("gateway closed unexpectedly"),
                    }
                }
                // commands
                Some(command) = self.commands.recv() => {
                    self.command(command).await?;
                }
                // streaming audio
                result = stream_to_rtp(&mut self.stream, &mut self.rtp) => {
                    result.context("failed to stream audio")?;

                    // advance queue
                    self.next()
                        .await
                        .context("failed to start audio stream")?;
                }
            }
        }

        // kill source gracefully
        let _ = self.kill_stream().await;

        Ok(())
    }

    async fn command(&mut self, command: Command) -> Result<(), Error> {
        match &command.kind {
            CommandType::Play(track) => {
                let embed = Embed {
                    description: Some("added song to queue".into()),
                    ..track.to_embed()
                };

                // add track to queue
                self.queue.push(track.clone());

                let http_client = self.http_client.clone();
                tokio::spawn(async move {
                    command
                        .respond()
                        .embed(embed)
                        .send(http_client)
                        .await
                });

                if self.stream.is_none() {
                    // try to pull source off of queue
                    self.next()
                        .await
                        .context("failed to start audio stream")?;
                }
            }
            CommandType::Skip => {
                // try to pull source off of queue
                self.next()
                    .await
                    .context("failed to start audio stream")?;

                let http_client = self.http_client.clone();
                tokio::spawn(async move {
                    command
                        .respond()
                        .content("skipped song")
                        .send(http_client)
                        .await
                });
            }
        }

        Ok(())
    }

    async fn next(&mut self) -> Result<(), Error> {
        // kill
        self.kill_stream().await?;

        // TODO: generate silence
        // we might add a RtpStreamer::end() function to signal the end
        // of the source and then wait for that to be over to push the
        // next audio source on

        // update source
        match self.queue.next() {
            Some(track) => {
                let source = track.open().await?;

                self.stream = Some(RtpStreamer::new(source));

                Ok(())
            }
            None => Ok(()),
        }
    }

    async fn kill_stream(&mut self) -> Result<(), Error> {
        match self.stream.take() {
            Some(stream) => stream.into_inner().kill().await.map_err(Into::into),
            None => std::future::ready(Ok(())).await,
        }
    }

    async fn voice_server_update(&mut self, vseu: VoiceServerUpdate) -> Result<(), Error> {
        let session = Session {
            guild_id: self.guild_id,
            user_id: self.user_id,
            endpoint: vseu.endpoint.unwrap(),
            token: vseu.token,
            session_id: self.ws.session().session_id.clone(),
        };

        let timeout = Instant::now() + Duration::from_millis(5000);
        (self.ws, self.rtp) = match timeout_at(timeout, Connection::connect(session)).await {
            Ok(conn) => conn.context("failed to connect to voice websocket")?,
            Err(_) => bail!("player reconnection timed out after 5000ms!"),
        };

        Ok(())
    }
}

/// Audio packet streamer.
///
/// Most of the time, we receive audio data faster than its playback speed. This
/// is actually really awesome! Well, until you realize that sending packets at
/// 4x the actual speed of the audio is going to cause some buffer issues and
/// also make you tonight's biggest loser.
struct RtpStreamer {
    source: Source,

    packet: RtpPacket<[u8; VOICE_PACKET_MAX]>,
    next_packet: Instant,
    ready: bool,
}

impl RtpStreamer {
    /// Create a new `RtpStreamer`.
    pub fn new(source: Source) -> RtpStreamer {
        RtpStreamer {
            source,
            packet: RtpPacket::default(),
            next_packet: Instant::now(),
            ready: false,
        }
    }

    /// Extracts the inner [`Source`].
    pub fn into_inner(self) -> Source {
        self.source
    }

    /// Streams the inner audio over the [`RtpSocket`].
    ///
    /// Doesn't return until all of the audio has been streamed. Cancel-safe.
    pub async fn stream(
        &mut self,
        rtp: &mut RtpSocket,
    ) -> Result<(), anyhow::Error> {
        loop {
            tokio::select! {
                () = sleep_until(self.next_packet), if self.ready => {
                    // send packet
                    rtp.send(&mut self.packet).await?;

                    // setup for next packet
                    self.packet = RtpPacket::default();
                    self.next_packet = self.next_packet + TIMESTEP_LENGTH;
                    self.ready = false;
                }
                result = self.source.read(self.packet.payload_mut()), if !self.ready => {
                    let len = result?;

                    if len > 0 {
                        self.packet.set_payload_len(len);
                        self.ready = true;

                        let now = Instant::now();
                        if self.next_packet < now {
                            warn!("overloaded! {}ms", (now - self.next_packet).as_millis());
                            self.next_packet = Instant::now();
                        }
                    } else {
                        // we are at the end of the stream, exit
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Streams the inner audio over the [`RtpSocket`].
///
/// This is intended to be cancelled.
async fn stream_to_rtp(
    stream: &mut Option<RtpStreamer>,
    rtp: &mut RtpSocket,
) -> Result<(), anyhow::Error> {
    match stream {
        Some(stream) => stream.stream(rtp).await,
        None => std::future::pending().await,
    }
}
