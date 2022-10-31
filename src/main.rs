use futures_util::StreamExt;
use log::LevelFilter;
use std::{env, sync::Arc};
use twilight_gateway::{Cluster, Intents};
use twilight_http::client::{Client, InteractionClient};
use twilight_cache_inmemory::InMemoryCache;

use swc::player::{Manager, audio::Query, commands::{Command, CommandType}};

use twilight_model::{
    application::interaction::{
        application_command::{CommandData, CommandOptionValue},
        Interaction, InteractionData,
    },
    channel::message::MessageFlags,
    http::interaction::{
        InteractionResponse, InteractionResponseType, InteractionResponseData,
    },
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

    // flag used to control if an interaction is updated because it would take
    // too long to handle.
    let mut acked = false;

    // get player
    let player = if let Some(channel_id) = channel_id {
        if let Some(player) = manager.get(guild_id).await {
            // check if we are in the same channel
            if channel_id == player.channel_id().await {
                player
            } else {
                let _ = http_client
                    .interaction(interaction.application_id)
                    .create_response(
                        interaction.id,
                        &interaction.token,
                        &InteractionResponse {
                            kind: InteractionResponseType::ChannelMessageWithSource,
                            data: Some(InteractionResponseData {
                                content: Some("you must be in the same voice channel as the bot to use this!".into()),
                                flags: Some(MessageFlags::EPHEMERAL),
                                ..Default::default()
                            }),
                        },
                    )
                    .exec()
                    .await;

                return;
            }
        } else {
            // players take a *long* time to initialize
            ack_interaction(
                &interaction,
                http_client.interaction(interaction.application_id),
            ).await;
            acked = true;

            // join channel
            manager.join(guild_id, channel_id).await
        }
    } else {
        let _ = http_client
            .interaction(interaction.application_id)
            .create_response(
                interaction.id,
                &interaction.token,
                &InteractionResponse {
                    kind: InteractionResponseType::ChannelMessageWithSource,
                    data: Some(InteractionResponseData {
                        content: Some("you must be in a voice channel to use this!".into()),
                        flags: Some(MessageFlags::EPHEMERAL),
                        ..Default::default()
                    }),
                },
            )
            .exec()
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

            // loading track information from ytdl takes a while
            if !acked {
                // players take a *long* time to initialize
                ack_interaction(
                    &interaction,
                    http_client.interaction(interaction.application_id),
                ).await;
                acked = true;
            }

            // TODO: gracefully handle error
            match Query::new(url).await.unwrap() {
                Query::Track(track) => {
                    player.command(Command::new(
                        interaction,
                        CommandType::Play(track),
                        acked,
                    ))
                        .unwrap();
                }
                Query::Playlist(playlist) => {
                    player.command(Command::new(
                        interaction,
                        CommandType::PlayList(playlist),
                        acked,
                    ))
                        .unwrap();
                }
            }
        }
        "skip" => {
            player.command(Command::new(
                interaction,
                CommandType::Skip,
                acked,
            ))
                .unwrap();
        }
        _ => log::warn!("unknown command /{}", data.name)
    }
}

fn ack_interaction(
    interaction: &Interaction,
    client: InteractionClient,
) -> impl std::future::Future {
    client
        .create_response(
            interaction.id,
            &interaction.token,
            &InteractionResponse {
                kind: InteractionResponseType::DeferredChannelMessageWithSource,
                data: None,
            },
        )
        .exec()
}
