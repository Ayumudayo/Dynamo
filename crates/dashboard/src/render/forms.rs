//! The settings dialog, served as an HTML fragment when a user opens it.

use dynamo_module_kit::{SettingsField, SettingsFieldKind, SettingsSchema};
use maud::{Markup, html};
use serde_json::Value;

use crate::i18n::Locale;
use crate::render::entities::{EntityRow, modal_id, render_status_badge, render_status_parts};
use crate::status::{EntityKind, Scope};

pub(crate) struct DialogToggle<'a> {
    pub(crate) name: &'static str,
    pub(crate) label: &'a str,
    pub(crate) help: &'a str,
    pub(crate) checked: bool,
}

pub(crate) struct DialogSpec<'a> {
    pub(crate) scope: Scope,
    pub(crate) row: &'a EntityRow,
    pub(crate) toggles: Vec<DialogToggle<'a>>,
    pub(crate) schema: &'a SettingsSchema,
    pub(crate) configuration: &'a Value,
    pub(crate) patch_url: String,
    pub(crate) reconcile_url: String,
    /// Whether the save request carries `configuration` (deployment modules
    /// have no configurable fields and only send their switches).
    pub(crate) sends_configuration: bool,
}

pub(crate) fn render_settings_dialog(spec: &DialogSpec<'_>, locale: Locale) -> Markup {
    let t = locale.t();
    let row = spec.row;
    let modal_id = modal_id(spec.scope, row.kind, &row.id);
    let testid = modal_id.replace(':', "-");
    let item_key = row.id.replace(':', "-");
    let kind = row.kind.key();
    let title_id = format!("modal-title-{modal_id}");
    let field_prefix = sanitize_id(&modal_id);
    let (eyebrow, empty_fields) = match row.kind {
        EntityKind::Module => (t.dialog_module_settings, t.dialog_no_module_fields),
        EntityKind::Command => (t.dialog_command_settings, t.dialog_no_command_fields),
    };
    html! {
        div.settings-modal-overlay id=(modal_id) data-testid={ "settings-modal-" (testid) } data-modal-overlay hidden {
            div.settings-modal data-modal-root role="dialog" aria-modal="true" aria-labelledby=(title_id) tabindex="-1" {
                form.settings-form
                    data-testid={ (spec.scope.key()) "-" (kind) "-form-" (item_key) }
                    data-settings-form
                    data-entity-kind=(kind)
                    data-entity-id=(row.id)
                    data-entity-name=(row.name)
                    data-patch-url=(spec.patch_url)
                    data-reconcile-url=(spec.reconcile_url)
                    data-sends-configuration=(spec.sends_configuration)
                    data-guild-id=[spec.scope.guild_id()]
                    novalidate {
                    div.settings-modal-head {
                        div {
                            p.eyebrow { (eyebrow) }
                            h2 id=(title_id) { (row.name) }
                        }
                        button.icon-button type="button" data-action="close-modal"
                            data-testid={ "modal-close-" (testid) } aria-label=(t.dialog_close) {
                            span.icon-close aria-hidden="true" { "×" }
                        }
                    }
                    div.settings-modal-body {
                        div.dialog-status data-entity-status data-entity-kind=(kind) data-entity-id=(row.id) {
                            div.page-header-meta {
                                span.field-label { (t.dialog_status) }
                                (render_status_badge(&row.status, locale))
                            }
                            (render_status_parts(&row.status, locale))
                        }
                        @for (index, toggle) in spec.toggles.iter().enumerate() {
                            @let id = format!("{field_prefix}-switch-{index}");
                            div.toggle-row {
                                div.toggle-row-text {
                                    strong id={ (id) "-label" } { (toggle.label) }
                                    span.field-help id={ (id) "-help" } { (toggle.help) }
                                }
                                label.switch {
                                    input type="checkbox" name=(toggle.name) data-entity-switch=(toggle.name)
                                        aria-labelledby={ (id) "-label" } aria-describedby={ (id) "-help" }
                                        checked[toggle.checked];
                                    span.switch-track aria-hidden="true" {}
                                }
                            }
                        }
                        @if spec.sends_configuration {
                            @if spec.schema.sections.is_empty() {
                                p.muted.small { (empty_fields) }
                            }
                            @for section in &spec.schema.sections {
                                fieldset.settings-section {
                                    legend { (locale.catalog(section.title)) }
                                    @if let Some(description) = section.description {
                                        p.field-help { (locale.catalog(description)) }
                                    }
                                    div.settings-grid {
                                        @for field in &section.fields {
                                            (render_field(field, spec.configuration, &field_prefix, locale))
                                        }
                                        @if let Some(target) = embed_preview_target(&section.fields) {
                                            (render_embed_preview(&target, locale))
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div.settings-modal-foot {
                        div.discard-bar data-discard-bar hidden role="alertdialog" aria-label=(t.dialog_unsaved) {
                            p { (t.dialog_unsaved) }
                            button.button.button-secondary.button-compact type="button" data-action="keep-editing" { (t.dialog_keep_editing) }
                            button.button.button-ghost.button-compact type="button" data-action="discard-changes" { (t.dialog_discard) }
                        }
                        span.modal-status data-modal-status role="status" aria-live="polite" {}
                        button.button.button-secondary type="button" data-action="cancel-modal"
                            data-testid={ "cancel-settings-" (item_key) } { (t.dialog_cancel) }
                        button.button.button-primary type="submit" data-testid={ "save-settings-" (item_key) } { (t.dialog_save) }
                    }
                }
            }
        }
    }
}

/// Element ids are namespaced by dialog so fields with the same key in
/// different modules never collide.
fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

pub(crate) fn field_control_id(prefix: &str, key: &str) -> String {
    format!("{prefix}-field-{}", sanitize_id(key))
}

fn kind_key(kind: &SettingsFieldKind) -> &'static str {
    match kind {
        SettingsFieldKind::Toggle => "toggle",
        SettingsFieldKind::Text => "text",
        SettingsFieldKind::Textarea => "textarea",
        SettingsFieldKind::Url => "url",
        SettingsFieldKind::Color => "color",
        SettingsFieldKind::Channel => "channel",
        SettingsFieldKind::Role => "role",
        SettingsFieldKind::RoleList => "role_list",
        SettingsFieldKind::TextList => "text_list",
        SettingsFieldKind::Json => "json",
        SettingsFieldKind::InviteRanks => "invite_ranks",
        SettingsFieldKind::TicketCategories => "ticket_categories",
        SettingsFieldKind::Integer { .. } => "integer",
        SettingsFieldKind::Select { .. } => "select",
    }
}

fn field_placeholder_tokens(key: &str) -> Option<&'static [&'static str]> {
    if matches!(
        key,
        "welcome.content"
            | "welcome.embed.description"
            | "welcome.embed.footer"
            | "farewell.content"
            | "farewell.embed.description"
            | "farewell.embed.footer"
    ) {
        Some(&[
            "{member:name}",
            "{member:mention}",
            "{member:tag}",
            "{server}",
            "{count}",
        ])
    } else if key == "xp.message" {
        Some(&["{user}", "{level}"])
    } else {
        None
    }
}

pub(crate) fn render_field(
    field: &SettingsField,
    configuration: &Value,
    prefix: &str,
    locale: Locale,
) -> Markup {
    let t = locale.t();
    let control_id = field_control_id(prefix, field.key);
    let help_id = format!("{control_id}-help");
    let error_id = format!("{control_id}-error");
    let label = locale.catalog(field.label);
    let help = field.help_text.map(|help| locale.catalog(help));
    let described_by = match help {
        Some(_) => format!("{help_id} {error_id}"),
        None => error_id.clone(),
    };
    let kind = kind_key(&field.kind);
    let wide = matches!(
        field.kind,
        SettingsFieldKind::Textarea
            | SettingsFieldKind::RoleList
            | SettingsFieldKind::TextList
            | SettingsFieldKind::Json
            | SettingsFieldKind::InviteRanks
            | SettingsFieldKind::TicketCategories
            | SettingsFieldKind::Toggle
    );

    if let SettingsFieldKind::Toggle = field.kind {
        let checked = value_at_path(configuration, field.key)
            .and_then(Value::as_bool)
            .unwrap_or(false);
        return html! {
            div.toggle-row.span-2 data-testid={ "field-" (field.key.replace(':', "-")) } {
                div.toggle-row-text {
                    strong id={ (control_id) "-label" } { (label) }
                    @if let Some(help) = help { span.field-help id=(help_id) { (help) } }
                }
                label.switch {
                    input type="checkbox" id=(control_id)
                        data-setting-key=(field.key) data-setting-kind="toggle"
                        aria-labelledby={ (control_id) "-label" }
                        aria-describedby=[help.map(|_| help_id.as_str())]
                        checked[checked];
                    span.switch-track aria-hidden="true" {}
                }
            }
        };
    }

    let scalar = field_string_value(configuration, field.key).unwrap_or_default();
    let list = field_list_value(configuration, field.key);
    html! {
        div.field.span-2[wide] data-testid={ "field-" (field.key.replace(':', "-")) } data-field-kind=(kind) {
            label for=(control_id) {
                (label)
                @if field.required { span.required-mark aria-hidden="true" { "*" } span.visually-hidden { " (" (t.field_required) ")" } }
            }
            @if let Some(help) = help { span.field-help id=(help_id) { (help) } }
            @match &field.kind {
                SettingsFieldKind::Integer { min, max } => {
                    input id=(control_id) type="number" inputmode="numeric" step="1"
                        data-setting-key=(field.key) data-setting-kind=(kind)
                        value=(scalar) min=[*min] max=[*max]
                        data-required=[field.required.then_some("true")]
                        aria-describedby=(described_by);
                }
                SettingsFieldKind::Select { options } => {
                    select id=(control_id) data-setting-key=(field.key) data-setting-kind=(kind)
                        data-required=[field.required.then_some("true")]
                        aria-describedby=(described_by) {
                        @for option in options {
                            option value=(option.value) selected[scalar == option.value] { (locale.catalog(option.label)) }
                        }
                    }
                }
                SettingsFieldKind::Color => {
                    div.color-field {
                        input type="color" data-color-picker-for=(control_id) aria-label=(t.field_color_picker)
                            value=(normalized_hex(&scalar).unwrap_or_else(|| "#068add".to_string()));
                        input id=(control_id) type="text" data-setting-key=(field.key) data-setting-kind=(kind)
                            value=(scalar) placeholder="#068ADD" spellcheck="false" autocomplete="off"
                            data-required=[field.required.then_some("true")]
                            aria-describedby=(described_by);
                    }
                }
                SettingsFieldKind::Channel | SettingsFieldKind::Role => {
                    @let picker = if matches!(field.kind, SettingsFieldKind::Channel) { "channel" } else { "role" };
                    @let placeholder = if picker == "channel" { t.field_channel_placeholder } else { t.field_role_placeholder };
                    input id=(control_id) type="text" inputmode="numeric" spellcheck="false" autocomplete="off"
                        data-setting-key=(field.key) data-setting-kind=(kind) data-picker=(picker)
                        value=(scalar) placeholder=(placeholder)
                        data-required=[field.required.then_some("true")]
                        aria-describedby=(described_by);
                }
                SettingsFieldKind::RoleList => {
                    textarea.mono id=(control_id) rows="3" spellcheck="false"
                        data-setting-key=(field.key) data-setting-kind=(kind) data-picker="role-list"
                        placeholder=(t.field_role_list_placeholder)
                        data-required=[field.required.then_some("true")]
                        aria-describedby=(described_by) { (list) }
                }
                SettingsFieldKind::TextList => {
                    textarea id=(control_id) rows="4" spellcheck="false"
                        data-setting-key=(field.key) data-setting-kind=(kind)
                        placeholder=(t.field_one_per_line)
                        data-required=[field.required.then_some("true")]
                        aria-describedby=(described_by) { (list) }
                }
                SettingsFieldKind::Json => {
                    textarea.mono id=(control_id) rows="6" spellcheck="false"
                        data-setting-key=(field.key) data-setting-kind=(kind)
                        placeholder=(t.field_json_hint)
                        data-required=[field.required.then_some("true")]
                        aria-describedby=(described_by) { (scalar) }
                }
                SettingsFieldKind::InviteRanks | SettingsFieldKind::TicketCategories => {
                    @let array_json = if scalar.trim().is_empty() { "[]" } else { scalar.as_str() };
                    textarea id=(control_id) hidden
                        data-setting-key=(field.key) data-setting-kind=(kind)
                        data-required=[field.required.then_some("true")]
                        aria-describedby=(described_by) { (array_json) }
                    div.structured-editor
                        data-structured-editor=(kind)
                        data-for=(control_id) {}
                }
                SettingsFieldKind::Textarea => {
                    textarea id=(control_id) rows="4"
                        data-setting-key=(field.key) data-setting-kind=(kind)
                        data-required=[field.required.then_some("true")]
                        aria-describedby=(described_by) { (scalar) }
                }
                SettingsFieldKind::Url => {
                    input id=(control_id) type="url" inputmode="url" spellcheck="false"
                        data-setting-key=(field.key) data-setting-kind=(kind)
                        value=(scalar) placeholder="https://"
                        data-required=[field.required.then_some("true")]
                        aria-describedby=(described_by);
                }
                SettingsFieldKind::Text | SettingsFieldKind::Toggle => {
                    input id=(control_id) type="text"
                        data-setting-key=(field.key) data-setting-kind=(kind)
                        value=(scalar)
                        data-required=[field.required.then_some("true")]
                        aria-describedby=(described_by);
                }
            }
            @if let Some(tokens) = field_placeholder_tokens(field.key) {
                div.placeholder-chips role="group" aria-label=(t.placeholder_chips_label) {
                    @for token in tokens {
                        button.chip.placeholder-chip type="button"
                            data-insert-token=(token)
                            data-target-field=(control_id) {
                            (token)
                        }
                    }
                }
            }
            p.field-error id=(error_id) data-field-error aria-live="polite" {}
        }
    }
}

enum PreviewTarget {
    Greeting(String),
    Ticket,
}

/// Greeting-style sections describe a message plus an embed; ticket setup
/// sections configure the embed card created by `/ticket setup`.
fn embed_preview_target(fields: &[SettingsField]) -> Option<PreviewTarget> {
    if let Some(prefix) = fields
        .iter()
        .find_map(|field| field.key.strip_suffix(".embed.description"))
    {
        return Some(PreviewTarget::Greeting(prefix.to_string()));
    }
    if fields
        .iter()
        .any(|field| matches!(field.key, "setup_title" | "setup_description"))
    {
        return Some(PreviewTarget::Ticket);
    }
    None
}

fn render_embed_preview(target: &PreviewTarget, locale: Locale) -> Markup {
    let t = locale.t();
    let (preview_attr, show_sample_toggle, is_ticket) = match target {
        PreviewTarget::Greeting(prefix) => (prefix.as_str(), true, false),
        PreviewTarget::Ticket => ("__ticket", false, true),
    };
    html! {
        div.embed-preview data-embed-preview=(preview_attr) {
            div.embed-preview-head {
                span.field-label { (t.preview_title) }
                @if show_sample_toggle {
                    label.preview-sample-toggle {
                        input type="checkbox" data-preview-sample;
                        span { (t.preview_sample_toggle) }
                    }
                }
            }
            div.embed-preview-frame aria-live="off" {
                @if !is_ticket {
                    div.embed-preview-content data-preview-part="content" {}
                }
                div.embed-preview-embed data-preview-part="embed" hidden {
                    @if is_ticket {
                        h4.embed-preview-title data-preview-part="title" {}
                    }
                    div.embed-preview-desc data-preview-part="description" {}
                    @if !is_ticket {
                        div.embed-preview-image data-preview-part="image" {}
                    }
                    div.embed-preview-footer data-preview-part="footer" {}
                }
                p.muted.small data-preview-part="empty" { (t.preview_empty) }
            }
            @if show_sample_toggle {
                span.field-help { (t.preview_note) }
            }
        }
    }
}

fn normalized_hex(value: &str) -> Option<String> {
    let hex = value.trim().trim_start_matches('#');
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_lowercase()))
}

pub(crate) fn value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

pub(crate) fn field_string_value(configuration: &Value, key: &str) -> Option<String> {
    let value = value_at_path(configuration, key)?;
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Array(_) | Value::Object(_) => serde_json::to_string_pretty(value).ok(),
    }
}

/// Lists are edited one item per line. Values saved by the old JSON editor (a
/// JSON array string) are shown the same way.
pub(crate) fn field_list_value(configuration: &Value, key: &str) -> String {
    let Some(value) = value_at_path(configuration, key) else {
        return String::new();
    };
    let items = match value {
        Value::Array(items) => items.clone(),
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Array(items)) => items,
            _ => return text.clone(),
        },
        Value::Null => return String::new(),
        other => vec![other.clone()],
    };
    items
        .iter()
        .map(|item| match item {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(key: &'static str, kind: SettingsFieldKind) -> SettingsField {
        SettingsField {
            key,
            label: "Channel",
            help_text: Some("Target channel for the greeting."),
            required: false,
            kind,
        }
    }

    #[test]
    fn channel_field_is_namespaced_labelled_and_picker_enabled() {
        let rendered = render_field(
            &field("welcome.channel", SettingsFieldKind::Channel),
            &serde_json::json!({ "welcome": { "channel": "123" } }),
            "modal-guild-module-greeting",
            Locale::En,
        )
        .into_string();
        let id = "modal-guild-module-greeting-field-welcome-channel";
        assert!(rendered.contains(&format!("for=\"{id}\"")));
        assert!(rendered.contains(&format!("id=\"{id}\"")));
        assert!(rendered.contains(&format!("aria-describedby=\"{id}-help {id}-error\"")));
        assert!(rendered.contains("data-picker=\"channel\""));
        assert!(rendered.contains("data-setting-kind=\"channel\""));
        assert!(rendered.contains("value=\"123\""));
    }

    #[test]
    fn same_key_in_two_dialogs_gets_distinct_ids() {
        let field = field("enabled", SettingsFieldKind::Text);
        let a =
            render_field(&field, &Value::Null, "modal-guild-module-a", Locale::En).into_string();
        let b =
            render_field(&field, &Value::Null, "modal-guild-module-b", Locale::En).into_string();
        assert!(a.contains("id=\"modal-guild-module-a-field-enabled\""));
        assert!(b.contains("id=\"modal-guild-module-b-field-enabled\""));
    }

    #[test]
    fn list_fields_render_one_item_per_line_including_legacy_json_strings() {
        assert_eq!(
            field_list_value(&serde_json::json!({ "ids": ["1", 2] }), "ids"),
            "1\n2"
        );
        assert_eq!(
            field_list_value(&serde_json::json!({ "ids": "[\"1\",\"2\"]" }), "ids"),
            "1\n2"
        );
    }

    #[test]
    fn field_labels_are_translated() {
        let rendered = render_field(
            &field("welcome.channel", SettingsFieldKind::Channel),
            &Value::Null,
            "p",
            Locale::Ko,
        )
        .into_string();
        assert!(rendered.contains(">채널<"));
        assert!(rendered.contains("인사 메시지를 보낼 채널입니다."));
    }

    #[test]
    fn values_are_escaped() {
        let rendered = render_field(
            &field("x", SettingsFieldKind::Text),
            &serde_json::json!({ "x": "\"><script>alert(1)</script>" }),
            "p",
            Locale::En,
        )
        .into_string();
        assert!(!rendered.contains("<script>"));
        assert!(rendered.contains("&quot;&gt;&lt;script&gt;"));
    }
}
