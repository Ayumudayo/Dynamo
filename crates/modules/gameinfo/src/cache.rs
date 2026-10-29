use crate::{
    maintenance::{MaintInfo, MaintenanceCancellation},
    pll::PllInfo,
};
use dynamo_runtime_api::Error;
use serde::Deserialize;
use std::{path::PathBuf, sync::OnceLock};
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
    #[serde(rename = "MAINT_CANCELLED", default)]
    cancellations: Vec<MaintenanceCancellation>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

pub(crate) struct GameInfoCacheStore {
    data_path: PathBuf,
    sample_path: PathBuf,
    legacy_path: Option<PathBuf>,
    legacy_search_root: Option<PathBuf>,
}

impl GameInfoCacheStore {
    pub(crate) fn new() -> Self {
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let data_dir = std::env::var_os("DYNAMO_DATA_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| working_dir.join("logs"));
        Self {
            data_path: data_dir.join("gameinfo-cache.json"),
            sample_path: data_dir.join("gameinfo-cache.sample.json"),
            legacy_path: std::env::var_os("DYNAMO_LEGACY_GAMEINFO_CACHE_PATH")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            legacy_search_root: Some(working_dir),
        }
    }

    pub(crate) async fn load_maintinfo(&self) -> Result<Option<MaintInfo>, Error> {
        Ok(self.load_maintenance_state().await?.0)
    }

    pub(crate) async fn load_maintenance_state(
        &self,
    ) -> Result<(Option<MaintInfo>, Vec<MaintenanceCancellation>), Error> {
        let _guard = cache_lock().lock().await;
        let cache = self.load_unlocked().await?;
        let info = parse_optional::<MaintInfo>(cache.maintinfo)
            .filter(|info| !cache.cancellations.iter().any(|entry| entry.matches(info)));
        Ok((info, cache.cancellations))
    }

    pub(crate) async fn save_maintinfo(&self, info: &MaintInfo) -> Result<(), Error> {
        self.update(|cache| {
            if !cache.cancellations.iter().any(|entry| entry.matches(info)) {
                cache.maintinfo = serde_json::to_value(info).expect("serializable maint info");
            }
        })
        .await
    }

    pub(crate) async fn save_maintenance_cancellation(
        &self,
        entry: &MaintenanceCancellation,
    ) -> Result<(), Error> {
        self.update(|cache| {
            if !cache.cancellations.iter().any(|old| old == entry) {
                cache.cancellations.push(entry.clone());
            }
            if parse_optional::<MaintInfo>(cache.maintinfo.clone())
                .is_some_and(|info| entry.matches(&info))
            {
                cache.maintinfo = serde_json::Value::Null;
            }
        })
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

        if let Some(legacy_path) = self.find_legacy_cache().await? {
            let content = fs::read_to_string(&legacy_path).await?;
            // Validate before importing; do not silently replace damaged data.
            serde_json::from_str::<GameInfoCacheFile>(&content)?;
            let tmp = self.data_path.with_extension("json.tmp");
            fs::write(&tmp, content).await?;
            fs::rename(tmp, &self.data_path).await?;
            tracing::info!(source = %legacy_path.display(), destination = %self.data_path.display(),
                "migrated gameinfo cache; original retained");
            return Ok(());
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

    async fn find_legacy_cache(&self) -> Result<Option<PathBuf>, Error> {
        if let Some(path) = &self.legacy_path {
            if !fs::try_exists(path).await? {
                anyhow::bail!(
                    "configured legacy gameinfo cache does not exist: {}",
                    path.display()
                );
            }
            return Ok(Some(path.clone()));
        }
        // Old Windows cross-builds created this literal directory on Linux,
        // then traversed three parents. Only recognize that concrete marker;
        // never import an arbitrary parent directory's cache.
        let Some(root) = &self.legacy_search_root else {
            return Ok(None);
        };
        if cfg!(windows) {
            return Ok(None);
        }
        let mut entries = fs::read_dir(root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(":\\")
                && name.ends_with("\\crates\\modules\\gameinfo")
                && entry.file_type().await?.is_dir()
            {
                let Some(ancestor) = root.parent().and_then(|parent| parent.parent()) else {
                    continue;
                };
                let path = ancestor.join("logs/gameinfo-cache.json");
                if fs::try_exists(&path).await? {
                    return Ok(Some(path));
                }
            }
        }
        Ok(None)
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

    fn test_store(label: &str) -> GameInfoCacheStore {
        let root =
            std::env::temp_dir().join(format!("dynamo-cache-{label}-{}", std::process::id()));
        GameInfoCacheStore {
            data_path: root.join("cache.json"),
            sample_path: root.join("sample.json"),
            legacy_path: None,
            legacy_search_root: None,
        }
    }

    fn notice() -> MaintInfo {
        MaintInfo {
            id: "original".into(),
            url: "original".into(),
            start_stamp: 100,
            end_stamp: 200,
            end_time_unknown: false,
            title_kr: "점검".into(),
            scope: "전 월드".into(),
            body_fingerprint: 1,
            translated_description: None,
            stale: false,
        }
    }

    #[tokio::test]
    async fn legacy_import_preserves_source_and_existing_destination_wins() {
        let mut store = test_store("legacy");
        fs::create_dir_all(store.data_path.parent().unwrap())
            .await
            .unwrap();
        let legacy = store.data_path.with_file_name("legacy.json");
        let payload =
            serde_json::json!({"MAINTINFO": notice(), "PLLINFO": {}, "custom": {"retain": true}})
                .to_string();
        fs::write(&legacy, &payload).await.unwrap();
        store.legacy_path = Some(legacy.clone());
        assert_eq!(
            store.load_maintinfo().await.unwrap().unwrap().url,
            "original"
        );
        assert_eq!(fs::read_to_string(&legacy).await.unwrap(), payload);
        let mut newer = notice();
        newer.url = "newer".into();
        store.save_maintinfo(&newer).await.unwrap();
        assert_eq!(store.load_maintinfo().await.unwrap().unwrap().url, "newer");
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&store.data_path).await.unwrap()).unwrap();
        assert_eq!(persisted["custom"]["retain"], true);
        fs::remove_file(&store.data_path).await.unwrap();
        fs::remove_file(&legacy).await.unwrap();
        fs::remove_dir(store.data_path.parent().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn invalid_legacy_cache_is_not_promoted_to_empty_cache() {
        let mut store = test_store("legacy-invalid");
        fs::create_dir_all(store.data_path.parent().unwrap())
            .await
            .unwrap();
        let legacy = store.data_path.with_file_name("legacy.json");
        fs::write(&legacy, "broken-json").await.unwrap();
        store.legacy_path = Some(legacy.clone());
        assert!(store.load_maintinfo().await.is_err());
        assert!(!fs::try_exists(&store.data_path).await.unwrap());
        assert_eq!(fs::read_to_string(&legacy).await.unwrap(), "broken-json");
        fs::remove_file(&legacy).await.unwrap();
        fs::remove_dir(store.data_path.parent().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancellation_survives_reload_and_blocks_original_and_followup() {
        let store = test_store("cancel-reload");
        let original = notice();
        store.save_maintinfo(&original).await.unwrap();
        let cancellation = MaintenanceCancellation {
            notice_url: "cancel".into(),
            original_url: Some(original.url.clone()),
            scope: original.scope.clone(),
            start_stamp: Some(original.start_stamp),
            start_date: None,
        };
        store
            .save_maintenance_cancellation(&cancellation)
            .await
            .unwrap();
        let reloaded = test_store("cancel-reload");
        assert!(reloaded.load_maintinfo().await.unwrap().is_none());
        reloaded.save_maintinfo(&original).await.unwrap();
        let mut followup = original.clone();
        followup.id = "followup".into();
        followup.url = "followup".into();
        reloaded.save_maintinfo(&followup).await.unwrap();
        assert!(reloaded.load_maintinfo().await.unwrap().is_none());
        followup.start_stamp = 300;
        followup.end_stamp = 400;
        reloaded.save_maintinfo(&followup).await.unwrap();
        assert_eq!(
            reloaded
                .load_maintinfo()
                .await
                .unwrap()
                .unwrap()
                .start_stamp,
            300
        );
        fs::remove_file(&store.data_path).await.unwrap();
        fs::remove_dir(store.data_path.parent().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn failed_cancellation_write_preserves_cache_and_can_retry() {
        let store = test_store("cancel-retry");
        let original = notice();
        store.save_maintinfo(&original).await.unwrap();
        let blocked_tmp = store.data_path.with_extension("json.tmp");
        fs::create_dir(&blocked_tmp).await.unwrap();
        let cancellation = MaintenanceCancellation {
            notice_url: "cancel".into(),
            original_url: Some(original.url.clone()),
            scope: original.scope.clone(),
            start_stamp: Some(100),
            start_date: None,
        };
        assert!(
            store
                .save_maintenance_cancellation(&cancellation)
                .await
                .is_err()
        );
        let (still_present, cancellations) = store.load_maintenance_state().await.unwrap();
        assert!(still_present.is_some());
        assert!(cancellations.is_empty());
        fs::remove_dir(&blocked_tmp).await.unwrap();
        store
            .save_maintenance_cancellation(&cancellation)
            .await
            .unwrap();
        assert!(store.load_maintinfo().await.unwrap().is_none());
        fs::remove_file(&store.data_path).await.unwrap();
        fs::remove_dir(store.data_path.parent().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn concurrent_cache_updates_preserve_maintenance_and_pll() {
        let unique = format!("dynamo-gameinfo-cache-test-{}", std::process::id());
        let root = std::env::temp_dir().join(unique);
        let store = GameInfoCacheStore {
            data_path: root.join("cache.json"),
            sample_path: root.join("sample.json"),
            legacy_path: None,
            legacy_search_root: None,
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
