//! The fixtures the live git tests share: a repository with an
//! identity and a commit in it, made through the verbs above.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use super::askpass::Credentials;
use super::commands::{
    Add, Clone, Commit, Hash, Init, ListWorktrees, Push, Remote, RevParse, SetConfig,
};
use super::url::Url;
use super::{Repository, add, clone, commit, init, list_worktrees, push, rev_parse, set_config};
use crate::temp::Scratch;

/// The identity the tests' commits are made under, unsigned.
pub fn identified(repository: &Repository) {
    for (key, value) in [
        ("user.name", "Ada"),
        ("user.email", "ada@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        set_config(SetConfig::new(repository.clone(), key.to_owned(), value.to_owned()).unwrap())
            .unwrap();
    }
}

/// `content` written to `name` in `repository`, staged, and
/// committed under `subject`: the commit's hash.
pub fn committed(repository: &Repository, name: &str, content: &str, subject: &str) -> Hash {
    std::fs::write(repository.path().join(name), content).unwrap();
    add(Add::new(repository.clone(), vec![name.into()]).unwrap()).unwrap();
    commit(Commit::new(repository.clone(), subject.to_owned()).unwrap()).unwrap();
    head(repository)
}

pub fn head(repository: &Repository) -> Hash {
    rev_parse(RevParse::new(repository.clone(), "HEAD".to_owned()).unwrap()).unwrap()
}

/// `paths`, already written in `repository` by whoever, staged and
/// committed under `subject`: the commit's hash.
pub fn committed_all(repository: &Repository, paths: Vec<PathBuf>, subject: &str) -> Hash {
    add(Add::new(repository.clone(), paths).unwrap()).unwrap();
    commit(Commit::new(repository.clone(), subject.to_owned()).unwrap()).unwrap();
    head(repository)
}

/// The branch `repository` has checked out, as git lists it. Git lists
/// paths as it resolved them, symlinks followed, so the comparison is
/// between resolved paths.
pub fn branch_of(repository: &Repository) -> String {
    let path = repository.path().canonicalize().unwrap();
    list_worktrees(ListWorktrees::new(repository.clone()))
        .unwrap()
        .into_iter()
        .find(|worktree| {
            worktree
                .path
                .canonicalize()
                .is_ok_and(|listed| listed == path)
        })
        .and_then(|worktree| worktree.branch)
        .expect("a checked-out branch")
}

/// `branch` of `repository` pushed to its origin.
pub fn pushed(repository: &Repository, branch: &str) {
    push(Push::new(repository.clone(), Remote::origin(), branch.to_owned()).unwrap()).unwrap();
}

/// A clone of `origin` at `into` under `scratch`, on `branch` when
/// one is named.
pub fn cloned(
    scratch: &Scratch,
    origin: &Repository,
    into: &str,
    branch: Option<&str>,
) -> Repository {
    clone(
        Clone::new(
            Url::Local(origin.path().to_owned()),
            scratch.join(into).into(),
            branch.map(str::to_owned),
        )
        .unwrap(),
    )
    .unwrap()
}

/// A bare origin under `scratch`, and a clone of it with one commit
/// of `hello.txt` on `main`, pushed.
pub fn seeded(scratch: &Scratch) -> (Repository, Repository) {
    let origin = init(Init::new(scratch.join("origin.git").into()).unwrap()).unwrap();
    let work = cloned(scratch, &origin, "work", None);
    identified(&work);
    committed(&work, "hello.txt", "hello\n", "the first commit");
    push(Push::new(work.clone(), Remote::origin(), "main".to_owned()).unwrap()).unwrap();
    (origin, work)
}

/// The origin of `repository` moved to `url`.
pub fn aimed(repository: &Repository, url: String) {
    let key = "remote.origin.url".to_owned();
    set_config(SetConfig::new(repository.clone(), key, url).unwrap()).unwrap();
}

/// A GitHub token's credentials, for a remote to be answered with.
#[must_use]
pub fn credentials() -> Credentials {
    Credentials {
        username: "x-access-token".to_owned(),
        secret: "ghp_sesame".into(),
    }
}

/// What a remote that asks is sent of [`credentials`]: Base64 of
/// `x-access-token:ghp_sesame`.
pub const OFFERED: &str = "Basic eC1hY2Nlc3MtdG9rZW46Z2hwX3Nlc2FtZQ==";

/// A credential helper that answers every remote as an intruder.
pub const INTRUDER: &str = "!f() { echo username=intruder; echo password=hunter2; }; f";

/// What a remote that asks is sent when [`INTRUDER`] answers: Base64
/// of `intruder:hunter2`.
pub const INTRUDED: &str = "Basic aW50cnVkZXI6aHVudGVyMg==";

/// How long a test waits on what [`guarded`] reports: long enough for
/// git to ask, be refused, and ask again.
pub const PATIENCE: Duration = Duration::from_secs(30);

/// A loopback listener that answers every request with a demand
/// for Basic authentication, and reports the first `Authorization`
/// header it is sent.
pub fn guarded() -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/repo.git", listener.local_addr().unwrap());
    let (seen_in, seen) = channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let mut reader = BufReader::new(stream);
            // The headers end at the first empty line.
            let authorization = (&mut reader)
                .lines()
                .map_while(Result::ok)
                .take_while(|line| !line.is_empty())
                .filter_map(|line| line.strip_prefix("Authorization: ").map(str::to_owned))
                .last();
            let _ = reader.into_inner().write_all(
                b"HTTP/1.1 401 Unauthorized\r\n\
                  WWW-Authenticate: Basic realm=\"epik\"\r\n\
                  Content-Length: 0\r\nConnection: close\r\n\r\n",
            );
            if let Some(authorization) = authorization {
                let _ = seen_in.send(authorization);
                break;
            }
        }
    });
    (url, seen)
}
