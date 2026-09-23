use async_trait::async_trait;
use dynamo_repositories::{DeploymentSettingsRepository, GuildSettingsRepository};
use dynamo_settings::{
    DeploymentCommandSettings, DeploymentModuleSettings, DeploymentSettings, GuildCommandSettings,
    GuildModuleSettings, GuildSettings,
};
use mongodb::{
    bson::{Bson, Document, doc, to_bson},
    options::ReturnDocument,
};

use crate::{
    Error, MongoPersistence,
    config::DEPLOYMENT_SETTINGS_ID,
    documents::{DeploymentSettingsDocument, GuildSettingsDocument},
    ids::guild_document_id,
};

fn settings_set_on_insert(document_id: &str, excluded_parent_path: Option<&str>) -> Document {
    let mut set_on_insert = doc! { "_id": document_id };

    if excluded_parent_path != Some("modules") {
        set_on_insert.insert("modules", Document::new());
    }
    if excluded_parent_path != Some("commands") {
        set_on_insert.insert("commands", Document::new());
    }

    set_on_insert
}

pub(crate) fn settings_field_path(section: &str, id_kind: &str, id: &str) -> Result<String, Error> {
    if id.is_empty() {
        return Err(anyhow::anyhow!(
            "{id_kind} id cannot be empty for Mongo settings paths"
        ));
    }
    if id.contains('.') {
        return Err(anyhow::anyhow!(
            "{id_kind} id `{id}` cannot contain `.` for Mongo settings paths"
        ));
    }
    if id.starts_with('$') {
        return Err(anyhow::anyhow!(
            "{id_kind} id `{id}` cannot start with `$` for Mongo settings paths"
        ));
    }

    Ok(format!("{section}.{id}"))
}

pub(crate) fn settings_upsert_update(
    document_id: &str,
    settings_path: &str,
    settings: Bson,
) -> Document {
    let excluded_parent_path = settings_path.split_once('.').map(|(parent, _)| parent);
    doc! {
        "$setOnInsert": settings_set_on_insert(document_id, excluded_parent_path),
        "$set": { settings_path: settings },
    }
}

#[async_trait]
impl GuildSettingsRepository for MongoPersistence {
    async fn get(&self, guild_id: u64) -> Result<Option<GuildSettings>, Error> {
        let id = guild_document_id(guild_id);
        let document = self.guild_settings.find_one(doc! { "_id": &id }).await?;
        document.map(GuildSettingsDocument::into_domain).transpose()
    }

    async fn upsert_module_settings(
        &self,
        guild_id: u64,
        module_id: &str,
        settings: GuildModuleSettings,
    ) -> Result<GuildSettings, Error> {
        let id = guild_document_id(guild_id);
        let module_path = settings_field_path("modules", "module", module_id)?;
        let document = self
            .guild_settings
            .find_one_and_update(
                doc! { "_id": &id },
                settings_upsert_update(&id, &module_path, to_bson(&settings)?),
            )
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await?
            .ok_or_else(|| anyhow::anyhow!("guild module settings upsert returned no document"))?;
        document.into_domain()
    }

    async fn upsert_command_settings(
        &self,
        guild_id: u64,
        command_id: &str,
        settings: GuildCommandSettings,
    ) -> Result<GuildSettings, Error> {
        let id = guild_document_id(guild_id);
        let command_path = settings_field_path("commands", "command", command_id)?;
        let document = self
            .guild_settings
            .find_one_and_update(
                doc! { "_id": &id },
                settings_upsert_update(&id, &command_path, to_bson(&settings)?),
            )
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await?
            .ok_or_else(|| anyhow::anyhow!("guild command settings upsert returned no document"))?;
        document.into_domain()
    }
}

#[async_trait]
impl DeploymentSettingsRepository for MongoPersistence {
    async fn get(&self) -> Result<DeploymentSettings, Error> {
        let document = self
            .deployment_settings
            .find_one(doc! { "_id": DEPLOYMENT_SETTINGS_ID })
            .await?;
        Ok(document
            .unwrap_or_else(DeploymentSettingsDocument::default_document)
            .into_domain())
    }

    async fn upsert_module_settings(
        &self,
        module_id: &str,
        settings: DeploymentModuleSettings,
    ) -> Result<DeploymentSettings, Error> {
        let module_path = settings_field_path("modules", "module", module_id)?;
        let document = self
            .deployment_settings
            .find_one_and_update(
                doc! { "_id": DEPLOYMENT_SETTINGS_ID },
                settings_upsert_update(DEPLOYMENT_SETTINGS_ID, &module_path, to_bson(&settings)?),
            )
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("deployment module settings upsert returned no document")
            })?;
        Ok(document.into_domain())
    }

    async fn upsert_command_settings(
        &self,
        command_id: &str,
        settings: DeploymentCommandSettings,
    ) -> Result<DeploymentSettings, Error> {
        let command_path = settings_field_path("commands", "command", command_id)?;
        let document = self
            .deployment_settings
            .find_one_and_update(
                doc! { "_id": DEPLOYMENT_SETTINGS_ID },
                settings_upsert_update(DEPLOYMENT_SETTINGS_ID, &command_path, to_bson(&settings)?),
            )
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("deployment command settings upsert returned no document")
            })?;
        Ok(document.into_domain())
    }
}
