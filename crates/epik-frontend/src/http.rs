//! The browser's feed: the change log as Server-Sent Events from the
//! origin the page came from.
//!
//! `EventSource` does what the window's channel does and one thing
//! more: it reconnects on its own, sending the last `id:` it saw, and
//! the server resumes from there — so a loss is made good without a
//! reload, and what was missed arrives as if heard. There is no replay
//! call because there is no snapshot: the stream from 0 is the replay,
//! and a page opened after a build has finished folds that build from
//! it.

use std::rc::Rc;

use epik::monitor::Entry;
use wasm_bindgen::prelude::*;
use web_sys::{EventSource, MessageEvent};

use crate::monitor::Feed;

/// The stream's path on the server, beside the page.
const CHANGES: &str = "/monitor/changes";

/// The event stream over HTTP.
pub struct Monitor;

impl Feed for Monitor {
    /// The handlers live for the page, which is why they are forgotten;
    /// the source itself is the browser's to keep while it has them.
    fn listen(
        &self,
        hear: impl Fn(Entry) + 'static,
        standing: impl Fn(Result<(), String>) + 'static,
    ) {
        let Ok(source) = EventSource::new(CHANGES) else {
            standing(Err("the browser would not open the event stream".to_owned()));
            return;
        };
        let standing = Rc::new(standing);
        let message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let entry = event
                .data()
                .as_string()
                .and_then(|data| serde_json::from_str(&data).ok());
            if let Some(entry) = entry {
                hear(entry);
            }
        });
        let open = Closure::<dyn FnMut(JsValue)>::new({
            let standing = Rc::clone(&standing);
            move |_| standing(Ok(()))
        });
        let error = Closure::<dyn FnMut(JsValue)>::new({
            let source = source.clone();
            move |_| {
                // The browser gives up only on a stream it cannot use — a
                // wrong content type, a 404; a dropped one it retries.
                let reason = if source.ready_state() == EventSource::CLOSED {
                    "the event stream closed"
                } else {
                    "the event stream dropped; reconnecting"
                };
                standing(Err(reason.to_owned()));
            }
        });
        source.set_onmessage(Some(message.as_ref().unchecked_ref()));
        source.set_onopen(Some(open.as_ref().unchecked_ref()));
        source.set_onerror(Some(error.as_ref().unchecked_ref()));
        message.forget();
        open.forget();
        error.forget();
    }

    /// The stream is the replay.
    fn replay(&self, deliver: impl FnOnce(Result<Vec<Entry>, String>) + 'static) {
        deliver(Ok(Vec::new()));
    }
}
