use crate::constants::{TICKET_CLOSE_BUTTON_ID, TICKET_CREATE_BUTTON_ID};
use poise::serenity_prelude::{ButtonStyle, CreateActionRow, CreateButton};

pub(crate) fn ticket_create_components() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(TICKET_CREATE_BUTTON_ID)
            .label("Open a ticket")
            .style(ButtonStyle::Success),
    ])]
}

pub(crate) fn ticket_close_components() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(TICKET_CLOSE_BUTTON_ID)
            .label("Close Ticket")
            .style(ButtonStyle::Primary)
            .emoji('🔒'),
    ])]
}
