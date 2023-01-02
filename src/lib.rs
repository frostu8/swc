//! Soundwave command library.

#[macro_use]
extern crate log;

pub mod player;
pub mod interaction;

use twilight_model::application::command::{Command, CommandType, CommandOption, CommandOptionType};
use twilight_model::id::Id;

macro_rules! command {
    ($name:literal, $desc:literal, $($options:expr),* $(,)*) => {
        Command {
            application_id: None,
            default_member_permissions: None,
            dm_permission: None,
            description: String::from($desc),
            description_localizations: None,
            guild_id: None,
            id: None,
            kind: CommandType::ChatInput,
            name: String::from($name),
            name_localizations: None,
            options: vec![$($options),*],
            version: Id::new(1),
        }
    }
}

macro_rules! option {
    ($ty:expr, $name:literal, $desc:literal) => {
        option!($ty, $name, $desc, required: false)
    };
    ($ty:expr, $name:literal, $desc:literal, required: $required:literal) => {
        CommandOption {
            autocomplete: None,
            channel_types: None,
            choices: None,
            description: String::from($desc),
            description_localizations: None,
            kind: $ty,
            max_length: None,
            max_value: None,
            min_length: None,
            min_value: None,
            name: String::from($name),
            name_localizations: None,
            options: None,
            required: Some($required),
        }
    }
}

/// Creates a list of commands the bot supports.
pub fn commands() -> Vec<Command> {
    vec![
        command!(
            "play",
            "play a music track",
            option!(CommandOptionType::String, "url", "the url of the track", required: true),
        ),
        command!(
            "skip",
            "end the currently playing track and advance to the next",
        ),
    ]
}
