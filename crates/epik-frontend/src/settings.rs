//! The settings window: two tabs — GitHub and Models — over the two
//! secrets and the entries of `config.toml`, in the arrangement a settings
//! dialog usually has. Apply writes the tab it is on, Ok writes every tab
//! that is dirty and closes, Esc closes and writes nothing.
//!
//! A secret's bytes live in exactly one place on this side: the password
//! input's reactive state. They arrive there through [`ipc::reveal_secret`]
//! and leave through [`ipc::save_secret`], and nowhere else — never a log
//! line, never a persisted structure.
//!
//! The reasoning — which fields are dirty, what Apply writes, what a
//! dropdown offers — is pure and sits beside the components, where a test
//! can reach it.

use epik::chat::ModelInfo;
use epik::config::{self, Config};
use epik::keystore::{Resolved, Secret};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::ipc;

/// The two tabs, in display order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Tab {
    GitHub,
    Models,
}

impl Tab {
    const ALL: [Tab; 2] = [Tab::GitHub, Tab::Models];

    fn label(self) -> &'static str {
        match self {
            Tab::GitHub => "GitHub",
            Tab::Models => "Models",
        }
    }

    /// The tab's fields, in display order: the secret first, because on a
    /// fresh install nothing below it can be filled in until it is there.
    fn fields(self) -> &'static [Field] {
        match self {
            Tab::GitHub => &[Field::Token, Field::Owner],
            Tab::Models => &[Field::Key, Field::Chat, Field::Agent],
        }
    }

    /// The secret the rest of the tab depends on.
    fn secret(self) -> Field {
        self.fields()[0]
    }

    /// The configuration entries: every field but the secret.
    fn entries(self) -> &'static [Field] {
        &self.fields()[1..]
    }
}

/// Every box on the panel. A secret's box and a configuration entry's box
/// are the same kind of thing to dirt and to Apply: text, compared with
/// what was loaded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Field {
    /// `GITHUB_TOKEN`, in the keyring.
    Token,
    /// `[github] owner`.
    Owner,
    /// `ANTHROPIC_API_KEY`, in the keyring.
    Key,
    /// `[model] chat`.
    Chat,
    /// `[model] agent`.
    Agent,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Token => "Token",
            Field::Owner => "Owner",
            Field::Key => "Anthropic API key",
            Field::Chat => "Chat",
            Field::Agent => "Agent",
        }
    }

    /// The keystore name of a secret field; the entries have none.
    fn keystore_name(self) -> &'static str {
        match self {
            Field::Token => "GITHUB_TOKEN",
            Field::Key => "ANTHROPIC_API_KEY",
            Field::Owner | Field::Chat | Field::Agent => unreachable!("not a secret"),
        }
    }

    fn of(self, fields: &Fields) -> &str {
        match self {
            Field::Token => &fields.token,
            Field::Owner => &fields.owner,
            Field::Key => &fields.key,
            Field::Chat => &fields.chat,
            Field::Agent => &fields.agent,
        }
    }

    fn slot(self, fields: &mut Fields) -> &mut String {
        match self {
            Field::Token => &mut fields.token,
            Field::Owner => &mut fields.owner,
            Field::Key => &mut fields.key,
            Field::Chat => &mut fields.chat,
            Field::Agent => &mut fields.agent,
        }
    }
}

/// What every box holds, as text. An absent configuration entry is the
/// empty string here and `None` in the file, both ways.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Fields {
    pub token: String,
    pub owner: String,
    pub key: String,
    pub chat: String,
    pub agent: String,
}

impl Fields {
    /// The entries as the configuration states them; the secrets empty,
    /// because they come from the keyring.
    pub(crate) fn from_config(config: &Config) -> Self {
        Self {
            owner: config.github.owner.clone().unwrap_or_default(),
            chat: config.model.chat.clone().unwrap_or_default(),
            agent: config.model.agent.clone().unwrap_or_default(),
            ..Self::default()
        }
    }

    /// The configuration these boxes state: an emptied entry is omitted.
    pub(crate) fn config(&self) -> Config {
        let entry = |text: &str| (!text.is_empty()).then(|| text.to_owned());
        Config {
            model: config::Model {
                chat: entry(&self.chat),
                agent: entry(&self.agent),
            },
            github: config::GitHub {
                owner: entry(&self.owner),
            },
        }
    }

    /// `self` with `tab`'s entries taken from `current` — the other tab's
    /// boxes untouched, which is what keeps its unapplied edits out of a
    /// write.
    fn patched(&self, tab: Tab, current: &Fields) -> Fields {
        let mut patched = self.clone();
        for &field in tab.entries() {
            *field.slot(&mut patched) = field.of(current).to_owned();
        }
        patched
    }
}

/// The fields of `tab` that differ from what was loaded.
pub(crate) fn dirty(tab: Tab, loaded: &Fields, current: &Fields) -> Vec<Field> {
    tab.fields()
        .iter()
        .copied()
        .filter(|field| field.of(loaded) != field.of(current))
        .collect()
}

/// What Apply writes for one tab, to the two stores it reaches.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct Writes {
    /// The keyring write: the secret when it is dirty and non-empty.
    /// Unchanged means no pointless keychain touch — on macOS a write can
    /// mean a permission prompt. Emptied means nothing, because clearing
    /// a secret is deliberately not a feature.
    pub secret: Option<Secret>,
    /// The file write: the loaded configuration patched with this tab's
    /// entries, when any of them is dirty.
    pub config: Option<Config>,
}

pub(crate) fn writes(tab: Tab, loaded: &Fields, current: &Fields) -> Writes {
    let secret = tab.secret().of(current);
    let patched = loaded.patched(tab, current);
    Writes {
        secret: (!secret.is_empty() && secret != tab.secret().of(loaded))
            .then(|| Secret::from(secret)),
        config: (patched != *loaded).then(|| patched.config()),
    }
}

/// One entry a dropdown offers: the text the configuration would state,
/// and the text shown for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Choice {
    pub value: String,
    pub shown: String,
}

/// What a dropdown offers over `fetched`, set to `configured`. With
/// `default`, an explicit Default entry comes first, standing for an
/// omitted entry. A configured model the list does not contain heads the
/// list shown by its id, so the list is never a reason a selection
/// changes; an empty list still offers the configured value.
pub(crate) fn choices(configured: &str, fetched: &[ModelInfo], default: bool) -> Vec<Choice> {
    let choice = |value: &str, shown: &str| Choice {
        value: value.to_owned(),
        shown: shown.to_owned(),
    };
    let unlisted = !configured.is_empty() && fetched.iter().all(|model| model.id != configured);
    default
        .then(|| choice("", "Default"))
        .into_iter()
        .chain(unlisted.then(|| choice(configured, configured)))
        .chain(fetched.iter().map(|m| choice(&m.id, &m.display_name)))
        .collect()
}

/// Where the model list stands, in the three-state manner of [`Resolved`].
#[derive(Clone, Debug, Eq, PartialEq)]
enum Models {
    Loading,
    Listed(Vec<ModelInfo>),
    /// The provider would not answer; the reason is the tab's to show.
    Refused(String),
}

/// The panel's reactive state. `RwSignal` is `Copy`, so the whole struct
/// is, and every closure takes it by value.
#[derive(Clone, Copy)]
struct Panel {
    /// What came out of the stores, for dirt to compare against.
    /// Re-baselined by each write that lands.
    loaded: RwSignal<Fields>,
    /// The boxes. The only home the secret bytes have on this side of IPC.
    current: RwSignal<Fields>,
    tab: RwSignal<Tab>,
    models: RwSignal<Models>,
    /// What went wrong most recently, or nothing.
    note: RwSignal<Option<String>>,
}

impl Panel {
    fn set_both(self, field: Field, text: &str) {
        self.loaded.update(|f| *field.slot(f) = text.to_owned());
        self.current.update(|f| *field.slot(f) = text.to_owned());
    }

    fn fetch_models(self) {
        self.models.set(Models::Loading);
        spawn_local(async move {
            self.models.set(match ipc::list_models().await {
                Ok(list) => Models::Listed(list),
                Err(reason) => Models::Refused(reason),
            });
        });
    }

    /// Fills an empty Owner with the stored token's login. A token that
    /// will not answer leaves the field empty and says why.
    fn fill_owner(self) {
        spawn_local(async move {
            match ipc::github_login().await {
                Ok(login) => self.current.update(|f| {
                    if f.owner.is_empty() {
                        f.owner = login;
                    }
                }),
                Err(reason) => self.note.set(Some(format!(
                    "The GitHub token's account is unknown: {reason}"
                ))),
            }
        });
    }

    /// Writes `tab`'s dirty fields and marks them clean. Each store is
    /// reported on its own: one failing leaves the other's result
    /// standing. True when nothing failed.
    async fn apply(self, tab: Tab) -> bool {
        let current = self.current.get_untracked();
        let due = writes(tab, &self.loaded.get_untracked(), &current);
        let mut failed = Vec::new();
        if let Some(config) = &due.config {
            match ipc::write_config(config).await {
                Ok(()) => self.loaded.update(|l| *l = l.patched(tab, &current)),
                Err(reason) => failed.push(format!("The configuration was not written: {reason}")),
            }
        }
        if let Some(secret) = &due.secret {
            let field = tab.secret();
            match ipc::save_secret(field.keystore_name(), secret).await {
                Ok(()) => {
                    self.loaded
                        .update(|l| *field.slot(l) = secret.reveal().to_owned());
                    match tab {
                        Tab::Models => self.fetch_models(),
                        Tab::GitHub if self.current.with_untracked(|f| f.owner.is_empty()) => {
                            self.fill_owner();
                        }
                        Tab::GitHub => {}
                    }
                }
                Err(reason) => failed.push(format!("{} was not saved: {reason}", field.label())),
            }
        }
        self.note
            .set((!failed.is_empty()).then(|| failed.join(" ")));
        failed.is_empty()
    }
}

/// The settings window's whole content. A fresh window every open, so the
/// eyeballs start hidden and every box starts from whatever its store
/// says.
#[component]
pub fn SettingsPage() -> impl IntoView {
    let panel = Panel {
        loaded: RwSignal::new(Fields::default()),
        current: RwSignal::new(Fields::default()),
        tab: RwSignal::new(Tab::GitHub),
        models: RwSignal::new(Models::Loading),
        note: RwSignal::new(None),
    };

    // The file, then the keyring, then what depends on the keyring.
    spawn_local(async move {
        match ipc::read_config().await {
            Ok(config) => {
                let fields = Fields::from_config(&config);
                for field in [Field::Owner, Field::Chat, Field::Agent] {
                    panel.set_both(field, field.of(&fields));
                }
            }
            Err(reason) => panel.note.set(Some(format!(
                "The configuration couldn't be read: {reason}"
            ))),
        }
        let mut token_found = false;
        for field in [Field::Token, Field::Key] {
            match ipc::reveal_secret(field.keystore_name()).await {
                Resolved::Found(secret) => {
                    panel.set_both(field, secret.reveal());
                    token_found |= field == Field::Token;
                }
                Resolved::Absent => {}
                Resolved::Unreachable(reason) => panel.note.set(Some(format!(
                    "The system keychain couldn't be reached: {reason}"
                ))),
            }
        }
        panel.fetch_models();
        if token_found && panel.current.with_untracked(|f| f.owner.is_empty()) {
            panel.fill_owner();
        }
    });

    // Esc: close, nothing changed, regardless of box contents.
    let handle = window_event_listener(ev::keydown, move |event| {
        if event.key() == "Escape" {
            ipc::close_settings();
        }
    });
    on_cleanup(move || handle.remove());

    let apply = move |_| {
        spawn_local(async move {
            panel.apply(panel.tab.get_untracked()).await;
        });
    };
    let ok = move |_| {
        spawn_local(async move {
            let mut clean = true;
            for tab in Tab::ALL {
                clean &= panel.apply(tab).await;
            }
            if clean {
                ipc::close_settings();
            }
        });
    };

    view! {
        <main class="flex h-screen flex-col bg-neutral-50 p-6 dark:bg-neutral-900">
            <nav class="flex gap-1 border-b border-neutral-300 dark:border-neutral-700">
                {Tab::ALL.map(|tab| tab_button(panel, tab)).collect_view()}
            </nav>
            <ul class="flex flex-col gap-3 pt-4">
                {move || {
                    panel
                        .tab
                        .get()
                        .fields()
                        .iter()
                        .map(|&field| field_row(panel, field))
                        .collect_view()
                }}
            </ul>
            <Show when=move || panel.tab.get() == Tab::Models>
                <p class="mt-3 text-sm text-neutral-500 dark:text-neutral-400">
                    {move || match panel.models.get() {
                        Models::Loading => Some("Fetching the model list…".to_owned()),
                        Models::Listed(_) => None,
                        Models::Refused(reason) => {
                            Some(format!("The model list couldn't be fetched: {reason}"))
                        }
                    }}
                </p>
            </Show>
            <Show when=move || panel.note.get().is_some()>
                <p class="mt-3 text-sm text-[#d4940a] dark:text-[#f5a623]">
                    {move || panel.note.get()}
                </p>
            </Show>
            <div class="mt-auto flex justify-end gap-2 pt-4">
                // Apply has nothing to do on a clean tab, and says so.
                <button
                    type="button"
                    class="rounded-md border border-neutral-300 px-4 py-1.5 text-sm font-medium text-neutral-700 hover:bg-neutral-200 disabled:opacity-50 disabled:hover:bg-transparent dark:border-neutral-700 dark:text-neutral-200 dark:hover:bg-neutral-800"
                    disabled=move || {
                        dirty(panel.tab.get(), &panel.loaded.get(), &panel.current.get()).is_empty()
                    }
                    on:click=apply
                >
                    "Apply"
                </button>
                <button
                    type="button"
                    class="rounded-md bg-[#00b377] px-4 py-1.5 text-sm font-medium text-white hover:bg-[#009966] dark:bg-[#00e599] dark:text-neutral-950 dark:hover:bg-[#33edb3]"
                    on:click=ok
                >
                    "Ok"
                </button>
            </div>
        </main>
    }
}

/// One tab in the strip; the current one carries the underline.
fn tab_button(panel: Panel, tab: Tab) -> impl IntoView {
    view! {
        <button
            type="button"
            class=move || {
                if panel.tab.get() == tab {
                    "-mb-px border-b-2 border-[#00b377] px-3 py-1.5 text-sm font-medium text-neutral-900 dark:border-[#00e599] dark:text-neutral-100"
                } else {
                    "-mb-px border-b-2 border-transparent px-3 py-1.5 text-sm text-neutral-500 hover:text-neutral-700 dark:text-neutral-400 dark:hover:text-neutral-200"
                }
            }
            on:click=move |_| panel.tab.set(tab)
        >
            {tab.label()}
        </button>
    }
}

const INPUT: &str = "min-w-0 flex-1 rounded-md border border-neutral-300 bg-white px-3 py-1.5 text-sm text-neutral-900 focus:border-[#00b377] focus:outline-none dark:border-neutral-700 dark:bg-neutral-950 dark:text-neutral-100 dark:focus:border-[#00e599]";

/// One labeled row: the label, then the control the field calls for.
fn field_row(panel: Panel, field: Field) -> impl IntoView {
    let id = format!("field-{}", field.label().to_lowercase().replace(' ', "-"));
    let value = move || panel.current.with(|f| field.of(f).to_owned());
    let set = move |ev| {
        panel
            .current
            .update(|f| *field.slot(f) = event_target_value(&ev))
    };
    let control = match field {
        Field::Token | Field::Key => secret_box(panel, field, id.clone()).into_any(),
        Field::Owner => view! {
            <input id=id.clone() type="text" autocomplete="off" class=INPUT prop:value=value on:input=set />
        }
        .into_any(),
        Field::Chat | Field::Agent => {
            let listed = move || match panel.models.get() {
                Models::Listed(list) => list,
                Models::Loading | Models::Refused(_) => Vec::new(),
            };
            let options = move || {
                let configured = value();
                choices(&configured, &listed(), field == Field::Agent)
                    .into_iter()
                    .map(|choice| {
                        let selected = choice.value == configured;
                        view! {
                            <option value=choice.value prop:selected=selected>
                                {choice.shown}
                            </option>
                        }
                    })
                    .collect_view()
            };
            view! {
                <select id=id.clone() class=INPUT prop:value=value on:change=set>
                    {options}
                </select>
            }
            .into_any()
        }
    };
    view! {
        <li class="flex items-center gap-3">
            <label for=id class="w-32 shrink-0 text-sm text-neutral-600 dark:text-neutral-400">
                {field.label()}
            </label>
            {control}
        </li>
    }
}

/// A password box and its eyeball.
fn secret_box(panel: Panel, field: Field, id: String) -> impl IntoView {
    let revealed = RwSignal::new(false);
    let value = move || panel.current.with(|f| field.of(f).to_owned());
    let set = move |ev| {
        panel
            .current
            .update(|f| *field.slot(f) = event_target_value(&ev))
    };
    view! {
        <input
            id=id
            type=move || if revealed.get() { "text" } else { "password" }
            autocomplete="off"
            class=format!("{INPUT} font-mono")
            prop:value=value
            on:input=set
        />
        <button
            type="button"
            aria-label="Reveal the secret"
            class="shrink-0 rounded-md p-1.5 text-neutral-500 hover:bg-neutral-200 hover:text-neutral-700 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-200"
            on:click=move |_| revealed.update(|shown| *shown = !*shown)
        >
            <svg
                class="h-4 w-4"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
            >
                <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7Z" />
                <circle cx="12" cy="12" r="3" />
                <Show when=move || revealed.get()>
                    <line x1="4" y1="20" x2="20" y2="4" />
                </Show>
            </svg>
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded() -> Fields {
        Fields {
            token: "ghp-kept".to_owned(),
            owner: "octo".to_owned(),
            key: "sk-kept".to_owned(),
            chat: "claude-chat".to_owned(),
            agent: String::new(),
        }
    }

    fn model(id: &str, name: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_owned(),
            display_name: name.to_owned(),
        }
    }

    #[test]
    fn the_tabs_lead_with_their_secret() {
        assert_eq!(Tab::GitHub.fields(), [Field::Token, Field::Owner]);
        assert_eq!(
            Tab::Models.fields(),
            [Field::Key, Field::Chat, Field::Agent]
        );
        assert_eq!(Tab::GitHub.secret().keystore_name(), "GITHUB_TOKEN");
        assert_eq!(Tab::Models.secret().keystore_name(), "ANTHROPIC_API_KEY");
    }

    #[test]
    fn nothing_is_dirty_as_loaded() {
        for tab in Tab::ALL {
            assert_eq!(dirty(tab, &loaded(), &loaded()), []);
        }
    }

    #[test]
    fn dirt_belongs_to_the_tab_whose_box_changed() {
        let mut current = loaded();
        current.owner = "other".to_owned();
        current.agent = "claude-agent".to_owned();
        assert_eq!(dirty(Tab::GitHub, &loaded(), &current), [Field::Owner]);
        assert_eq!(dirty(Tab::Models, &loaded(), &current), [Field::Agent]);
    }

    #[test]
    fn an_unchanged_tab_touches_neither_store() {
        for tab in Tab::ALL {
            assert_eq!(writes(tab, &loaded(), &loaded()), Writes::default());
        }
    }

    #[test]
    fn a_changed_secret_is_saved_and_the_file_left_alone() {
        let mut current = loaded();
        current.key = "sk-new".to_owned();
        assert_eq!(
            writes(Tab::Models, &loaded(), &current),
            Writes {
                secret: Some(Secret::from("sk-new")),
                config: None,
            }
        );
    }

    #[test]
    fn a_first_secret_is_saved() {
        let start = Fields {
            chat: "claude-chat".to_owned(),
            ..Fields::default()
        };
        let mut current = start.clone();
        current.token = "ghp-first".to_owned();
        assert_eq!(
            writes(Tab::GitHub, &start, &current).secret,
            Some(Secret::from("ghp-first"))
        );
    }

    #[test]
    fn an_emptied_secret_is_not_cleared() {
        let mut current = loaded();
        current.key = String::new();
        assert_eq!(writes(Tab::Models, &loaded(), &current), Writes::default());
    }

    #[test]
    fn a_changed_entry_patches_the_loaded_configuration() {
        let mut current = loaded();
        current.owner = "other".to_owned();
        let expected = Config {
            model: config::Model {
                chat: Some("claude-chat".to_owned()),
                agent: None,
            },
            github: config::GitHub {
                owner: Some("other".to_owned()),
            },
        };
        assert_eq!(
            writes(Tab::GitHub, &loaded(), &current),
            Writes {
                secret: None,
                config: Some(expected),
            }
        );
    }

    #[test]
    fn the_other_tabs_unapplied_edits_stay_out_of_the_write() {
        let mut current = loaded();
        current.owner = "other".to_owned();
        current.chat = "claude-later".to_owned();
        let config = writes(Tab::GitHub, &loaded(), &current).config.unwrap();
        assert_eq!(config.github.owner.as_deref(), Some("other"));
        assert_eq!(config.model.chat.as_deref(), Some("claude-chat"));
        let config = writes(Tab::Models, &loaded(), &current).config.unwrap();
        assert_eq!(config.model.chat.as_deref(), Some("claude-later"));
        assert_eq!(config.github.owner.as_deref(), Some("octo"));
    }

    #[test]
    fn an_emptied_entry_writes_as_omitted() {
        let mut start = loaded();
        start.agent = "claude-agent".to_owned();
        let config = writes(Tab::Models, &start, &loaded()).config.unwrap();
        assert_eq!(config.model.agent, None);
        assert_eq!(config.model.chat.as_deref(), Some("claude-chat"));
    }

    #[test]
    fn a_filled_in_login_counts_as_dirty() {
        let mut start = loaded();
        start.owner = String::new();
        assert_eq!(dirty(Tab::GitHub, &start, &loaded()), [Field::Owner]);
        let config = writes(Tab::GitHub, &start, &loaded()).config.unwrap();
        assert_eq!(config.github.owner.as_deref(), Some("octo"));
    }

    #[test]
    fn fields_and_config_agree_both_ways() {
        let config = loaded().config();
        assert_eq!(config.model.agent, None);
        let back = Fields::from_config(&config);
        assert_eq!(back.token, "");
        assert_eq!(back.key, "");
        assert_eq!(
            (back.owner, back.chat, back.agent),
            (loaded().owner, loaded().chat, String::new())
        );
    }

    fn fetched() -> Vec<ModelInfo> {
        vec![
            model("claude-new", "Claude New"),
            model("claude-old", "Claude Old"),
        ]
    }

    fn choice(value: &str, shown: &str) -> Choice {
        Choice {
            value: value.to_owned(),
            shown: shown.to_owned(),
        }
    }

    #[test]
    fn a_listed_model_shows_display_names_in_fetched_order() {
        assert_eq!(
            choices("claude-old", &fetched(), false),
            [
                choice("claude-new", "Claude New"),
                choice("claude-old", "Claude Old"),
            ]
        );
    }

    #[test]
    fn an_unlisted_model_stays_in_the_list_by_its_id() {
        assert_eq!(
            choices("claude-mine", &fetched(), false),
            [
                choice("claude-mine", "claude-mine"),
                choice("claude-new", "Claude New"),
                choice("claude-old", "Claude Old"),
            ]
        );
    }

    #[test]
    fn an_empty_list_still_offers_the_configured_model() {
        assert_eq!(
            choices("claude-mine", &[], false),
            [choice("claude-mine", "claude-mine")]
        );
        assert_eq!(choices("", &[], false), []);
    }

    #[test]
    fn the_agent_list_leads_with_default_for_an_absent_entry() {
        assert_eq!(
            choices("", &fetched(), true),
            [
                choice("", "Default"),
                choice("claude-new", "Claude New"),
                choice("claude-old", "Claude Old"),
            ]
        );
        assert_eq!(choices("", &[], true), [choice("", "Default")]);
    }

    #[test]
    fn the_agent_list_keeps_default_first_before_an_unlisted_model() {
        assert_eq!(
            choices("claude-mine", &fetched(), true)[..2],
            [choice("", "Default"), choice("claude-mine", "claude-mine")]
        );
    }
}
