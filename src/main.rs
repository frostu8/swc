use futures_util::StreamExt;
use log::LevelFilter;
use std::{env, sync::Arc};
use twilight_gateway::{Cluster, Intents};
use twilight_http::client::Client;
use twilight_cache_inmemory::InMemoryCache;

use swc::player::{Manager, audio::Query};
use swc::interaction::ResponseExt;

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

    let mut manager: Option<Manager> = None;

    while let Some((_, ev)) = events.next().await {
        cache.update(&ev);

        match ev {
            Event::Ready(ready) => {
                let user_id = ready.user.id;

                // setup commands
                let _ = http_client
                    .interaction(ready.application.id)
                    .set_guild_commands(Id::new(971294860336316428), &swc::commands())
                    .await;

                // initialize manager
                manager = Some(Manager::new(user_id, http_client.clone(), Arc::clone(&cluster)));

                //player.push(Source::ytdl("https://youtu.be/vqzMdWcwSQs").await.unwrap()).unwrap();
            }
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
            Event::VoiceStateUpdate(ev) => {
                if let Some(manager) = &manager {
                    manager.voice_state_update(ev).await;
                }
            }
            Event::VoiceServerUpdate(ev) => {
                if let Some(manager) = &manager {
                    manager.voice_server_update(ev).await;
                }
            }
            _ => (),
        }
    }

    Ok(())
}

/// Handles execution of a command.
pub async fn handle(
    manager: Manager,
    http_client: Arc<Client>,
    cache: Arc<InMemoryCache>,
    interaction: Interaction,
    data: Box<CommandData>,
) {
    let guild_id = interaction.guild_id.expect("guild_id in slash command");
    let user_id = interaction.member.as_ref().expect("user in slash command").user.as_ref().unwrap().id;

    let channel_id = cache
        .voice_state(user_id, guild_id)
        .map(|s| s.channel_id());

    // get player
    let player = if let Some(channel_id) = channel_id {
        if let Some(player) = manager.get(guild_id).await {
            // check if we are in the same channel
            if channel_id == player.channel_id().await {
                player
            } else {
                let _ = http_client
                    .interaction(interaction.application_id)
                    .respond(interaction.id, &interaction.token)
                    .error("you must be in the same voice channel as the bot to use this!")
                    .await;

                return;
            }
        } else {
            // join channel
            manager.join(guild_id, channel_id).await
        }
    } else {
        let _ = http_client
            .interaction(interaction.application_id)
            .respond(interaction.id, &interaction.token)
            .error("you must be in a voice channel to use this!")
            .await;

        return;
    };

    match data.name.as_str() {
        "play" => {
            // first argument is always url
            let url = &data
                .options
                .get(0)
                .expect("invalid command schema")
                .value;

            let url = match url {
                CommandOptionValue::String(url) => url,
                _ => panic!("invalid command schema"),
            };

            // querying sometimes takes a while, so ack the response.
            let client = http_client.interaction(interaction.application_id);

            let _ = client.ack(interaction.id, &interaction.token).await;

            match Query::new(url).await {
                Ok(Query::Track(track)) => {
                    let _ = http_client
                        .interaction(interaction.application_id)
                        .respond(interaction.id, &interaction.token)
                        .embed(Embed {
                            description: Some("track added to queue".to_owned()),
                            ..track.to_embed()
                        })
                        .update()
                        .await;

                    player.enqueue(track).unwrap();
                }
                Ok(Query::Playlist(playlist)) => {
                    let _ = http_client
                        .interaction(interaction.application_id)
                        .respond(interaction.id, &interaction.token)
                        .embed(Embed {
                            description: Some("playlist added to queue".to_owned()),
                            ..playlist.to_embed()
                        })
                        .update()
                        .await;

                    player.enqueue_all(playlist.into_entries()).unwrap();
                }
                Err(err) => {
                    let _ = http_client
                        .interaction(interaction.application_id)
                        .respond(interaction.id, &interaction.token)
                        .error(err)
                        .update()
                        .await;
                }
            }
        }
        "skip" => {
            let _ = http_client
                .interaction(interaction.application_id)
                .respond(interaction.id, &interaction.token)
                .content("<:thoomb:1059535720097796196> track skipped")
                .await;

            player.next().unwrap();
        }
        _ => log::warn!("unknown command /{}", data.name)
    }
}
