use std::{env, sync::Arc};

use swc::music::{self, QueueServer};
use swc::interaction::ext::*;

use twilight_gateway::{Shard, ShardId, Intents, Config};
use twilight_http::client::Client;
use twilight_cache_inmemory::InMemoryCache;
use twilight_model::{
    id::Id,
    application::interaction::{
        application_command::CommandData,
        Interaction, InteractionData,
    },
    gateway::event::Event, 
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + 'static>> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt::init();

    // init ytdl executable
    swc::ytdl::init_ytdl_executable(|| {
        env::var("YTDL_EXECUTABLE")
            .unwrap_or_else(|_| String::from("youtube-dl"))
    });

    // initialize discord shard
    // we only need one shard, but our infrastructure can be scaled up
    // relatively easily.
    let shard_config = Config::builder(
        env::var("DISCORD_TOKEN")?,
        Intents::GUILDS | Intents::GUILD_VOICE_STATES,
    )
        /*
        .event_types(EventTypeFlags::READY
            | EventTypeFlags::INTERACTION_CREATE
            | EventTypeFlags::VOICE_STATE_UPDATE
            | EventTypeFlags::VOICE_SERVER_UPDATE)*/
        .build();
    let mut shard = Shard::with_config(ShardId::ONE, shard_config);

    // create http client
    let http_client = Arc::new(Client::new(env::var("DISCORD_TOKEN")?));

    // create cache
    let cache = Arc::new(InMemoryCache::builder().message_cache_size(10).build());

    let queue_server = wait_for_ready(
        &mut shard,
        &cache,
        &http_client,
    ).await?;

    loop {
        let ev = match shard.next_event().await {
            Ok(event) => event,
            Err(err) if err.is_fatal() => {
                tracing::error!(?err, "FATAL: {}", err);
                return Err(err.into());
            }
            Err(err) => {
                tracing::warn!(?err, "got disconnect, reconnecting");
                continue;
            }
        };

        cache.update(&ev);
        //log::debug!("{:?}", ev);

        match ev {
            //Event::Ready(ready) => { }
            Event::InteractionCreate(mut interaction) => {
                match interaction.data.take() {
                    Some(InteractionData::ApplicationCommand(data)) => {
                        handle_command(
                            &queue_server,
                            interaction.0,
                            data
                        ).await;
                    }
                    _ => ()
                }
            }
            Event::VoiceStateUpdate(ev) => {
                queue_server.voice_state_update(ev).await;
            }
            Event::VoiceServerUpdate(ev) => {
                queue_server.voice_server_update(ev).await;
            }
            _ => (),
        }
    }
}

/// Handles a command.
///
/// **This is run on the main thread! Do not block!**
async fn handle_command(
    queue_server: &Arc<QueueServer>,
    interaction: Interaction,
    data: Box<CommandData>,
) {
    let Some(guild_id) = data.guild_id else {
        return;
    };

    let Some(user) = interaction.member.as_ref().and_then(|m| m.user.as_ref()) else {
        return;
    };

    let command_data = music::CommandData {
        application_id: interaction.application_id,
        interaction_id: interaction.id,
        interaction_token: interaction.token,
        guild_id,
        user_id: user.id,
    };

    match &*data.name {
        "play" => {
            // first argument is the query
            let query = data
                .options
                .cast::<String>(0)
                .expect("invalid command schema");

            // send to the queue
            queue_server.command(
                guild_id,
                music::Command {
                    data: command_data,
                    action: music::Action::Play(query),
                },
            ).await;
        }
        "skip" => {
            // send to the queue
            queue_server.command(
                guild_id,
                music::Command {
                    data: command_data,
                    action: music::Action::Skip,
                },
            ).await;
        }
        "queue" => {
            // send to the queue
            queue_server.command(
                guild_id,
                music::Command {
                    data: command_data,
                    action: music::Action::Queue,
                },
            ).await;
        }
        "shuffle" => {
            // send to the queue
            queue_server.command(
                guild_id,
                music::Command {
                    data: command_data,
                    action: music::Action::Shuffle,
                },
            ).await;
        }
        // ignore missing commands
        name => {
            log::warn!("got missing or invalid command: /{}", name)
        }
    }
}

async fn wait_for_ready(
    shard: &mut Shard,
    cache: &Arc<InMemoryCache>,
    http_client: &Arc<Client>,
) -> Result<Arc<QueueServer>, Box<dyn std::error::Error + 'static>> {
    loop {
        let ev = match shard.next_event().await {
            Ok(event) => event,
            Err(err) => {
                log::error!("FATAL: {}", err);
                return Err(err.into());
            }
        };

        cache.update(&ev);

        if let Event::Ready(ready) = ev {
            let user_id = ready.user.id;

            log::info!("got ready, initializing on {}", user_id);

            // setup commands
            http_client
                .interaction(ready.application.id)
                .set_guild_commands(Id::new(952331087714070548), &swc::commands())
                //.set_global_commands(&swc::commands())
                .await
                .unwrap();

            // initialize music queues
            let queue_server = Arc::new(QueueServer::new(
                shard.sender(),
                cache.clone(),
                http_client.clone(),
                user_id,
            ));

            return Ok(queue_server);
        }
    }
}

