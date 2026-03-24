mod module;

pub mod events {
    pub use super::module::{handle_interaction, handle_message, handle_voice_state_update};
}

pub use module::StatsModule;
