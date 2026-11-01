//! A copy of a remote host's run records, kept current by asking its app.
//!
//! The mirror of a host has the layout of `~/.epik`, so its records are at
//! `<mirror>/runs/<owner>/<name>/<number>/…`, and the code that reads this
//! machine's runs reads them. The mirror is a cache: it only ever copies,
//! and the host owns the runs. The decision record
//! `docs/design-history/2026-10-31-remote-hosts.md` says why.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use crate::ipc::{Chunk, RecordFile, Request, Response};
use crate::remote::Client;
use crate::run::{self, RunKey};

/// Brings the records under `mirror_root` up to date with the host's that
/// `client` reaches. Each of the host's record files is read from the size
/// its copy already has, and appended to it. A copy longer than the host's
/// file was rewritten there, and is started over. A run the host no longer
/// has is removed. Nothing is written outside `mirror_root`.
pub fn sync(client: &impl Client, mirror_root: &Path) -> io::Result<()> {
    let runs = match client.request(&Request::Runs)? {
        Response::RunKeys { runs } => runs,
        other => return Err(unexpected("runs", other)),
    };
    let kept: BTreeSet<RunKey> = runs.into_iter().map(|latest| latest.run).collect();
    for key in run::keys(mirror_root)? {
        if !kept.contains(&key) {
            run::delete(mirror_root, &key)?;
        }
    }
    for key in &kept {
        sync_run(client, mirror_root, key)?;
    }
    Ok(())
}

/// Brings the copy of each of `key`'s record files up to date.
fn sync_run(client: &impl Client, mirror_root: &Path, key: &RunKey) -> io::Result<()> {
    let files = match client.request(&Request::Files { run: key.clone() })? {
        Response::Files { files, .. } => files,
        other => return Err(unexpected("files", other)),
    };
    let dir = run::run_dir(mirror_root, key);
    for file in files {
        let copy = dir.join(within(&file.path)?);
        sync_file(client, key, &file, &copy)?;
    }
    Ok(())
}

/// Appends to `copy` what has been appended to `file` since, or starts it
/// over if `file` is shorter.
fn sync_file(client: &impl Client, key: &RunKey, file: &RecordFile, copy: &Path) -> io::Result<()> {
    let had = match fs::metadata(copy) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let mut offset = if file.size < had { 0 } else { had };
    if let Some(parent) = copy.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = OpenOptions::new()
        .create(true)
        .append(true)
        .truncate(false)
        .open(copy)?;
    if offset < had {
        out.set_len(0)?;
    }
    while offset < file.size {
        let request = Request::Read {
            run: key.clone(),
            path: file.path.clone(),
            offset,
        };
        let Chunk {
            data, end, size, ..
        } = match client.request(&request)? {
            Response::Chunk(chunk) => chunk,
            other => return Err(unexpected("read", other)),
        };
        // Nothing but a line still being written.
        if end <= offset {
            break;
        }
        out.write_all(data.as_bytes())?;
        offset = end;
        if offset >= size {
            break;
        }
    }
    Ok(())
}

/// `path`, a record file's path as the host gives it, as a relative path
/// that stays in the run's directory.
fn within(path: &str) -> io::Result<PathBuf> {
    let relative = Path::new(path);
    let stays = relative
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    let names = relative
        .components()
        .any(|part| matches!(part, Component::Normal(_)));
    if stays && names {
        Ok(relative.to_path_buf())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{path:?} is not a path in a run's directory"),
        ))
    }
}

/// The error for `reply`, which isn't the answer to a `request` request.
fn unexpected(request: &str, reply: Response) -> io::Error {
    match reply {
        Response::Error { message } => io::Error::other(message),
        other => io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the host answered {request} with {other:?}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use chrono::TimeZone;
    use tempfile::TempDir;

    use super::*;
    use crate::ipc;
    use crate::record::{self, BuildWriter, Event, Kind};

    /// A host's app, answering from the records under its own root.
    struct Host {
        root: TempDir,
        /// Every request it has been sent.
        requests: Mutex<Vec<Request>>,
    }

    impl Host {
        fn new() -> Self {
            Self {
                root: TempDir::new().unwrap(),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn root(&self) -> &Path {
            self.root.path()
        }

        /// Records a build of issue `number`, and returns its directory.
        fn build(&self, number: u64) -> PathBuf {
            let at = chrono::Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap()
                + chrono::Duration::seconds(i64::try_from(number).unwrap());
            let key = key(number);
            let mut writer = BuildWriter::create_new(self.root(), &key, at).unwrap();
            writer
                .append(&Event::BuildStarted {
                    at,
                    build_id: writer.build_id().to_owned(),
                    repo: "owner/repo".to_owned(),
                    number,
                    title: format!("Issue {number}"),
                    kind: Kind::Issue,
                    branch: "main".to_owned(),
                    issues: Vec::new(),
                    closed: Vec::new(),
                })
                .unwrap();
            record::build_dir(self.root(), &key, writer.build_id())
        }

        fn reads(&self) -> Vec<(String, u64)> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter_map(|request| match request {
                    Request::Read { path, offset, .. } => Some((path.clone(), *offset)),
                    _ => None,
                })
                .collect()
        }
    }

    impl Client for Host {
        fn request(&self, request: &Request) -> io::Result<Response> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(ipc::records(self.root(), request.clone()))
        }
    }

    fn key(number: u64) -> RunKey {
        RunKey::new("owner/repo", number)
    }

    fn append(path: &Path, text: &str) {
        OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    /// Every file under `root`'s run records, by its path from them.
    fn records(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(base: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(base, &path, files);
                } else {
                    let relative = path.strip_prefix(base).unwrap().to_path_buf();
                    files.insert(relative, fs::read(&path).unwrap());
                }
            }
        }
        let base = record::runs_dir(root);
        let mut files = BTreeMap::new();
        walk(&base, &base, &mut files);
        files
    }

    #[test]
    fn a_new_run_is_copied_where_the_runs_are_read() {
        let host = Host::new();
        let build = host.build(3);
        append(&build.join("3.jsonl"), "{\"one\":1}\n{\"two\":2}\n");
        let mirror = TempDir::new().unwrap();

        sync(&host, mirror.path()).unwrap();

        assert_eq!(records(mirror.path()), records(host.root()));
        let runs = run::list(mirror.path()).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run, key(3));
    }

    #[test]
    fn a_growing_file_is_extended_from_what_the_mirror_has() {
        let host = Host::new();
        let log = host.build(3).join("3.jsonl");
        append(&log, "{\"one\":1}\n");
        let mirror = TempDir::new().unwrap();
        sync(&host, mirror.path()).unwrap();
        let had = fs::metadata(&log).unwrap().len();

        append(&log, "{\"two\":2}\n{\"three\":3}\n");
        sync(&host, mirror.path()).unwrap();

        assert_eq!(records(mirror.path()), records(host.root()));
        let reads = host.reads();
        let log_reads: Vec<u64> = reads
            .iter()
            .filter(|(path, _)| path.ends_with("/3.jsonl"))
            .map(|(_, offset)| *offset)
            .collect();
        assert_eq!(log_reads, [0, had]);
    }

    #[test]
    fn a_line_still_being_written_waits_for_its_end() {
        let host = Host::new();
        let log = host.build(3).join("3.jsonl");
        append(&log, "{\"one\":1}\n{\"tw");
        let mirror = TempDir::new().unwrap();

        sync(&host, mirror.path()).unwrap();
        let copy = run::run_dir(mirror.path(), &key(3)).join(
            log.strip_prefix(run::run_dir(host.root(), &key(3)))
                .unwrap(),
        );
        assert_eq!(fs::read_to_string(&copy).unwrap(), "{\"one\":1}\n");

        append(&log, "o\":2}\n");
        sync(&host, mirror.path()).unwrap();
        assert_eq!(records(mirror.path()), records(host.root()));
    }

    #[test]
    fn a_rewritten_file_is_started_over() {
        let host = Host::new();
        let log = host.build(3).join("3.jsonl");
        append(&log, "{\"one\":1}\n{\"two\":2}\n");
        let mirror = TempDir::new().unwrap();
        sync(&host, mirror.path()).unwrap();

        fs::write(&log, "{\"new\":1}\n").unwrap();
        sync(&host, mirror.path()).unwrap();

        assert_eq!(records(mirror.path()), records(host.root()));
    }

    #[test]
    fn a_run_the_host_no_longer_has_is_removed() {
        let host = Host::new();
        host.build(3);
        host.build(4);
        let mirror = TempDir::new().unwrap();
        sync(&host, mirror.path()).unwrap();

        run::delete(host.root(), &key(3)).unwrap();
        sync(&host, mirror.path()).unwrap();

        assert!(!run::run_dir(mirror.path(), &key(3)).exists());
        assert_eq!(run::keys(mirror.path()).unwrap(), [key(4)]);
        assert_eq!(records(mirror.path()), records(host.root()));
    }

    #[test]
    fn a_run_with_more_than_twenty_beside_it_is_kept() {
        let host = Host::new();
        let count = ipc::DEFAULT_LIMIT as u64 + 5;
        for number in 1..=count {
            host.build(number);
        }
        let mirror = TempDir::new().unwrap();

        sync(&host, mirror.path()).unwrap();
        sync(&host, mirror.path()).unwrap();

        assert_eq!(run::list(mirror.path()).unwrap().len(), count as usize);
        assert!(run::run_dir(mirror.path(), &key(1)).is_dir());
    }

    /// A host that lists one file whose path climbs out of its run.
    struct Climbing;

    impl Client for Climbing {
        fn request(&self, request: &Request) -> io::Result<Response> {
            Ok(match request {
                Request::Runs => Response::RunKeys {
                    runs: vec![ipc::RunLatest {
                        run: key(3),
                        latest_at: chrono::Utc::now(),
                    }],
                },
                Request::Files { run } => Response::Files {
                    run: run.clone(),
                    files: vec![ipc::RecordFile {
                        path: "../../../../escaped".to_owned(),
                        size: 2,
                    }],
                },
                Request::Read { path, offset, .. } => Response::Chunk(ipc::Chunk {
                    path: path.clone(),
                    offset: *offset,
                    data: "x\n".to_owned(),
                    end: 2,
                    size: 2,
                }),
                other => panic!("unexpected {other:?}"),
            })
        }
    }

    #[test]
    fn a_path_outside_the_run_is_refused() {
        let outer = TempDir::new().unwrap();
        let mirror = outer.path().join("mirror");

        let error = sync(&Climbing, &mirror).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
        assert!(!outer.path().join("escaped").exists());
    }

    /// A host whose app answers every request with an error.
    struct Failing;

    impl Client for Failing {
        fn request(&self, _: &Request) -> io::Result<Response> {
            Ok(Response::Error {
                message: "no app is running".to_owned(),
            })
        }
    }

    #[test]
    fn a_hosts_error_is_the_syncs_and_leaves_the_mirror_alone() {
        let host = Host::new();
        host.build(3);
        let mirror = TempDir::new().unwrap();
        sync(&host, mirror.path()).unwrap();

        let error = sync(&Failing, mirror.path()).unwrap_err();

        assert!(error.to_string().contains("no app is running"), "{error}");
        assert_eq!(run::keys(mirror.path()).unwrap(), [key(3)]);
    }
}
