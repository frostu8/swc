//! Simple interaction helpers.

use twilight_model::application::interaction::application_command::{
    CommandDataOption, CommandOptionValue,
};

pub mod ext {
    pub use super::CommandOptionValueCastExt;
    pub use super::CommandOptionValueListCastExt;
}

/// Cast extension methods for interaction options.
///
/// We always know the schema of our commands, so this helper method
/// significantly cuts down the boilerplate.
pub trait CommandOptionValueCastExt: Sized {
    /// Casts the value to a type.
    ///
    /// Returns `Ok(T)` if the type is successfully casted, otherwise
    /// `Err(CastError)`.
    fn cast<'a, T>(&'a self) -> Result<T, CastError>
    where
        T: CommandOptionType<'a>;
}

impl CommandOptionValueCastExt for CommandDataOption {
    fn cast<'a, T>(&'a self) -> Result<T, CastError>
    where
        T: CommandOptionType<'a>
    {
        T::cast_from(&self.value)
    }
}

/// Cast extension methods for lists of interaction options.
///
/// See [`CommandOptionValueCastExt`].
pub trait CommandOptionValueListCastExt: Sized {
    /// Casts the value at index `idx` to a type.
    ///
    /// Returns `Ok(T)` if the type is successfully casted, otherwise
    /// `Err(CastError)` if the cast failed or the index is out of bounds.
    fn cast<'a, T>(&'a self, idx: usize) -> Result<T, CastError>
    where
        T: CommandOptionType<'a>;
}

impl CommandOptionValueListCastExt for Vec<CommandDataOption> {
    fn cast<'a, T>(&'a self, idx: usize) -> Result<T, CastError>
    where
        T: CommandOptionType<'a>
    {
        self.get(idx).ok_or(CastError).and_then(|s| s.cast())
    }
}

/// A type that a [`CommandOptionValue`] can be.
pub trait CommandOptionType<'a>: Sized {
    fn cast_from(value: &'a CommandOptionValue) -> Result<Self, CastError>;
}

impl<'a> CommandOptionType<'a> for &'a str {
    fn cast_from(value: &'a CommandOptionValue) -> Result<&'a str, CastError> {
        match value {
            CommandOptionValue::String(data) => Ok(data),
            _ => Err(CastError),
        }
    }
}

impl<'a> CommandOptionType<'a> for String {
    fn cast_from(value: &'a CommandOptionValue) -> Result<String, CastError> {
        match value {
            CommandOptionValue::String(data) => Ok(data.clone()),
            _ => Err(CastError),
        }
    }
}

impl<'a> CommandOptionType<'a> for bool {
    fn cast_from(value: &'a CommandOptionValue) -> Result<bool, CastError> {
        match value {
            CommandOptionValue::Boolean(data) => Ok(*data),
            _ => Err(CastError),
        }
    }
}

#[derive(Debug)]
pub struct CastError;

