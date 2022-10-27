//! Provides the audio [`Player`] and the player manager [`Manager`].

pub mod audio;
pub mod constants;
pub mod commands;
mod conn;
mod manager;

pub use manager::Manager;

use conn::{payload::Speaking, Connection, Event, RtpPacket, RtpSocket, Session};
use audio::{Source, Track, Queue};
use commands::{Command, CommandType};
use constants::{TIMESTEP_LENGTH, VOICE_PACKET_MAX};

use anyhow::{bail, Context as _, Error};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::{sleep_until, timeout_at, Duration, Instant};

use std::sync::Arc;

use twilight_model::{
    gateway::payload::incoming::{VoiceServerUpdate, VoiceStateUpdate},
    channel::embed::Embed,
    http::interaction::{InteractionResponse, InteractionResponseType, InteractionResponseData},
    id::{
        marker::{GuildMarker, UserMarker, ApplicationMarker},
        Id,
    },
};
use twilight_http::Client;

/// A music player for a specific guild.
#[derive(Clone)]
pub struct Player {
    guild_id: Id<GuildMarker>,

    gateway_tx: UnboundedSender<GatewayEvent>,
    commands_tx: UnboundedSender<Command>,
}

impl Player {
    /// Creates a new player.
    pub async fn new(
        http_client: Arc<Client>,
        user_id: impl Into<Id<UserMarker>>,
        application_id: impl Into<Id<ApplicationMarker>>,
        guild_id: impl Into<Id<GuildMarker>>,
    ) -> Player {
        let user_id = user_id.into();
        let application_id = application_id.into();
        let guild_id = guild_id.into();

        let (gateway_tx, gateway_rx) = mpsc::unbounded_channel();
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();

        // spawn player task
        tokio::spawn(run(http_client, user_id, application_id, guild_id, gateway_rx, commands_rx));

        // create Player
        Player {
            guild_id,
            gateway_tx,
            commands_tx,
        }
    }

    /// The guild id of the player.
    pub fn guild_id(&self) -> Id<GuildMarker> {
        self.guild_id
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

#[derive(Debug)]
enum GatewayEvent {
    VoiceStateUpdate(Box<VoiceStateUpdate>),
    VoiceServerUpdate(VoiceServerUpdate),
}

async fn run(
    http_client: Arc<Client>,
    user_id: Id<UserMarker>,
    application_id: Id<ApplicationMarker>,
    guild_id: Id<GuildMarker>,
    gateway: UnboundedReceiver<GatewayEvent>,
    commands: UnboundedReceiver<Command>,
) {
    match run_inner(http_client, user_id, application_id, guild_id, gateway, commands).await {
        Ok(()) => (),
        Err(err) => error!("{:?}", err),
    }
}

async fn run_inner(
    http_client: Arc<Client>,
    user_id: Id<UserMarker>,
    application_id: Id<ApplicationMarker>,
    guild_id: Id<GuildMarker>,
    mut gateway: UnboundedReceiver<GatewayEvent>,
    mut commands: UnboundedReceiver<Command>,
) -> Result<(), anyhow::Error> {
    // begin initialization: wait for events
    let timeout = Instant::now() + Duration::from_millis(5000);
    let mut vstu: Option<Box<VoiceStateUpdate>> = None;
    let mut vseu: Option<VoiceServerUpdate> = None;

    debug!("waiting for VoiceStateUpdate and VoiceServerUpdate...");

    loop {
        match timeout_at(timeout, gateway.recv()).await {
            Ok(Some(ev)) => {
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
            Ok(None) => bail!("gateway closed unexpectedly"),
            Err(_) => bail!("player initialization timed out after 5000ms!"),
        }
    }

    // establish session
    let session = if let (Some(vseu), Some(vstu)) = (vseu, vstu) {
        Session {
            guild_id,
            user_id,
            endpoint: vseu.endpoint.unwrap(),
            token: vseu.token,
            session_id: vstu.0.session_id,
        }
    } else {
        bail!("got no response for voice state update");
    };

    debug!("got new session; establishing ws connection...");

    let timeout = Instant::now() + Duration::from_millis(5000);
    let (mut conn, mut rtp) = match timeout_at(timeout, Connection::connect(session)).await {
        Ok(conn) => conn.context("failed to connect to voice websocket")?,
        Err(_) => bail!("player initialization timed out after 5000ms!"),
    };

    let mut queue = Queue::new(5);
    let mut stream: Option<RtpStreamer> = None;

    conn.send(Speaking {
        speaking: 1,
        ssrc: rtp.ssrc(),
        delay: Some(0),
    })
    .await
    .unwrap();

    let interaction_client = http_client.interaction(application_id);

    loop {
        tokio::select! {
            biased;

            // voice websocket event
            ev = conn.recv() => {
                match ev {
                    Some(Ok(Event::ChangeSocket(new_rtp))) => {
                        // update socket
                        rtp = new_rtp;
                    }
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
            ev = gateway.recv() => {
                match ev {
                    Some(GatewayEvent::VoiceServerUpdate(vseu)) => {
                        // server update; reconnect
                        let session = Session {
                            guild_id, user_id,
                            endpoint: vseu.endpoint.unwrap(),
                            token: vseu.token,
                            session_id: conn.session().session_id.clone(),
                        };
                        let timeout = Instant::now() + Duration::from_millis(5000);
                        (conn, rtp) = match timeout_at(timeout, Connection::connect(session)).await {
                            Ok(conn) => conn.context("failed to connect to voice websocket")?,
                            Err(_) => bail!("player reconnection timed out after 5000ms!"),
                        };
                    }
                    Some(GatewayEvent::VoiceStateUpdate(_vstu)) => {
                        // TODO: handle state update
                    }
                    None => bail!("gateway closed unexpectedly"),
                }
            }
            // commands
            Some(command) = commands.recv() => {
                match command.kind {
                    CommandType::Play(track) => {
                        let embed = Embed {
                            description: Some("added song to queue".into()),
                            ..track.to_embed()
                        };

                        // add track to queue
                        queue.push(track);

                        let fut = interaction_client
                            .create_response(
                                command.interaction.id,
                                &command.interaction.token,
                                &InteractionResponse {
                                    kind: InteractionResponseType::ChannelMessageWithSource,
                                    data: Some(InteractionResponseData {
                                        embeds: Some(vec![embed]),
                                        ..Default::default()
                                    }),
                                },
                            )
                            .exec();

                        tokio::spawn(fut);
                    }
                    CommandType::Skip => {
                        // try to pull source off of queue
                        change_source(&mut stream, queue.next()).await
                            .context("failed to start audio stream")?;

                        let fut = interaction_client
                            .create_response(
                                command.interaction.id,
                                &command.interaction.token,
                                &InteractionResponse {
                                    kind: InteractionResponseType::ChannelMessageWithSource,
                                    data: Some(InteractionResponseData {
                                        // TODO: embed magic
                                        content: Some("skipped song".into()),
                                        ..Default::default()
                                    }),
                                },
                            )
                            .exec();

                        tokio::spawn(fut);
                    }
                }

                if stream.is_none() {
                    // try to pull source off of queue
                    change_source(&mut stream, queue.next()).await
                        .context("failed to start audio stream")?;
                }
            }
            // streaming audio
            result = stream_optional(&mut stream, &mut rtp) => {
                result.context("failed to stream audio")?;

                // advance queue
                change_source(&mut stream, queue.next()).await
                    .context("failed to start audio stream")?;
            }
        }
    }

    // kill source gracefully
    let _ = kill_optional(&mut stream).await;

    Ok(())
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

/// Changes the source of the [`RtpStreamer`].
///
/// A `source` of `None` unsets the source.
async fn change_source(
    stream: &mut Option<RtpStreamer>,
    track: Option<&Track>,
) -> Result<(), anyhow::Error> {
    // kill
    let _ = kill_optional(stream).await;

    // TODO: generate silence
    // we might add a RtpStreamer::end() function to signal the end
    // of the source and then wait for that to be over to push the
    // next audio source on

    // update source
    match track {
        Some(track) => {
            let source = track.open().await?;

            *stream = Some(RtpStreamer::new(source));

            Ok(())
        }
        None => Ok(()),
    }
}

/// Streams the inner audio over the [`RtpSocket`].
///
/// This is intended to be cancelled.
async fn stream_optional(
    stream: &mut Option<RtpStreamer>,
    rtp: &mut RtpSocket,
) -> Result<(), anyhow::Error> {
    match stream {
        Some(stream) => stream.stream(rtp).await,
        None => std::future::pending().await,
    }
}

/// Kills the [`RtpStreamer`] if it is `Some`.
async fn kill_optional(
    stream: &mut Option<RtpStreamer>,
) -> Result<(), audio::Error> {
    match stream.take() {
        Some(stream) => stream.into_inner().kill().await,
        None => std::future::ready(Ok(())).await,
    }
}
