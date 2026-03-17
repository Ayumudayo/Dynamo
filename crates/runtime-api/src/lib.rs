use dynamo_module_kit::{CommandCatalog, ModuleCatalog};
use dynamo_persistence_api::Persistence;
use dynamo_services_api::ServiceRegistry;
use poise::Context as PoiseContext;

pub type Error = anyhow::Error;
pub type Context<'a> = PoiseContext<'a, AppState, Error>;

#[derive(Clone)]
pub struct AppState {
    pub started_at: std::time::Instant,
    pub module_catalog: ModuleCatalog,
    pub command_catalog: CommandCatalog,
    pub persistence: Persistence,
    pub services: ServiceRegistry,
}

impl AppState {
    pub fn new(
        module_catalog: ModuleCatalog,
        command_catalog: CommandCatalog,
        persistence: Persistence,
        services: ServiceRegistry,
    ) -> Self {
        Self {
            started_at: std::time::Instant::now(),
            module_catalog,
            command_catalog,
            persistence,
            services,
        }
    }
}
