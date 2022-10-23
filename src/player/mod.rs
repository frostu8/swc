//! Provides the audio [`Player`] and the player manager [`Manager`].

pub mod audio;
mod conn;
mod manager;
pub mod queue;

pub use manager::Manager;

use conn::{payload::Speaking, Connection, Event, RtpPacket, RtpSocket, Session};
use audio::Source as AudioSource;
use queue::Source;
use crate::constants::TIMESTEP_LENGTH;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::{sleep_until, timeout_at, Duration, Instant};

use twilight_model::{
    gateway::payload::incoming::{VoiceServerUpdate, VoiceStateUpdate},
    id::{
        marker::{GuildMarker, UserMarker},
        Id,
    },
};

/// A music player for a specific guild.
#[derive(Clone)]
pub struct Player {
    guild_id: Id<GuildMarker>,
    gateway_tx: UnboundedSender<GatewayEvent>,
    chsrc_tx: UnboundedSender<Source>,
}

impl Player {
    /// Creates a new player.
    pub async fn new(
        user_id: impl Into<Id<UserMarker>>,
        guild_id: impl Into<Id<GuildMarker>>,
    ) -> Player {
        let user_id = user_id.into();
        let guild_id = guild_id.into();

        let (gateway_tx, gateway_rx) = mpsc::unbounded_channel();
        let (chsrc_tx, chsrc_rx) = mpsc::unbounded_channel();

        // spawn player task
        tokio::spawn(run(guild_id, user_id, gateway_rx, chsrc_rx));

        // create Player
        Player {
            gateway_tx,
            guild_id,
            chsrc_tx,
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

    /// Updates the player's state with a [`VoiceStateUpdate`] event.
    pub fn voice_state_update(&self, ev: Box<VoiceStateUpdate>) -> Result<(), Error> {
        self.gateway_tx
            .send(GatewayEvent::VoiceStateUpdate(ev))
            .map_err(|_| Error)
    }

    /// Updates the player's state with a [`VoiceServerUpdate`] event.
    pub fn voice_server_update(&self, ev: VoiceServerUpdate) -> Result<(), Error> {
        self.gateway_tx
            .send(GatewayEvent::VoiceServerUpdate(ev))
            .map_err(|_| Error)
    }
}

/// An error for operations with [`Player`].
///
/// A [`Player`] may stop at any time in response to an unrecoverable error.
/// Things like being disconnected administratively by someone who has that
/// power is an unrecoverable error, and so is Discord servers going entirely
/// down.
#[derive(Clone, Copy, Debug)]
pub struct Error;

#[derive(Debug)]
enum GatewayEvent {
    VoiceStateUpdate(Box<VoiceStateUpdate>),
    VoiceServerUpdate(VoiceServerUpdate),
}

async fn run(
    guild_id: Id<GuildMarker>,
    user_id: Id<UserMarker>,
    mut gateway: UnboundedReceiver<GatewayEvent>,
    mut chsrc: UnboundedReceiver<Source>,
) {
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
            Ok(None) => {
                error!("gateway closed!");
                return;
            }
            Err(_) => {
                error!("player initialization timed out after 5000ms!");
                return;
            }
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
        error!("got no response for voice state update");
        return;
    };

    debug!("got new session; establishing ws connection...");

    let timeout = Instant::now() + Duration::from_millis(5000);
    let (mut conn, mut rtp) = match timeout_at(timeout, Connection::connect(session)).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(err)) => {
            error!("connect: {}", err);
            return;
        }
        Err(_) => {
            error!("player initialization timed out after 5000ms!");
            return;
        }
    };

    let source = AudioSource::new("https://youtu.be/IAkn-4qoK_Y")
        .await
        .unwrap();
    let mut stream = RtpStreamer::new(source);

    conn.send(Speaking {
        speaking: 1,
        ssrc: rtp.ssrc(),
        delay: Some(0),
    })
    .await
    .unwrap();

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
                    Some(Err(err)) => {
                        // print error and continue
                        error!("voice: {}", err);
                        break;
                    }
                    None => break
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
                            Ok(Ok(conn)) => conn,
                            Ok(Err(err)) => {
                                error!("connect: {}", err);
                                break;
                            }
                            Err(_) => {
                                error!("player reconnection timed out after 5000ms!");
                                break;
                            }
                        };
                    }
                    Some(GatewayEvent::VoiceStateUpdate(_vstu)) => {
                        // TODO: handle state update
                    }
                    None => {
                        error!("gateway closed unexpectedly");
                        break;
                    }
                }
            }
            // streaming audio
            result = stream.stream(&mut rtp) => {
                match result {
                    Ok(()) => {
                        warn!("audio ended");
                    }
                    Err(err) => {
                        error!("audio: {}", err);
                        break;
                    }
                }
            }
        }
    }

    // kill source gracefully
    let _ = stream.into_inner().kill().await;
}

/// Audio packet streamer.
///
/// Most of the time, we receive audio data faster than its playback speed. This
/// is actually really awesome! Well, until you realize that sending packets at
/// 4x the actual speed of the audio is going to cause some buffer issues and
/// also make you tonight's biggest loser.
struct RtpStreamer {
    source: AudioSource,

    packet: RtpPacket<[u8; crate::constants::VOICE_PACKET_MAX]>,
    next_packet: Instant,
    ready: bool,
}

impl RtpStreamer {
    /// Create a new `RtpStreamer`.
    pub fn new(source: AudioSource) -> RtpStreamer {
        RtpStreamer {
            source,
            packet: RtpPacket::default(),
            next_packet: Instant::now(),
            ready: false,
        }
    }

    /// Extracts the inner [`AudioSource`].
    pub fn into_inner(self) -> AudioSource {
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

async fn sleep_until_optional(deadline: Option<Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    } else {
        std::future::pending::<()>().await;
    }
}
