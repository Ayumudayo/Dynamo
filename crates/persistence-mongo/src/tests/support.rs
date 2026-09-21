use futures_util::FutureExt;
use mongodb::{Client, bson::oid::ObjectId};
use std::{
    any::Any,
    env,
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
};

use crate::MongoPersistence;

type PanicPayload = Box<dyn Any + Send + 'static>;

pub(super) enum IsolatedTestOutcome {
    Completed(anyhow::Result<()>),
    Panicked(PanicPayload),
}

struct IsolatedMongoTest {
    client: Client,
    database_name: String,
}

impl IsolatedMongoTest {
    async fn create() -> anyhow::Result<Self> {
        let connection_string = env::var("MONGODB_URI_FOR_ISOLATED_TEST").map_err(|_| {
            anyhow::anyhow!("isolated Mongo tests require the dedicated PowerShell runner")
        })?;
        let client = Client::with_uri_str(connection_string)
            .await
            .map_err(|_| anyhow::anyhow!("isolated Mongo client initialization failed"))?;

        Ok(Self {
            client,
            database_name: isolated_database_name(),
        })
    }

    fn store(&self) -> MongoPersistence {
        MongoPersistence::from_database(self.client.database(&self.database_name))
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        self.client
            .database(&self.database_name)
            .drop()
            .await
            .map_err(|_| anyhow::anyhow!("isolated Mongo database drop failed"))?;

        let remaining_databases = self
            .client
            .list_database_names()
            .await
            .map_err(|_| anyhow::anyhow!("isolated Mongo cleanup verification failed"))?;
        anyhow::ensure!(
            !remaining_databases
                .iter()
                .any(|name| name == &self.database_name),
            "isolated Mongo database still exists after cleanup"
        );
        Ok(())
    }
}

pub(super) fn isolated_database_name() -> String {
    format!(
        "dynmongo_{}_{}",
        std::process::id(),
        ObjectId::new().to_hex()
    )
}

pub(super) fn resolve_isolated_test_outcome(
    test_outcome: IsolatedTestOutcome,
    cleanup_result: anyhow::Result<()>,
) -> anyhow::Result<()> {
    if let Err(error) = cleanup_result {
        return Err(anyhow::anyhow!("isolated Mongo cleanup failed: {error}"));
    }

    match test_outcome {
        IsolatedTestOutcome::Completed(result) => result,
        IsolatedTestOutcome::Panicked(payload) => resume_unwind(payload),
    }
}

pub(super) async fn run_isolated_test_lifecycle<F, Fut, C, CleanupFut>(
    test: F,
    cleanup: C,
) -> anyhow::Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
    C: FnOnce() -> CleanupFut,
    CleanupFut: Future<Output = anyhow::Result<()>>,
{
    let test_outcome = match catch_unwind(AssertUnwindSafe(test)) {
        Ok(future) => match AssertUnwindSafe(future).catch_unwind().await {
            Ok(result) => IsolatedTestOutcome::Completed(result),
            Err(payload) => IsolatedTestOutcome::Panicked(payload),
        },
        Err(payload) => IsolatedTestOutcome::Panicked(payload),
    };
    let cleanup_result = cleanup().await;

    resolve_isolated_test_outcome(test_outcome, cleanup_result)
}

pub(super) async fn run_isolated_mongo_test<F, Fut>(test: F) -> anyhow::Result<()>
where
    F: FnOnce(MongoPersistence) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let isolated = IsolatedMongoTest::create().await?;
    let store = isolated.store();

    run_isolated_test_lifecycle(|| test(store), || isolated.cleanup()).await
}
