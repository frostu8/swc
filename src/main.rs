use futures_util::StreamExt;
use log::LevelFilter;
use std::{env, sync::Arc};

use swc::music::QueueServer;

use twilight_gateway::{Cluster, Intents, cluster::Events};
use twilight_http::client::Client;
use twilight_cache_inmemory::InMemoryCache;
use twilight_model::{
    id::Id,
    application::interaction::{
        application_command::{CommandData, CommandOptionValue},
        Interaction, InteractionData,
    },
    channel::message::Embed,
    gateway::event::Event, 
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    env_logger::Builder::new()
        .filter_level(LevelFilter::Debug)
        .parse_default_env()
        .init();

    // initialize discord cluster
    let (cluster, mut events) =
        Cluster::new(env::var("DISCORD_TOKEN")?, Intents::GUILDS | Intents::GUILD_VOICE_STATES).await?;
    let cluster = Arc::new(cluster);

    // spawn cluster thread
    let cluster_spawn = Arc::clone(&cluster);
    tokio::spawn(async move {
        cluster_spawn.up().await;
    });

    // create http client
    let http_client = Arc::new(Client::new(env::var("DISCORD_TOKEN")?));

    // create cache
    let cache = Arc::new(InMemoryCache::builder().message_cache_size(10).build());

    let queue_server = wait_for_ready(
        &mut events,
        &cluster,
        &cache,
        &http_client,
    ).await;

    while let Some((_, ev)) = events.next().await {
        cache.update(&ev);

        match ev {
            //Event::Ready(ready) => { }
            /*
            Event::InteractionCreate(mut interaction) => {
                match interaction.data.take() {
                    Some(InteractionData::ApplicationCommand(data)) => {
                        tokio::spawn(handle(
                            manager.clone().unwrap(),
                            http_client.clone(),
                            cache.clone(),
                            interaction.0,
                            data
                        ));
                    }
                    _ => ()
                }
            }
            */
            Event::VoiceStateUpdate(ev) => {
                queue_server.voice_state_update(ev).await;
            }
            Event::VoiceServerUpdate(ev) => {
                queue_server.voice_server_update(ev).await;
            }
            _ => (),
        }
    }

    Ok(())
}

async fn wait_for_ready(
    events: &mut Events,
    cluster: &Arc<Cluster>,
    cache: &Arc<InMemoryCache>,
    http_client: &Arc<Client>,
) -> Arc<QueueServer> {
    while let Some((_, ev)) = events.next().await {
        cache.update(&ev);

        if let Event::Ready(ready) = ev {
            let user_id = ready.user.id;

            log::info!("got ready, initializing on {}", user_id);

            // setup commands
            /*
            let _ = http_client
                .interaction(ready.application.id)
                .set_guild_commands(Id::new(971294860336316428), &swc::commands())
                .await;*/

            // initialize music queues
            let queue_server = Arc::new(QueueServer::new(
                cluster.clone(),
                cache.clone(),
                http_client.clone(),
                user_id,
            ));

            return queue_server;
        }
    }

    panic!("cluster closed!");
}

