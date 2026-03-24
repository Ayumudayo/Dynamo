mod module;

pub mod events {
    pub use super::module::{send_farewell, send_welcome};
}

pub use module::GreetingModule;
