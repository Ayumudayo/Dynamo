use dynamo_core::ModuleRegistry;

pub fn module_registry() -> ModuleRegistry {
    ModuleRegistry::new(vec![Box::new(dynamo_module_info::InfoModule)])
}
