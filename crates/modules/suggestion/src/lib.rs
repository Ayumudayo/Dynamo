mod module;

pub mod interactions {
    pub use super::module::handle_interaction as handle;
}

pub use module::SuggestionModule;
