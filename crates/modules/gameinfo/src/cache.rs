use crate::{maintenance::MaintInfo, pll::PllInfo};
use dynamo_runtime_api::Error;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::fs;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct GameInfoCacheFile {
    #[serde(rename = "MAINTINFO", default)]
    maintinfo: serde_json::Value,
    #[serde(rename = "PLLINFO", default)]
    pllinfo: serde_json::Value,
}

pub(crate) struct GameInfoCacheStore {
    data_path: PathBuf,
    sample_path: PathBuf,
}

impl GameInfoCacheStore {
    pub(crate) fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../");
        Self {
            data_path: root.join("logs/gameinfo-cache.json"),
            sample_path: root.join("logs/gameinfo-cache.sample.json"),
        }
    }

    pub(crate) async fn load_maintinfo(&self) -> Result<Option<MaintInfo>, Error> {
        let cache = self.load().await?;
        Ok(parse_optional(cache.maintinfo))
    }

    pub(crate) async fn save_maintinfo(&self, info: &MaintInfo) -> Result<(), Error> {
        self.update(|cache| {
            cache.maintinfo = serde_json::to_value(info).expect("serializable maint info");
        })
        .await
    }

    pub(crate) async fn load_pllinfo(&self) -> Result<Option<PllInfo>, Error> {
        let cache = self.load().await?;
        Ok(parse_optional(cache.pllinfo))
    }

    pub(crate) async fn save_pllinfo(&self, info: &PllInfo) -> Result<(), Error> {
        self.update(|cache| {
            let mut value = serde_json::to_value(info).expect("serializable pll info");
            normalize_pll_value(&mut value);
            cache.pllinfo = value;
        })
        .await
    }

    async fn load(&self) -> Result<GameInfoCacheFile, Error> {
        self.ensure_file().await?;
        let text = fs::read_to_string(&self.data_path).await?;
        let value = serde_json::from_str::<GameInfoCacheFile>(&text).unwrap_or_default();
        Ok(value)
    }

    async fn update(&self, mutator: impl FnOnce(&mut GameInfoCacheFile)) -> Result<(), Error> {
        self.ensure_file().await?;
        let mut cache = self.load().await?;
        mutator(&mut cache);
        let payload = serde_json::to_string_pretty(&cache)?;
        let tmp = self.data_path.with_extension("json.tmp");
        fs::write(&tmp, format!("{payload}\n")).await?;
        fs::rename(tmp, &self.data_path).await?;
        Ok(())
    }

    async fn ensure_file(&self) -> Result<(), Error> {
        if fs::try_exists(&self.data_path).await? {
            return Ok(());
        }

        if let Some(parent) = self.data_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        if fs::try_exists(&self.sample_path).await? {
            let content = fs::read(&self.sample_path).await?;
            fs::write(&self.data_path, content).await?;
        } else {
            fs::write(
                &self.data_path,
                "{\n  \"MAINTINFO\": {},\n  \"PLLINFO\": {}\n}\n",
            )
            .await?;
        }

        Ok(())
    }
}

fn parse_optional<T>(value: serde_json::Value) -> Option<T>
where
    T: for<'de> Deserialize<'de>,
{
    if value.is_null() {
        return None;
    }

    match value {
        serde_json::Value::Object(ref map) if map.is_empty() => None,
        other => serde_json::from_value(other).ok(),
    }
}

fn normalize_pll_value(value: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = value {
        if let Some(expire_time) = map.remove("expire_time") {
            map.insert("expireTime".to_string(), expire_time);
        }
        if let Some(fixed_title) = map.remove("fixed_title") {
            map.insert("fixedTitle".to_string(), fixed_title);
        }
    }
}
