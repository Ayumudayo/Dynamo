use crate::{maintenance::MaintInfo, pll::PllInfo};
use dynamo_runtime_api::Error;
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};
use tokio::fs;
use tokio::sync::Mutex;

static CACHE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn cache_lock() -> &'static Mutex<()> {
    CACHE_LOCK.get_or_init(|| Mutex::new(()))
}

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
        let _guard = cache_lock().lock().await;
        let cache = self.load_unlocked().await?;
        Ok(parse_optional(cache.maintinfo))
    }

    pub(crate) async fn save_maintinfo(&self, info: &MaintInfo) -> Result<(), Error> {
        self.update(|cache| {
            cache.maintinfo = serde_json::to_value(info).expect("serializable maint info");
        })
        .await
    }

    pub(crate) async fn clear_maintinfo(&self) -> Result<(), Error> {
        self.update(|cache| cache.maintinfo = serde_json::Value::Null)
            .await
    }

    pub(crate) async fn load_pllinfo(&self) -> Result<Option<PllInfo>, Error> {
        let _guard = cache_lock().lock().await;
        let cache = self.load_unlocked().await?;
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

    async fn load_unlocked(&self) -> Result<GameInfoCacheFile, Error> {
        self.ensure_file().await?;
        let text = fs::read_to_string(&self.data_path).await?;
        let value = serde_json::from_str::<GameInfoCacheFile>(&text)?;
        Ok(value)
    }

    async fn update(&self, mutator: impl FnOnce(&mut GameInfoCacheFile)) -> Result<(), Error> {
        let _guard = cache_lock().lock().await;
        self.ensure_file().await?;
        let mut cache = self.load_unlocked().await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{maintenance::MaintInfo, pll::StreamLink};

    #[tokio::test]
    async fn concurrent_cache_updates_preserve_maintenance_and_pll() {
        let unique = format!("dynamo-gameinfo-cache-test-{}", std::process::id());
        let root = std::env::temp_dir().join(unique);
        let store = GameInfoCacheStore {
            data_path: root.join("cache.json"),
            sample_path: root.join("sample.json"),
        };
        let maint = MaintInfo {
            id: "maint".into(),
            start_stamp: 100,
            end_stamp: 200,
            end_time_unknown: false,
            title_kr: "maint".into(),
            url: "maint-url".into(),
            scope: "전 월드".into(),
            body_fingerprint: 1,
            translated_description: None,
            stale: false,
        };
        let pll = PllInfo {
            fixed_title: "pll".into(),
            url: "pll-url".into(),
            start_stamp: Some(300),
            expire_time: 400,
            translated_description: None,
            translated_contents: Vec::new(),
            stream_links: vec![StreamLink {
                label: "watch".into(),
                url: "https://example.com".into(),
            }],
        };
        let (maint_result, pll_result) =
            tokio::join!(store.save_maintinfo(&maint), store.save_pllinfo(&pll));
        maint_result.unwrap();
        pll_result.unwrap();
        assert_eq!(store.load_maintinfo().await.unwrap().unwrap().id, "maint");
        assert_eq!(store.load_pllinfo().await.unwrap().unwrap().url, "pll-url");
        let _ = fs::remove_file(&store.data_path).await;
    }
}
