//! Provides the audio [`Player`].

mod ws;

use tokio::sync::{
    mpsc::{self, UnboundedSender, UnboundedReceiver},
};
use tokio::time::{sleep_until, Instant, Duration};
use twilight_model::{
    id::{Id, marker::{GuildMarker, UserMarker}},
    gateway::payload::incoming::{VoiceStateUpdate, VoiceServerUpdate},
};
use ws::{payload::{Event, Hello, Heartbeat, Ready, Identify}, WebSocket};

/// A music player for a specific guild.
pub struct Player {
    guild_id: Id<GuildMarker>,
    user_id: Id<UserMarker>,
    gateway_tx: UnboundedSender<GatewayEvent>,
}

impl Player {
    pub async fn new(
        user_id: impl Into<Id<UserMarker>>,
        guild_id: impl Into<Id<GuildMarker>>,
    ) -> Player {
        let user_id = user_id.into();
        let guild_id = guild_id.into();

        let (gateway_tx, gateway_rx) = mpsc::unbounded_channel();

        // spawn player task
        tokio::spawn(run(guild_id, user_id, gateway_rx));

        // create Player
        Player {
            gateway_tx,
            guild_id,
            user_id,
        }
    }

    pub fn voice_state_update(&self, ev: Box<VoiceStateUpdate>) {
        self.gateway_tx.send(GatewayEvent::VoiceStateUpdate(ev)).unwrap();
    }

    pub fn voice_server_update(&self, ev: VoiceServerUpdate) {
        self.gateway_tx.send(GatewayEvent::VoiceServerUpdate(ev)).unwrap();
    }
}

#[derive(Debug)]
enum GatewayEvent {
    VoiceStateUpdate(Box<VoiceStateUpdate>),
    VoiceServerUpdate(VoiceServerUpdate),
}

struct WsState {
    endpoint: String,
    session_id: String,
    token: String,
}

// manages heartbeat state
struct Heartbeater {
    interval: f32,
    next_nonce: u64,
    next: Instant,
}

impl Heartbeater {
    /// Creates a new heartbeater.
    pub fn new(interval: f32) -> Heartbeater {
        Heartbeater {
            interval,
            next_nonce: 0,
            next: Instant::now() + Duration::from_millis(interval as u64),
        }
    }

    /// Returns the next heartbeat after the alloted time has passed.
    pub async fn next(&mut self) -> Heartbeat {
        sleep_until(self.next).await;

        let heartbeat = Heartbeat(self.next_nonce);
        self.next_nonce += 1;
        self.next = Instant::now() + Duration::from_millis(self.interval as u64);

        heartbeat
    }
}

async fn run(
    guild_id: Id<GuildMarker>,
    user_id: Id<UserMarker>,
    mut gateway: UnboundedReceiver<GatewayEvent>,
) {
    // begin initialization: wait for events
    let timeout = Instant::now() + Duration::from_millis(5000);
    let mut vstu: Option<Box<VoiceStateUpdate>> = None;
    let mut vseu: Option<VoiceServerUpdate> = None;

    debug!("waiting for VoiceStateUpdate and VoiceServerUpdate...");

    loop {
        tokio::select! {
            _ = sleep_until(timeout) => {
                error!("player initialization timed out after 5000ms!");
                return;
            }
            Some(ev) = gateway.recv() => {
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
        }
    }

    // establish ws state
    let mut ws_state = {
        let vstu = vstu.unwrap();
        let vseu = vseu.unwrap();

        WsState {
            endpoint: vseu.endpoint.unwrap(),
            token: vseu.token,
            session_id: vstu.0.session_id,
        }
    };

    debug!("got new session; establishing ws connection...");

    let mut ws = WebSocket::new(&ws_state.endpoint).await.unwrap();

    debug!("sending identify payload");

    ws.send(&Event::Identify(Identify {
        guild_id,
        user_id,
        session_id: ws_state.session_id,
        token: ws_state.token,
    }))
    .await
    .unwrap();

    debug!("waiting for hello and ready");

    // wait for hello and ready events
    let mut hello: Option<Hello> = None;
    let mut ready: Option<Ready> = None;

    loop {
        tokio::select! {
            _ = sleep_until(timeout) => {
                error!("player initialization timed out after 5000ms!");
                return;
            }
            ev = ws.recv() => {
                match ev {
                    Ok(Some(ev)) => {
                        match ev {
                            Event::Hello(ev) => {
                                hello = Some(ev);
                            }
                            Event::Ready(ev) => {
                                ready = Some(ev);
                            }
                            ev => {
                                warn!("unexpected event: {:?}", ev);
                            }
                        }

                        if hello.is_some() && ready.is_some() {
                            break;
                        }
                    }
                    Ok(None) => {
                        error!("ws closed unexpectedly");
                        return;
                    }
                    Err(err) => {
                        error!("ws error: {}", err);
                        return;
                    }
                }
            }
        }
    }

    // create heartbeater
    let mut heartbeater = Heartbeater::new(hello.unwrap().heartbeat_interval);

    // begin event loop
    loop {
        tokio::select! {
            // next event
            ev = ws.recv() => {
                match ev {
                    Ok(Some(ev)) => match ev {
                        Event::HeartbeatAck(_) => {
                            debug!("VOICE_HEARTBEAT_ACK");
                        }
                        _ => ()
                    }
                    Ok(None) => {
                        break;
                    }
                    Err(ws::error::Error::Api(error)) if error.disconnected() => {
                        debug!("forcibly disconnected from voice");
                        break;
                    }
                    Err(err) => {
                        error!("ws error: {}", err);
                        break;
                    }
                }
            }
            // wait for heartbeats
            heartbeat = heartbeater.next() => {
                // send heartbeat
                ws.send(&Event::Heartbeat(heartbeat)).await.unwrap();
            }
        }
    }
}

