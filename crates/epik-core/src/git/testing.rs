//! The fixtures the live git tests share: a repository with an
//! identity and a commit in it, made through the verbs above.

use super::commands::{Add, Clone, Commit, Hash, Init, Push, Remote, RevParse, SetConfig};
use super::url::Url;
use super::{Repository, add, clone, commit, init, push, rev_parse, set_config};
use crate::temp::Scratch;

/// The remote a clone names its source.
pub fn origin() -> Remote {
    Remote::named("origin".to_owned()).unwrap()
}

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
    push(Push::new(work.clone(), self::origin(), "main".to_owned()).unwrap()).unwrap();
    (origin, work)
}
