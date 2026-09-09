//! The monitor as a page a browser can open: the bundle the window runs,
//! served over loopback HTTP, with the change log as an event stream
//! beside it.
//!
//! One renderer, two transports. The page is the wasm bundle Trunk
//! emits, embedded here so a headless backend is one file; the entries
//! reach it as Server-Sent Events, `id: <boot>.<seq>`, and a
//! reconnecting browser's `Last-Event-ID` is the cursor to resume from
//! — when its boot is this process's. A connection with none, or with
//! another process's id, replays from 0: attaching is replay, there is
//! no snapshot to serve, and a relaunched backend is a new log whose
//! `seq` restarts at 0, which an old cursor must not be read against.
//! The surface is read-only — it serves the page and the changes, and
//! can start nothing, stop nothing and answer nothing.
//!
//! `tiny_http`: blocking, a thread per connection, and off Tauri's async
//! runtime — the `epik` library is synchronous throughout and the server
//! keeps that discipline. A connection blocks in [`Log::wait_after`], so
//! an idle monitor costs a sleeping thread and nothing else.
//!
//! It binds loopback and refuses any other address, in words that say
//! why: remote access is an ssh tunnel until there is an authentication
//! story. Loopback is not the whole of the story, though: a page from
//! any origin whose name is re-pointed at 127.0.0.1 — DNS rebinding —
//! reaches a loopback server with requests the browser does not
//! restrict, so every request's `Host` must name this server, and one
//! that does not is refused with 421.

use std::ffi::OsStr;
use std::io::Write;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use epik::monitor::{Entry, Log};
use include_dir::{Dir, File, include_dir};
use tiny_http::{Header, Method, Request, Response, Server};

/// The bundle Trunk emitted, as of this build. `build.rs` makes the
/// directory exist before this is compiled, so a build that never ran
/// Trunk embeds an empty bundle and serves 404s for the page.
static BUNDLE: Dir = include_dir!("$CARGO_MANIFEST_DIR/../epik-frontend/dist");

/// The mark `trunk serve` leaves in a bundle it meant to serve itself:
/// the address its autoreload client dials, a placeholder the dev
/// server fills in on the way out and nothing else does.
const DEV_MARKER: &str = "__trunk_address__";

/// Whether the embedded bundle is `trunk serve`'s. Decided once; the
/// bundle does not change while the process runs.
static DEV_BUNDLE: LazyLock<bool> = LazyLock::new(|| {
    BUNDLE
        .get_file("index.html")
        .is_some_and(|index| dev_bundle(index.contents()))
});

/// Whether `index` is a dev-server bundle's page: one whose autoreload
/// client would dial the placeholder from any other server, forever.
/// Such a bundle is not served; the page is a sentence saying what to
/// run instead.
fn dev_bundle(index: &[u8]) -> bool {
    index
        .windows(DEV_MARKER.len())
        .any(|window| window == DEV_MARKER.as_bytes())
}

/// What the page says in place of a dev-server bundle.
const DEV_REFUSAL: &str = "the embedded monitor page is a trunk serve bundle; run trunk build \
in crates/epik-frontend and build the backend again\n";

/// The event stream's path.
pub const CHANGES: &str = "/monitor/changes";

/// This process's mark on every event id, `<boot>.<seq>`: unix
/// milliseconds at first use. A relaunched backend is a new log whose
/// `seq` restarts at 0, and a browser reconnecting with the old
/// process's `Last-Event-ID` must not resume from it — a cursor into a
/// log that no longer exists — so the id says which log it indexes.
static BOOT: LazyLock<u64> = LazyLock::new(|| {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
});

/// How long a connection waits for an entry before saying it is still
/// there — a comment line, so a client that has gone is noticed by the
/// write that fails.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// What `setup` calls: decides, serves, and reports to stderr whatever
/// stops it. The app carries on without a server either way.
pub fn start(listen: Option<&str>, log: &Arc<Log>) {
    let Some(listen) = listen else { return };
    match decide(listen).and_then(|addr| serve(addr, Arc::clone(log))) {
        Ok(addr) => eprintln!("the monitor is at http://{addr}/"),
        Err(reason) => eprintln!("{reason}"),
    }
}

/// Whether `listen` names an address the monitor will bind: `ip:port`
/// or `host:port`, resolved, with every address it resolves to on a
/// loopback interface — `localhost` names two, and a name that reaches
/// anywhere else is refused whole rather than bound on its loopback
/// half. The first address is the one bound. The Err is the sentence
/// for stderr.
pub fn decide(listen: &str) -> Result<SocketAddr, String> {
    let addrs: Vec<SocketAddr> = listen
        .to_socket_addrs()
        .map_err(|error| {
            format!(
                "the monitor's listen address {listen:?} is not a host:port it can resolve \
                 ({error}); no monitor server"
            )
        })?
        .collect();
    if let Some(routable) = addrs.iter().find(|addr| !addr.ip().is_loopback()) {
        return Err(format!(
            "the monitor refuses to listen on {listen} ({routable}): it binds loopback only, \
             and reaching it from another machine is an ssh tunnel until there is an \
             authentication story; no monitor server"
        ));
    }
    addrs.first().copied().ok_or_else(|| {
        format!("the monitor's listen address {listen:?} resolves to nothing; no monitor server")
    })
}

/// Serves `log` and the bundle on `addr` from a thread of its own, and
/// answers the address bound — which is how port 0 says which port.
pub fn serve(addr: SocketAddr, log: Arc<Log>) -> Result<SocketAddr, String> {
    let server = Server::http(addr)
        .map_err(|error| format!("the monitor could not listen on {addr}: {error}"))?;
    let bound = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| format!("the monitor bound {addr} to no IP address"))?;
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let log = Arc::clone(&log);
            thread::spawn(move || handle(request, &log, bound.ip()));
        }
    });
    Ok(bound)
}

/// Whether `host` — a request's `Host` header, or none — names this
/// server: `localhost`, a loopback literal, or the address bound, with
/// or without a port. Anything else is a page from some other origin
/// whose name has been pointed at loopback, and the browser's own
/// same-origin rules do not restrict it, so this check has to.
fn admitted(host: Option<&str>, bound: IpAddr) -> bool {
    let Some(host) = host else { return false };
    let name = match host.strip_prefix('[') {
        // `[::1]:7878`, or `[::1]`.
        Some(rest) => rest.split(']').next().unwrap_or(rest),
        // A bare `::1` has more than one colon; `localhost:7878` has one.
        None if host.matches(':').count() > 1 => host,
        None => host.rsplit_once(':').map_or(host, |(name, _)| name),
    };
    name.eq_ignore_ascii_case("localhost")
        || name
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback() || ip == bound)
}

/// Where a request goes, decided from the method and the path alone.
#[derive(PartialEq)]
enum Route {
    /// The event stream.
    Changes,
    /// One file of the bundle.
    Asset(&'static File<'static>),
    /// A file of a bundle `trunk serve` wrote, which is not served.
    DevBundle,
    /// A path the bundle does not hold.
    NotFound,
    /// Anything but GET: the surface is read-only.
    NotAllowed,
}

/// `/` is the page; anything else is looked up in the bundle as it is,
/// so `..` and its like find nothing.
fn route(method: &Method, url: &str) -> Route {
    if *method != Method::Get {
        return Route::NotAllowed;
    }
    let path = url.split('?').next().unwrap_or(url);
    let file = match path {
        CHANGES => return Route::Changes,
        "/" => "index.html",
        _ => path.trim_start_matches('/'),
    };
    match BUNDLE.get_file(file) {
        Some(_) if *DEV_BUNDLE => Route::DevBundle,
        Some(file) => Route::Asset(file),
        None => Route::NotFound,
    }
}

/// The content type an asset is served as, by its extension.
fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(OsStr::to_str) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn header(field: &str, value: &str) -> Header {
    Header::from_bytes(field, value).expect("ascii header text")
}

fn handle(request: Request, log: &Log, bound: IpAddr) {
    let host = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Host"))
        .map(|header| header.value.as_str().to_owned());
    if !admitted(host.as_deref(), bound) {
        let refused =
            Response::from_string("the monitor answers to localhost only\n").with_status_code(421);
        let _ = request.respond(refused);
        return;
    }
    let response = match route(request.method(), request.url()) {
        Route::Changes => return stream(request, log),
        Route::Asset(file) => Response::from_data(file.contents())
            .with_header(header("Content-Type", content_type(file.path())))
            .boxed(),
        Route::DevBundle => Response::from_string(DEV_REFUSAL)
            .with_status_code(503)
            .boxed(),
        Route::NotFound => Response::empty(404).boxed(),
        Route::NotAllowed => Response::empty(405).boxed(),
    };
    let _ = request.respond(response);
}

/// The cursor a request resumes from: one past the `seq` of its
/// `Last-Event-ID` when that id is this process's — and 0, the replay,
/// for a connection with none, with another boot's, or with one that
/// does not read as `<boot>.<seq>`.
fn cursor(request: &Request) -> u64 {
    let last = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Last-Event-ID"))
        .map(|header| header.value.as_str());
    resume(last, *BOOT)
}

/// [`cursor`] over the header's value and the boot to match.
fn resume(last: Option<&str>, boot: u64) -> u64 {
    last.and_then(|id| id.split_once('.'))
        .filter(|(seen, _)| seen.parse() == Ok(boot))
        .and_then(|(_, seq)| seq.parse::<u64>().ok())
        .map_or(0, |seq| seq.saturating_add(1))
}

/// One entry as an event: `<boot>.<seq>` as the id the browser sends
/// back, its JSON as the data.
fn event(entry: &Entry, boot: u64) -> String {
    let data = serde_json::to_string(entry).expect("an entry is JSON");
    format!("id: {boot}.{}\ndata: {data}\n\n", entry.seq)
}

/// The event stream: every entry from the cursor on, for as long as the
/// client stays, and a comment while nothing happens. The body has no
/// length and no framing — it ends when the connection does — which is
/// why it is written raw: tiny_http's `Response` would buffer an event
/// until the next one filled the buffer.
fn stream(request: Request, log: &Log) {
    let mut cursor = cursor(&request);
    let mut writer = request.into_writer();
    let mut send = |text: &str| {
        writer
            .write_all(text.as_bytes())
            .and_then(|()| writer.flush())
    };
    let head = "HTTP/1.1 200 OK\r\n\
                Content-Type: text/event-stream\r\n\
                Cache-Control: no-cache\r\n\
                Connection: close\r\n\r\n";
    if send(head).is_err() {
        return;
    }
    loop {
        let entries = log.wait_after(cursor, KEEP_ALIVE);
        let text = match entries.last() {
            None => ": keep-alive\n\n".to_owned(),
            Some(last) => {
                cursor = last.seq + 1;
                entries.iter().map(|entry| event(entry, *BOOT)).collect()
            }
        };
        if send(&text).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read};
    use std::net::TcpStream;

    use epik::feature::{IssueId, RunId};
    use epik::monitor::Change;

    use super::*;

    fn reserved(run: u64) -> Change {
        Change::Reserved {
            run: RunId(run),
            feature: IssueId::from(1),
            repository: "/r".to_owned(),
            branch: "feature-1".to_owned(),
            base: None,
        }
    }

    /// A server over a fresh log, and where it is.
    fn served() -> (Arc<Log>, SocketAddr) {
        let log = Arc::new(Log::new());
        let addr = serve("127.0.0.1:0".parse().unwrap(), Arc::clone(&log)).unwrap();
        (log, addr)
    }

    /// A raw request, answered: the status, the headers lowercased, and
    /// the connection positioned at the body.
    fn request(
        addr: SocketAddr,
        line: &str,
        headers: &[&str],
    ) -> (u16, Vec<String>, BufReader<TcpStream>) {
        request_from(addr, Some(&addr.to_string()), line, headers)
    }

    /// [`request`], naming `host` — or no `Host` at all.
    fn request_from(
        addr: SocketAddr,
        host: Option<&str>,
        line: &str,
        headers: &[&str],
    ) -> (u16, Vec<String>, BufReader<TcpStream>) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        write!(stream, "{line} HTTP/1.1\r\n").unwrap();
        if let Some(host) = host {
            write!(stream, "Host: {host}\r\n").unwrap();
        }
        for header in headers {
            write!(stream, "{header}\r\n").unwrap();
        }
        write!(stream, "\r\n").unwrap();
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status).unwrap();
        let status = status.split(' ').nth(1).unwrap().parse().unwrap();
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            headers.push(line.trim_end().to_lowercase());
        }
        (status, headers, reader)
    }

    /// The next event on the stream — its id as `(boot, seq)` and its
    /// entry — comment lines passed over.
    fn next_event(reader: &mut BufReader<TcpStream>) -> ((u64, u64), Entry) {
        let (mut id, mut data) = (None, None);
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0, "the stream ended");
            match line.trim_end_matches('\n') {
                "" => {
                    if let (Some(id), Some(data)) = (id, data.take()) {
                        return (id, data);
                    }
                }
                comment if comment.starts_with(':') => {}
                field => match field.split_once(": ").unwrap() {
                    ("id", value) => {
                        let (boot, seq) = value.split_once('.').unwrap();
                        id = Some((boot.parse().unwrap(), seq.parse().unwrap()));
                    }
                    ("data", value) => data = Some(serde_json::from_str(value).unwrap()),
                    (name, _) => panic!("an unexpected field {name}"),
                },
            }
        }
    }

    #[test]
    fn the_stream_replays_in_seq_order_with_seq_as_the_id_then_carries_on_live() {
        let (log, addr) = served();
        log.record(reserved(1));
        log.record(reserved(2));
        log.record(reserved(3));

        let (status, headers, mut reader) = request(addr, &format!("GET {CHANGES}"), &[]);
        assert_eq!(status, 200);
        assert!(headers.contains(&"content-type: text/event-stream".to_owned()));
        assert!(headers.contains(&"cache-control: no-cache".to_owned()));
        for (seq, expected) in log.since(0).iter().enumerate() {
            let (id, entry) = next_event(&mut reader);
            assert_eq!(id, (*BOOT, seq as u64));
            assert_eq!(entry, *expected);
        }

        log.record(Change::Finished { run: RunId(1) });
        let (id, entry) = next_event(&mut reader);
        assert_eq!(
            id,
            (*BOOT, 3),
            "recorded after the connection opened, and heard"
        );
        assert_eq!(entry.change, Change::Finished { run: RunId(1) });
    }

    #[test]
    fn a_reconnecting_client_resumes_after_its_last_event_id_of_this_boot() {
        let (log, addr) = served();
        log.record(reserved(1));
        log.record(reserved(2));
        log.record(reserved(3));

        let (status, _, mut reader) = request(
            addr,
            &format!("GET {CHANGES}"),
            &[&format!("Last-Event-ID: {}.1", *BOOT)],
        );
        assert_eq!(status, 200);
        let (id, entry) = next_event(&mut reader);
        assert_eq!(id, (*BOOT, 2), "only what follows");
        assert_eq!(entry, log.since(2)[0]);

        log.record(reserved(4));
        assert_eq!(next_event(&mut reader).0, (*BOOT, 3));
    }

    /// The id a page kept from a backend that has since been relaunched
    /// indexes a log that no longer exists: it replays from 0.
    #[test]
    fn a_last_event_id_from_another_boot_replays_from_the_start() {
        let (log, addr) = served();
        log.record(reserved(1));
        log.record(reserved(2));

        let (status, _, mut reader) = request(
            addr,
            &format!("GET {CHANGES}"),
            &["Last-Event-ID: 12345.40"],
        );
        assert_eq!(status, 200);
        assert_eq!(next_event(&mut reader).0, (*BOOT, 0));
        assert_eq!(next_event(&mut reader).0, (*BOOT, 1));
    }

    #[test]
    fn the_cursor_is_one_past_this_boots_seq_and_zero_otherwise() {
        assert_eq!(resume(Some("7.41"), 7), 42);
        assert_eq!(resume(Some("8.41"), 7), 0, "another boot");
        assert_eq!(resume(Some("41"), 7), 0, "no boot at all");
        assert_eq!(resume(Some("7.forty"), 7), 0);
        assert_eq!(resume(None, 7), 0);
    }

    #[test]
    fn anything_but_a_get_is_refused_and_an_unknown_path_is_not_found() {
        let (_log, addr) = served();
        assert_eq!(request(addr, &format!("POST {CHANGES}"), &[]).0, 405);
        assert_eq!(request(addr, "GET /nothing-here", &[]).0, 404);
    }

    /// The page is served when the build embedded a bundle, a 404 says
    /// so when it did not, and a 503 when what it embedded was `trunk
    /// serve`'s — all three are this crate compiling.
    #[test]
    fn the_page_is_the_bundles_index() {
        let (_log, addr) = served();
        let (status, headers, mut reader) = request(addr, "GET /", &[]);
        match BUNDLE.get_file("index.html") {
            None => assert_eq!(status, 404, "no bundle was embedded"),
            Some(_) if *DEV_BUNDLE => assert_eq!(status, 503, "a dev bundle was embedded"),
            Some(index) => {
                assert_eq!(status, 200);
                assert!(headers.contains(&"content-type: text/html; charset=utf-8".to_owned()));
                let mut body = vec![0; index.contents().len()];
                reader.read_exact(&mut body).unwrap();
                assert_eq!(body, index.contents());
            }
        }
    }

    /// `localhost` is the one name tried: it resolves from the hosts
    /// file, so no test here touches DNS.
    #[test]
    fn a_loopback_address_or_name_is_accepted_and_anything_else_refused() {
        assert_eq!(
            decide("127.0.0.1:7878").unwrap(),
            "127.0.0.1:7878".parse::<SocketAddr>().unwrap()
        );
        assert!(decide("[::1]:7878").is_ok());
        let local = decide("localhost:0").unwrap();
        assert!(local.ip().is_loopback(), "{local}");
        assert_eq!(local.port(), 0);
        let refused = decide("0.0.0.0:7878").unwrap_err();
        assert!(refused.contains("ssh tunnel"), "{refused}");
        assert!(refused.contains("0.0.0.0:7878"), "{refused}");
        let refused = decide("10.0.0.5:7878").unwrap_err();
        assert!(refused.contains("loopback"), "{refused}");
        let garbage = decide("localhost").unwrap_err();
        assert!(garbage.contains("host:port"), "{garbage}");
    }

    /// Every name this server goes by is admitted, and any other — a
    /// rebound domain, or no name at all — is not.
    #[test]
    fn only_a_host_naming_this_server_is_admitted() {
        let bound: IpAddr = "127.0.0.1".parse().unwrap();
        for host in [
            "localhost",
            "localhost:7878",
            "LOCALHOST:7878",
            "127.0.0.1",
            "127.0.0.1:7878",
            "[::1]",
            "[::1]:7878",
            "::1",
        ] {
            assert!(admitted(Some(host), bound), "{host}");
        }
        assert!(admitted(Some("[::1]:7878"), "::1".parse().unwrap()));
        for host in [
            "attacker.example",
            "attacker.example:7878",
            "10.0.0.5:7878",
            "",
        ] {
            assert!(!admitted(Some(host), bound), "{host}");
        }
        assert!(!admitted(None, bound));
    }

    #[test]
    fn a_request_from_another_host_is_refused_with_421() {
        let (log, addr) = served();
        log.record(reserved(1));
        assert_eq!(
            request_from(
                addr,
                Some("attacker.example"),
                &format!("GET {CHANGES}"),
                &[]
            )
            .0,
            421
        );
        assert_eq!(request_from(addr, None, "GET /", &[]).0, 421);
        assert_eq!(
            request_from(
                addr,
                Some(&format!("localhost:{}", addr.port())),
                "GET /nothing",
                &[]
            )
            .0,
            404,
            "localhost is this server"
        );
    }

    #[test]
    fn the_routes() {
        assert!(route(&Method::Get, CHANGES) == Route::Changes);
        assert!(route(&Method::Get, &format!("{CHANGES}?x=1")) == Route::Changes);
        assert!(route(&Method::Post, CHANGES) == Route::NotAllowed);
        assert!(route(&Method::Get, "/nothing-here") == Route::NotFound);
        assert!(route(&Method::Get, "/../Cargo.toml") == Route::NotFound);
        assert!(route(&Method::Get, "/") == route(&Method::Get, "/index.html"));
    }

    #[test]
    fn a_dev_server_bundle_is_told_by_its_placeholder() {
        assert!(dev_bundle(
            b"<script>window.__TRUNK_ADDRESS__ = '__trunk_address__'</script>"
        ));
        assert!(!dev_bundle(
            b"<html><body><script>fetch('/monitor/changes')</script>"
        ));
        assert!(!dev_bundle(b""));
    }

    #[test]
    fn content_types_follow_the_extension() {
        assert_eq!(content_type(Path::new("a.wasm")), "application/wasm");
        assert_eq!(content_type(Path::new("a.js")), "text/javascript");
        assert_eq!(content_type(Path::new("a.css")), "text/css");
        assert_eq!(content_type(Path::new("a")), "application/octet-stream");
    }

    #[test]
    fn an_event_carries_the_boot_and_the_seq_as_its_id() {
        let entry = Entry {
            seq: 7,
            at: 1,
            change: Change::Finished { run: RunId(1) },
        };
        let text = event(&entry, 5);
        assert!(text.starts_with("id: 5.7\ndata: {"), "{text}");
        assert!(text.ends_with("}\n\n"), "{text}");
    }
}
