use crate::{
    channels::current_guild_channel,
    constants::{TICKET_CATEGORY_SELECT_ID, TICKET_CLOSE_BUTTON_ID, TICKET_CREATE_BUTTON_ID},
    settings::load_settings,
    workflow::{close_ticket_channel, create_ticket_channel},
};
use dynamo_runtime_api::{AppState, Error};
use poise::serenity_prelude::{
    ComponentInteraction, CreateActionRow, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateSelectMenu, CreateSelectMenuKind,
    CreateSelectMenuOption, EditInteractionResponse, Interaction,
};

pub async fn handle(
    ctx: &poise::serenity_prelude::Context,
    interaction: &Interaction,
    data: &AppState,
) -> Result<bool, Error> {
    match interaction {
        Interaction::Component(component)
            if component.data.custom_id == TICKET_CREATE_BUTTON_ID =>
        {
            handle_ticket_open(ctx, component, data).await?;
            Ok(true)
        }
        Interaction::Component(component) if component.data.custom_id == TICKET_CLOSE_BUTTON_ID => {
            handle_ticket_close(ctx, component, data).await?;
            Ok(true)
        }
        Interaction::Component(component)
            if component.data.custom_id == TICKET_CATEGORY_SELECT_ID =>
        {
            handle_ticket_category_select(ctx, component, data).await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn handle_ticket_open(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
    data: &AppState,
) -> Result<(), Error> {
    let settings = load_settings(data, component.guild_id.map(|id| id.get())).await?;
    if settings.categories.len() > 1 {
        let options = settings
            .categories
            .iter()
            .map(|category| CreateSelectMenuOption::new(&category.name, &category.name))
            .collect::<Vec<_>>();

        component
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("Please choose a ticket category.")
                        .ephemeral(true)
                        .components(vec![CreateActionRow::SelectMenu(CreateSelectMenu::new(
                            TICKET_CATEGORY_SELECT_ID,
                            CreateSelectMenuKind::String { options },
                        ))]),
                ),
            )
            .await?;
        return Ok(());
    }

    component.defer_ephemeral(ctx).await?;
    let category_name = settings
        .categories
        .first()
        .map(|category| category.name.clone());
    let result = create_ticket_channel(ctx, component, &settings, category_name.as_deref()).await?;
    component
        .edit_response(ctx, EditInteractionResponse::new().content(result))
        .await?;
    Ok(())
}

async fn handle_ticket_category_select(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
    data: &AppState,
) -> Result<(), Error> {
    let settings = load_settings(data, component.guild_id.map(|id| id.get())).await?;
    let category_name = match &component.data.kind {
        poise::serenity_prelude::ComponentInteractionDataKind::StringSelect { values } => {
            values.first().cloned()
        }
        _ => None,
    };
    let Some(category_name) = category_name else {
        component
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("Please choose a valid category.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    };

    component.defer_ephemeral(ctx).await?;
    let result = create_ticket_channel(ctx, component, &settings, Some(&category_name)).await?;
    component
        .edit_response(ctx, EditInteractionResponse::new().content(result))
        .await?;
    Ok(())
}

async fn handle_ticket_close(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
    data: &AppState,
) -> Result<(), Error> {
    component.defer_ephemeral(ctx).await?;
    let Some(channel) = current_guild_channel(ctx, component.channel_id).await? else {
        component
            .edit_response(
                ctx,
                EditInteractionResponse::new()
                    .content("This action can only be used inside a guild text channel."),
            )
            .await?;
        return Ok(());
    };

    if !crate::channels::is_ticket_channel(&channel) {
        component
            .edit_response(
                ctx,
                EditInteractionResponse::new()
                    .content("This action can only be used in ticket channels."),
            )
            .await?;
        return Ok(());
    }

    let settings = load_settings(data, Some(channel.guild_id.get())).await?;
    let result = close_ticket_channel(ctx, &channel, &component.user.name, &settings, None).await?;
    component
        .edit_response(ctx, EditInteractionResponse::new().content(result))
        .await?;
    Ok(())
}
