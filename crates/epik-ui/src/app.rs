//! The window's components.
//!
//! Everything reactive here reads one [`Pane`], and the pane is a pure fold
//! tested next door. What is left in this file is arrangement and appearance:
//! no rule about what the user sees should be discoverable only by running it.
//!
//! Components are called by the `view!` macro, never by code that could
//! sensibly discard the result, so `#[must_use]` on each one is noise.
#![allow(clippy::must_use_candidate)]

use epik::session::Status;
use leptos::ev::KeyboardEvent;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::bridge;
use crate::markdown;
use crate::pane::{Outcome, Pane, Turn};
use crate::theme::Theme;

/// The whole window: a header, a transcript, and somewhere to type.
#[component]
pub fn App() -> impl IntoView {
    let pane = RwSignal::new(Pane::default());
    let theme = RwSignal::new(Theme::default());
    let draft = RwSignal::new(String::new());
    let transcript = NodeRef::<leptos::html::Div>::new();

    // The document element carries the theme, so one attribute flips every
    // token at once and no component has to know a theme exists.
    Effect::new(move |_| {
        let attribute = theme.get().attribute();
        if let Some(root) = document().document_element() {
            let _ = root.set_attribute("data-theme", attribute);
        }
    });

    // The event stream, subscribed once for the life of the window. The
    // envelope's conversation is ignored: there is exactly one, and ignoring
    // the field is the whole of what v0 does with it.
    bridge::listen(move |envelope| {
        pane.update(|pane| pane.saw(envelope.event));
    });

    // Who is answering. Asked on mount, because the status bar has to be able
    // to name the model before anything else happens.
    Effect::new(move |_| {
        spawn_local(async move {
            match bridge::status().await {
                Ok(status) => pane.update(|pane| pane.opened(status)),
                Err(error) => pane.update(|pane| pane.refused(error)),
            }
        });
    });

    // A reply arriving off the bottom of the window is a reply nobody reads.
    Effect::new(move |_| {
        // Reading the pane is what makes this run again on every delta.
        let _ = pane.with(|pane| (pane.turns().len(), pane.streaming().map(str::len)));
        if let Some(element) = transcript.get() {
            element.set_scroll_top(element.scroll_height());
        }
    });

    let send = move || {
        let text = draft.with(|draft| draft.trim().to_owned());
        if text.is_empty() {
            return;
        }
        // The pane refuses a second turn, and so would the host. Neither should
        // ever have to, because the input is disabled — but a UI that works only
        // while its own guard holds is a UI with one guard.
        if !pane
            .try_update(|pane| pane.asked(text.clone()))
            .unwrap_or(false)
        {
            return;
        }
        draft.set(String::new());
        spawn_local(async move {
            if let Err(error) = bridge::send_message(&text).await {
                let returned = pane.try_update(|pane| pane.unsent(error)).flatten();
                if let Some(text) = returned {
                    draft.set(text);
                }
            }
        });
    };

    view! {
        <div class="flex h-full flex-col overflow-hidden bg-root font-sans text-primary">
            <Header theme=theme />

            <div node_ref=transcript class="min-h-0 flex-1 overflow-y-auto">
                <div class="mx-auto flex w-full max-w-3xl flex-col gap-4 px-5 py-6">
                    <Show when=move || {
                        pane.with(|pane| pane.turns().is_empty() && !pane.is_busy())
                    }>
                        <Opening />
                    </Show>

                    <For
                        each=move || {
                            pane.with(|pane| {
                                pane.turns().iter().cloned().enumerate().collect::<Vec<_>>()
                            })
                        }
                        key=|entry| entry.clone()
                        children=move |(_, turn)| view! { <TurnView turn=turn /> }
                    />

                    <Show when=move || pane.with(Pane::is_busy)>
                        <Streaming pane=pane />
                    </Show>

                    <Show when=move || pane.with(Pane::needs_key)>
                        <KeyCard pane=pane />
                    </Show>

                    <Show when=move || pane.with(Pane::needs_github_token)>
                        <PatCard pane=pane />
                    </Show>
                </div>
            </div>

            <Refusal pane=pane />
            <Composer pane=pane draft=draft send=send />
            <StatusBar pane=pane />
        </div>
    }
}

/// The other channel. A command that came back refused shows here, above the
/// composer where the next attempt will be made — never as a card in the
/// transcript, which is where a turn that broke mid-stream goes. The two are
/// different colours, in different places, saying different things, because they
/// are different things.
#[component]
fn Refusal(pane: RwSignal<Pane>) -> impl IntoView {
    view! {
        <Show when=move || pane.with(|pane| pane.refusal().is_some())>
            <div class="shrink-0 border-t border-warning bg-warning-muted px-5 py-2">
                <p class="mx-auto w-full max-w-3xl text-xs">
                    <span class="font-medium text-warning">"Epik could not do that. "</span>
                    <span class="text-secondary">
                        {move || pane.with(|pane| pane.refusal().unwrap_or_default().to_owned())}
                    </span>
                </p>
            </div>
        </Show>
    }
}

/// Somewhere to type, disabled while a turn is in flight.
#[component]
fn Composer<F>(pane: RwSignal<Pane>, draft: RwSignal<String>, send: F) -> impl IntoView
where
    F: Fn() + Copy + 'static,
{
    view! {
        <div class="shrink-0 border-t border-edge bg-surface px-5 py-3">
            <div class="mx-auto flex w-full max-w-3xl items-end gap-2">
                <textarea
                    class="max-h-40 min-h-[2.75rem] flex-1 resize-none rounded-lg border border-edge bg-input px-3 py-2.5 text-sm leading-6 text-primary placeholder:text-faint focus:border-edge-strong focus:outline-none disabled:opacity-60"
                    rows="1"
                    placeholder="Say something to Epik"
                    prop:value=move || draft.get()
                    prop:disabled=move || pane.with(Pane::is_busy)
                    on:input=move |event| draft.set(event_target_value(&event))
                    on:keydown=move |event: KeyboardEvent| {
                        // Enter sends; shift-Enter is a new line, as every chat
                        // window has taught everybody to expect.
                        if event.key() == "Enter" && !event.shift_key() {
                            event.prevent_default();
                            send();
                        }
                    }
                ></textarea>
                <button
                    class="rounded-lg bg-accent px-4 py-2.5 text-sm font-medium text-on-accent hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50"
                    prop:disabled=move || pane.with(Pane::is_busy)
                    on:click=move |_| send()
                >
                    {move || if pane.with(Pane::is_busy) { "Answering" } else { "Send" }}
                </button>
            </div>
        </div>
    }
}

/// The window's own chrome: what it is, and the one control it has.
#[component]
fn Header(theme: RwSignal<Theme>) -> impl IntoView {
    view! {
        <header class="flex shrink-0 items-center justify-between border-b border-edge bg-bar px-5 py-3">
            <div class="flex items-center gap-2.5">
                <Mark />
                <span class="text-sm font-medium tracking-tight">"Epik"</span>
            </div>
            <button
                class="rounded-md border border-edge px-2.5 py-1 text-xs text-secondary hover:bg-hover hover:text-primary"
                title="Switch between light and dark"
                on:click=move |_| theme.update(|theme| *theme = theme.flipped())
            >
                {move || theme.get().invitation()}
            </button>
        </header>
    }
}

/// The brand's mark, as `brand.json` describes it: four nodes, four edges, two
/// of them in the accent. `currentColor` for the rest, so it is in whichever
/// theme surrounds it.
#[component]
fn Mark() -> impl IntoView {
    view! {
        <svg viewBox="0 0 22 22" class="h-5 w-5 text-primary" aria-hidden="true">
            <g stroke="currentColor" stroke-width="1.2" opacity="0.35">
                <line x1="5" y1="5" x2="17" y2="8" />
                <line x1="5" y1="5" x2="8" y2="17" />
                <line x1="17" y1="8" x2="14" y2="17" />
                <line x1="8" y1="17" x2="14" y2="17" />
            </g>
            <circle cx="5" cy="5" r="2.5" class="fill-accent" />
            <circle cx="17" cy="8" r="2" fill="currentColor" />
            <circle cx="8" cy="17" r="2" fill="currentColor" />
            <circle cx="14" cy="17" r="1.6" class="fill-accent" opacity="0.7" />
        </svg>
    }
}

/// The empty transcript. Chrome, and deliberately not shaped like a turn: the
/// greeting this design is named for is the model's to say, and this is only the
/// window saying what it is while it waits to be asked.
#[component]
fn Opening() -> impl IntoView {
    view! {
        <div class="flex flex-col items-center gap-1.5 py-20 text-center">
            <p class="text-base text-secondary">"Hello, I'm Epik."</p>
            <p class="text-xs text-muted">"You say it. We make it."</p>
        </div>
    }
}

/// One completed entry in the transcript.
#[component]
fn TurnView(turn: Turn) -> impl IntoView {
    match turn {
        // The user's own words, verbatim: their whitespace is theirs, and
        // nothing they type is markup.
        Turn::User { text } => view! {
            <div class="flex justify-end">
                <div class="max-w-[85%] whitespace-pre-wrap rounded-2xl rounded-br-md bg-accent-muted px-4 py-2.5 text-sm leading-6">
                    {text}
                </div>
            </div>
        }
        .into_any(),
        // Markdown, and only once the turn is complete: half-written markdown
        // renders as neither one thing nor the other.
        Turn::Assistant { text } => view! {
            <div
                class="prose max-w-none text-sm leading-6"
                inner_html=markdown::to_html(&text)
            ></div>
        }
        .into_any(),
        // A tool the model worked through: a modest line, deliberately — #97
        // owns the rich version. A refusal shows its own words, because "no
        // GitHub token" is an answer the user is owed.
        Turn::Tool { name, outcome } => {
            let (status, error) = match outcome {
                Outcome::Running => ("running…", None),
                Outcome::Finished => ("done", None),
                Outcome::Refused { error } => ("refused", Some(error)),
            };
            view! {
                <div class="rounded-md border border-edge bg-raised px-3 py-2 font-mono text-xs text-secondary">
                    <p>{format!("⚙ {name} · {status}")}</p>
                    {error
                        .map(|error| {
                            view! {
                                <p class="mt-1 whitespace-pre-wrap text-warning">{error}</p>
                            }
                        })}
                </div>
            }
            .into_any()
        }
        // A streamed failure: an error card, sitting where the rest of the
        // answer would have been. Command errors look nothing like this.
        Turn::Failed { error } => view! {
            <div class="rounded-lg border border-error bg-error-muted px-4 py-3">
                <p class="text-sm font-medium text-error">"Failed"</p>
                <p class="mt-1 whitespace-pre-wrap font-mono text-xs text-secondary">{error}</p>
            </div>
        }
        .into_any(),
    }
}

/// The reply as it arrives: plain text and a caret, because markdown mid-word is
/// worse than no markdown at all. It becomes markdown the moment it lands.
#[component]
fn Streaming(pane: RwSignal<Pane>) -> impl IntoView {
    view! {
        <div class="streaming whitespace-pre-wrap text-sm leading-6 text-primary">
            {move || pane.with(|pane| pane.streaming().unwrap_or_default().to_owned())}
        </div>
    }
}

/// Which secret a card asks for: the one difference between the two cards'
/// machinery, so it is the whole of what they pass around. Each case knows
/// its own host command, its input's invitation, and where its keyring
/// trouble is reported.
#[derive(Clone, Copy)]
enum Secret {
    /// The active provider's chat key.
    Key,
    /// The GitHub token.
    GithubToken,
}

impl Secret {
    /// Files the paste with the host, answering with the session as it now
    /// stands.
    // As in `bridge`: a future that awaits JavaScript is never `Send`, and
    // nothing here is ever moved off the web's one thread.
    #[allow(clippy::future_not_send)]
    async fn store(self, pasted: &str) -> Result<Status, String> {
        match self {
            Self::Key => bridge::set_api_key(pasted).await,
            Self::GithubToken => bridge::set_github_token(pasted).await,
        }
    }

    const fn placeholder(self) -> &'static str {
        match self {
            Self::Key => "Paste the key",
            Self::GithubToken => "Paste the token",
        }
    }

    /// The environment variable that answers for this secret when the
    /// keyring cannot.
    const fn env(self) -> &'static str {
        match self {
            Self::Key => "EPIK_API_KEY",
            Self::GithubToken => "EPIK_GITHUB_TOKEN",
        }
    }

    /// Why the keyring could not be consulted for this secret, when it
    /// could not be.
    fn trouble(self, pane: &Pane) -> Option<&str> {
        match self {
            Self::Key => pane.key_trouble(),
            Self::GithubToken => pane.github_trouble(),
        }
    }
}

/// The shape both paste-a-secret cards share: a box that names what is
/// wanted, takes one paste, and files it with the host — `opened` on
/// success, the banner on refusal. The words are the children's business;
/// the machinery lives once, here.
#[component]
fn SecretCard(
    pane: RwSignal<Pane>,
    secret: Secret,
    title: impl IntoView + 'static,
    children: Children,
) -> impl IntoView {
    let typed = RwSignal::new(String::new());

    let store = move || {
        let pasted = typed.with(|typed| typed.trim().to_owned());
        if pasted.is_empty() {
            return;
        }
        typed.set(String::new());
        spawn_local(async move {
            match secret.store(&pasted).await {
                Ok(status) => pane.update(|pane| pane.opened(status)),
                Err(error) => pane.update(|pane| pane.refused(error)),
            }
        });
    };

    view! {
        <div class="rounded-lg border border-edge bg-raised px-4 py-3.5">
            <p class="text-sm font-medium">{title}</p>
            {children()}
            <div class="mt-3 flex items-center gap-2">
                <input
                    type="password"
                    class="min-w-0 flex-1 rounded-md border border-edge bg-input px-3 py-2 font-mono text-xs text-primary placeholder:text-faint focus:border-edge-strong focus:outline-none"
                    placeholder=secret.placeholder()
                    autocomplete="off"
                    prop:value=move || typed.get()
                    on:input=move |event| typed.set(event_target_value(&event))
                    on:keydown=move |event: KeyboardEvent| {
                        if event.key() == "Enter" {
                            event.prevent_default();
                            store();
                        }
                    }
                />
                <button
                    class="rounded-md border border-edge px-3 py-2 text-xs font-medium text-primary hover:bg-hover"
                    on:click=move |_| store()
                >
                    "Ok"
                </button>
            </div>
        </div>
    }
}

/// The keyring-unreachable warning, one sentence with a card-shaped hole in
/// it. A keyring that cannot be reached did not stop the session from
/// opening, but it changes what a paste means, and the moment to say so is
/// before somebody pastes.
#[component]
fn Trouble(
    pane: RwSignal<Pane>,
    secret: Secret,
    /// What pasting means on this machine, completing "could not be
    /// reached, so …".
    consequence: &'static str,
    /// What setting the override variable gets, completing "Set VAR …".
    remedy: &'static str,
) -> impl IntoView {
    view! {
        <Show when=move || pane.with(|pane| secret.trouble(pane).is_some())>
            <p class="mt-2 text-xs text-warning">
                {format!("This computer's keyring could not be reached, so {consequence}: ")}
                <span class="font-mono">
                    {move || {
                        pane.with(|pane| secret.trouble(pane).unwrap_or_default().to_owned())
                    }}
                </span>
                ". Set "
                <span class="font-mono">{secret.env()}</span>
                {format!(" {remedy}.")}
            </p>
        </Show>
    }
}

/// Somewhere to put a key, shown when the configured provider has none.
///
/// The key goes to the operating system's keyring and Epik keeps a reference,
/// never a value — which is worth saying on the card itself, since a window
/// asking for a secret should say where it is about to put it.
#[component]
fn KeyCard(pane: RwSignal<Pane>) -> impl IntoView {
    let provider = move || {
        pane.with(|pane| {
            pane.status()
                .map_or_else(String::new, |status| status.provider.clone())
        })
    };

    view! {
        <SecretCard
            pane=pane
            secret=Secret::Key
            title=move || format!("{} needs a key", provider())
        >
            <p class="mt-1 text-xs text-secondary">
                "It goes into this computer's keyring, under the service "
                <span class="font-mono text-primary">"Epik"</span>
                ". Epik keeps a reference to it and never a copy — not in its config file, not anywhere else."
            </p>
            <Trouble
                pane=pane
                secret=Secret::Key
                consequence="a key pasted here will not be kept"
                remedy="instead, or use a provider that wants no key"
            />
        </SecretCard>
    }
}

/// Somewhere to put a GitHub token, shown when a GitHub verb has refused for
/// want of one — never sooner, because a chat that stays away from GitHub
/// owes nobody a token.
///
/// The guidance names every source worth naming and no more: the one-click
/// classic-PAT URL, the GitHub CLI's own credential, and the fine-grained
/// shape for whoever wants the tighter scope. The pastes that cannot serve
/// are refused by the library with their own sentence, which lands in the
/// banner — guidance delivered only to the person who needs it.
#[component]
fn PatCard(pane: RwSignal<Pane>) -> impl IntoView {
    view! {
        <SecretCard pane=pane secret=Secret::GithubToken title="GitHub needs a token">
            <p class="mt-1 text-xs text-secondary">
                "Create one at "
                <span class="select-all font-mono text-primary">
                    "https://github.com/settings/tokens/new?scopes=repo&description=Epik"
                </span>
                " — the page arrives pre-scoped, so it is one Generate and a copy. If you use the GitHub CLI, "
                <span class="font-mono text-primary">"gh auth token"</span>
                " prints one. A fine-grained PAT with Issues, Pull requests, and Contents access works too."
            </p>
            <p class="mt-1 text-xs text-secondary">
                "It goes into this computer's keyring, under the service "
                <span class="font-mono text-primary">"Epik"</span>
                ". Epik keeps a reference to it and never a copy."
            </p>
            <Trouble
                pane=pane
                secret=Secret::GithubToken
                consequence="a token pasted here works for this session but will not be kept"
                remedy="to make it stick"
            />
        </SecretCard>
    }
}

/// What is answering, and what the last turn cost.
///
/// This is the one thing the design promises in place of a settings UI: edit the
/// config, restart, and read here which provider took.
#[component]
fn StatusBar(pane: RwSignal<Pane>) -> impl IntoView {
    view! {
        <div class="flex shrink-0 items-center justify-between gap-4 border-t border-edge bg-bar px-5 py-1.5 font-mono text-[11px] text-muted">
            <span class="truncate">
                {move || {
                    pane.with(|pane| {
                        pane.status()
                            .map_or_else(
                                || "no session".to_owned(),
                                |status| format!("{} · {}", status.model, status.provider),
                            )
                    })
                }}
            </span>
            <span class="shrink-0">
                {move || {
                    pane.with(|pane| {
                        pane.usage()
                            .map(|usage| {
                                format!(
                                    "{} in · {} out",
                                    usage.prompt_tokens,
                                    usage.completion_tokens,
                                )
                            })
                    })
                }}
            </span>
        </div>
    }
}
