#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dynamo_dashboard::run_production().await
}
