use dynamo_access::module_access_for_context;
use dynamo_runtime_api::{Context, Error};

use crate::module::MODULE_ID;

pub(crate) async fn ensure_module_access(ctx: Context<'_>) -> Result<bool, Error> {
    let Some(reason_message) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    else {
        return Ok(true);
    };

    ctx.send(
        poise::CreateReply::default()
            .content(reason_message)
            .ephemeral(true),
    )
    .await?;
    Ok(false)
}
