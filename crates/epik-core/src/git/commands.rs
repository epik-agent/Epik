//! The git commands Epik speaks, as data.
//!
//! Each command is a struct holding exactly what git needs, checked at
//! construction — the one door in, whether the caller is library code
//! or a model's tool call arriving as JSON — and [`Command`] is the
//! list, in roughly the order a repository meets them. [`Command::argv`]
//! is the one place in the crate a command becomes a command line, so
//! every flag is pinned in one test. A command that reads something
//! back carries the reading beside it, a pure function over git's
//! output.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, FixedOffset};
use serde::Deserialize;

use super::Repository;
use super::url::Url;

/// Every git command Epik runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Init(Init),
    Clone(Clone),
    ShowPrefix(ShowPrefix),
    SetConfig(SetConfig),
    Branch(Branch),
    Switch(Switch),
    AddWorktree(AddWorktree),
    ListWorktrees(ListWorktrees),
    Add(Add),
    Status(Status),
    Diff(Diff),
    Commit(Commit),
    RevParse(RevParse),
    Log(Log),
    Show(Show),
    Fetch(Fetch),
    Pull(Pull),
    Merge(Merge),
    AbortMerge(AbortMerge),
    Reset(Reset),
    Push(Push),
    ListRemotes(ListRemotes),
    DeleteBranch(DeleteBranch),
    RemoveWorktree(RemoveWorktree),
}

impl Command {
    /// The command line after `git`: `-C` and the repository, for a
    /// verb that runs in one; the verb; its flags, each with its value
    /// attached by `=`; then the positionals — behind `--` where the
    /// verb takes paths, and refused at construction wherever one could
    /// read as a flag.
    #[must_use]
    pub fn argv(&self) -> Vec<String> {
        match self {
            Self::Init(init) => init.argv(),
            Self::Clone(clone) => clone.argv(),
            Self::ShowPrefix(prefix) => prefix.argv(),
            Self::SetConfig(config) => config.argv(),
            Self::Branch(branch) => branch.argv(),
            Self::Switch(switch) => switch.argv(),
            Self::AddWorktree(worktree) => worktree.argv(),
            Self::ListWorktrees(worktrees) => worktrees.argv(),
            Self::Add(add) => add.argv(),
            Self::Status(status) => status.argv(),
            Self::Diff(diff) => diff.argv(),
            Self::Commit(commit) => commit.argv(),
            Self::RevParse(rev_parse) => rev_parse.argv(),
            Self::Log(log) => log.argv(),
            Self::Show(show) => show.argv(),
            Self::Fetch(fetch) => fetch.argv(),
            Self::Pull(pull) => pull.argv(),
            Self::Merge(merge) => merge.argv(),
            Self::AbortMerge(abort) => abort.argv(),
            Self::Reset(reset) => reset.argv(),
            Self::Push(push) => push.argv(),
            Self::ListRemotes(remotes) => remotes.argv(),
            Self::DeleteBranch(branch) => branch.argv(),
            Self::RemoveWorktree(worktree) => worktree.argv(),
        }
    }
}

/// `-C`, the repository, and `verb`: how every command that runs in a
/// repository begins.
fn within(repository: &Repository, verb: &str) -> Vec<String> {
    vec![
        "-C".to_owned(),
        repository.path().to_string_lossy().into_owned(),
        verb.to_owned(),
    ]
}

/// Refuses a `value` that git would read as a flag: one beginning
/// with `-`.
fn plain(what: &str, value: &str) -> Result<()> {
    if value.starts_with('-') {
        bail!("a {what} may not begin with a dash: {value:?}");
    }
    Ok(())
}

/// Refuses a relative `path`: Epik has no working directory to resolve
/// one against.
fn absolute(what: &str, path: &Path) -> Result<()> {
    if path.is_relative() {
        bail!("{what} must be an absolute path: {}", path.display());
    }
    Ok(())
}

/// A commit's full hash: forty hex digits, or sixty-four in a SHA-256
/// repository. Made only from git's own words, so a hash in hand names
/// a commit git reported.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hash(String);

impl Hash {
    /// `text` as a hash.
    ///
    /// # Errors
    ///
    /// Anything but forty or sixty-four lowercase hex digits.
    pub fn new(text: &str) -> Result<Self> {
        let text = text.trim();
        let hex = text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !(hex && (text.len() == 40 || text.len() == 64)) {
            bail!("not a commit hash: {text:?}");
        }
        Ok(Self(text.to_owned()))
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A remote, as git takes one on a command line: a configured name
/// like `origin`, or a [`Url`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Remote(String);

impl Remote {
    /// The remote a clone names its source.
    #[must_use]
    pub fn origin() -> Self {
        Self("origin".to_owned())
    }

    /// The remote configured under `name`.
    ///
    /// # Errors
    ///
    /// A name beginning with `-`: git would read it as a flag.
    pub fn named(name: String) -> Result<Self> {
        plain("remote name", &name)?;
        Ok(Self(name))
    }
}

impl From<Url> for Remote {
    fn from(url: Url) -> Self {
        Self(url.to_string())
    }
}

impl fmt::Display for Remote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `git init`: a bare repository at `path`, on `main`, with no commits
/// yet — the origin a clone is made from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Init {
    path: PathBuf,
}

impl Init {
    /// A bare repository at `path`.
    ///
    /// # Errors
    ///
    /// Epik has no working directory to resolve against, so `path`
    /// must be absolute.
    pub fn new(path: PathBuf) -> Result<Self> {
        absolute("the repository", &path)?;
        Ok(Self { path })
    }

    /// Where the repository lands.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn argv(&self) -> Vec<String> {
        vec![
            "init".to_owned(),
            "--quiet".to_owned(),
            "--bare".to_owned(),
            "--initial-branch=main".to_owned(),
            "--".to_owned(),
            self.path.to_string_lossy().into_owned(),
        ]
    }
}

/// `git clone`: `from`, into a directory that does not exist yet.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(try_from = "SerializedClone")]
pub struct Clone {
    from: Url,
    into: PathBuf,
    branch: Option<String>,
}

/// A clone as serialized: the fields, on their way to [`Clone::new`].
#[derive(Deserialize)]
struct SerializedClone {
    from: Url,
    into: PathBuf,
    #[serde(default)]
    branch: Option<String>,
}

impl Clone {
    /// A clone of `from` into `into`, checked out at `branch` when one
    /// is named and at the remote's default otherwise.
    ///
    /// # Errors
    ///
    /// Epik has no working directory to resolve against, so `into` must
    /// be absolute, and so must a local `from`. A branch beginning with
    /// `-` is refused: git would read it as a flag.
    pub fn new(from: Url, into: PathBuf, branch: Option<String>) -> Result<Self> {
        if let Url::Local(path) = &from {
            absolute("a local source", path)?;
        }
        absolute("the destination", &into)?;
        if let Some(branch) = &branch {
            plain("branch", branch)?;
        }
        Ok(Self { from, into, branch })
    }

    /// Where the clone lands.
    #[must_use]
    pub fn destination(&self) -> &Path {
        &self.into
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = vec!["clone".to_owned()];
        if let Some(branch) = &self.branch {
            argv.push("--branch".to_owned());
            argv.push(branch.clone());
        }
        argv.push("--".to_owned());
        argv.push(self.from.to_string());
        argv.push(self.into.to_string_lossy().into_owned());
        argv
    }
}

/// The step deserializing takes after reading the fields, so a clone
/// arriving as JSON goes through [`Clone::new`] like any other.
impl TryFrom<SerializedClone> for Clone {
    type Error = anyhow::Error;

    fn try_from(serialized: SerializedClone) -> Result<Self> {
        Self::new(serialized.from, serialized.into, serialized.branch)
    }
}

/// `git rev-parse --show-prefix`: where `path` sits in its repository,
/// as the path below the root — empty at the root of a working tree,
/// bare or linked, so the reading is what makes a path a
/// [`Repository`]. The one command that runs in a path rather than a
/// repository, being how a path becomes one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShowPrefix {
    path: PathBuf,
}

impl ShowPrefix {
    /// The prefix of `path`.
    ///
    /// # Errors
    ///
    /// Epik has no working directory to resolve against, so `path`
    /// must be absolute.
    pub fn new(path: PathBuf) -> Result<Self> {
        absolute("the repository", &path)?;
        Ok(Self { path })
    }

    /// The path asked about.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The prefix in `stdout`, as [`ShowPrefix::argv`] asked for it:
    /// one line, empty at the root.
    #[must_use]
    pub fn prefix(stdout: &str) -> &str {
        stdout.trim()
    }

    fn argv(&self) -> Vec<String> {
        vec![
            "-C".to_owned(),
            self.path.to_string_lossy().into_owned(),
            "rev-parse".to_owned(),
            "--show-prefix".to_owned(),
        ]
    }
}

/// `git config`: `key` set to `value` in `repository`'s own config —
/// the identity its commits are made under, chiefly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetConfig {
    repository: Repository,
    key: String,
    value: String,
}

impl SetConfig {
    /// `key` set to `value` in `repository`.
    ///
    /// # Errors
    ///
    /// A key beginning with `-`: git would read it as a flag.
    pub fn new(repository: Repository, key: String, value: String) -> Result<Self> {
        plain("config key", &key)?;
        Ok(Self {
            repository,
            key,
            value,
        })
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "config");
        argv.extend([
            "--local".to_owned(),
            "--".to_owned(),
            self.key.clone(),
            self.value.clone(),
        ]);
        argv
    }
}

/// `git branch`: `name` made in `repository`, at `at` when a commit is
/// given and at `HEAD` otherwise.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Branch {
    repository: Repository,
    name: String,
    at: Option<Hash>,
}

impl Branch {
    /// The branch `name`, starting at `at` or `HEAD`.
    ///
    /// # Errors
    ///
    /// A name beginning with `-`: git would read it as a flag.
    pub fn new(repository: Repository, name: String, at: Option<Hash>) -> Result<Self> {
        plain("branch", &name)?;
        Ok(Self {
            repository,
            name,
            at,
        })
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "branch");
        argv.push(self.name.clone());
        if let Some(at) = &self.at {
            argv.push(at.to_string());
        }
        argv
    }
}

/// `git switch`: `branch` checked out in `repository`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Switch {
    repository: Repository,
    branch: String,
}

impl Switch {
    /// A switch to `branch`, which exists.
    ///
    /// # Errors
    ///
    /// A branch beginning with `-`: git would read it as a flag.
    pub fn new(repository: Repository, branch: String) -> Result<Self> {
        plain("branch", &branch)?;
        Ok(Self { repository, branch })
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "switch");
        argv.push(self.branch.clone());
        argv
    }
}

/// `git worktree add`: `branch`, which exists, checked out at `linked`,
/// a directory that does not exist yet, sharing `main`'s history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AddWorktree {
    main: Repository,
    linked: PathBuf,
    branch: String,
}

impl AddWorktree {
    /// A linked worktree of `main` at `linked`, on `branch`.
    ///
    /// # Errors
    ///
    /// A relative `linked`, or a branch beginning with `-`.
    pub fn new(main: Repository, linked: PathBuf, branch: String) -> Result<Self> {
        absolute("the worktree", &linked)?;
        plain("branch", &branch)?;
        Ok(Self {
            main,
            linked,
            branch,
        })
    }

    /// Where the linked worktree lands.
    #[must_use]
    pub fn linked(&self) -> &Path {
        &self.linked
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.main, "worktree");
        argv.extend([
            "add".to_owned(),
            "--".to_owned(),
            self.linked.to_string_lossy().into_owned(),
            self.branch.clone(),
        ]);
        argv
    }
}

/// `git worktree list`: every worktree of `repository`, which may be
/// any of them, the main one first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListWorktrees {
    repository: Repository,
}

/// One worktree as `git worktree list` reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    /// The commit checked out; none in a bare repository.
    pub head: Option<Hash>,
    /// The branch checked out; none when bare or detached.
    pub branch: Option<String>,
    pub bare: bool,
}

impl ListWorktrees {
    #[must_use]
    pub const fn new(repository: Repository) -> Self {
        Self { repository }
    }

    /// The worktrees in `stdout`, as [`ListWorktrees::argv`] asked for
    /// them: one paragraph each, a `key value` line per fact.
    ///
    /// # Errors
    ///
    /// A paragraph without a `worktree` line, or a `HEAD` that is not a
    /// hash — neither of which git produces.
    pub fn worktrees(stdout: &str) -> Result<Vec<WorktreeInfo>> {
        stdout
            .split("\n\n")
            .filter(|paragraph| !paragraph.trim().is_empty())
            .map(|paragraph| {
                let mut worktree = WorktreeInfo {
                    path: PathBuf::new(),
                    head: None,
                    branch: None,
                    bare: false,
                };
                let mut named = false;
                for line in paragraph.lines() {
                    match line.split_once(' ').unwrap_or((line, "")) {
                        ("worktree", path) => {
                            worktree.path = PathBuf::from(path);
                            named = true;
                        }
                        ("HEAD", hash) => worktree.head = Some(Hash::new(hash)?),
                        ("branch", reference) => {
                            let name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
                            worktree.branch = Some(name.to_owned());
                        }
                        ("bare", _) => worktree.bare = true,
                        _ => {}
                    }
                }
                if !named {
                    bail!("not a worktree: {paragraph:?}");
                }
                Ok(worktree)
            })
            .collect()
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "worktree");
        argv.extend(["list".to_owned(), "--porcelain".to_owned()]);
        argv
    }
}

/// `git add`: `paths` in `repository`, staged as they stand — each
/// relative to the repository's root, or absolute and inside it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Add {
    repository: Repository,
    paths: Vec<PathBuf>,
}

impl Add {
    /// Stages `paths` in `repository`.
    ///
    /// # Errors
    ///
    /// No paths, or a path beginning with `:`, which git reads as
    /// pathspec magic rather than a name.
    pub fn new(repository: Repository, paths: Vec<PathBuf>) -> Result<Self> {
        if paths.is_empty() {
            bail!("an add needs at least one path");
        }
        if let Some(path) = paths
            .iter()
            .find(|path| path.to_string_lossy().starts_with(':'))
        {
            bail!("a path may not begin with a colon: {}", path.display());
        }
        Ok(Self { repository, paths })
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "add");
        argv.push("--".to_owned());
        argv.extend(
            self.paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned()),
        );
        argv
    }
}

/// `git status`: every path in `repository` that differs from `HEAD`,
/// in the index or the working tree, untracked ones included. An empty
/// answer is a clean tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Status {
    repository: Repository,
}

/// One path as `git status` reports it: its state in the index and in
/// the working tree, each git's one-letter code — `M` modified, `A`
/// added, `D` deleted, `R` renamed, `U` unmerged, `?` untracked, a
/// space for unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Change {
    pub staged: char,
    pub unstaged: char,
    pub path: PathBuf,
    /// Where a renamed or copied path came from.
    pub from: Option<PathBuf>,
}

impl Status {
    #[must_use]
    pub const fn new(repository: Repository) -> Self {
        Self { repository }
    }

    /// The changes in `stdout`, as [`Status::argv`] asked for them: NUL
    /// after every path, and a renamed path followed by its source.
    ///
    /// # Errors
    ///
    /// An entry shorter than its two codes and a space, or a rename
    /// without its source — neither of which git produces.
    pub fn changes(stdout: &str) -> Result<Vec<Change>> {
        let mut fields = stdout.split('\0').filter(|field| !field.is_empty());
        let mut changes = Vec::new();
        while let Some(entry) = fields.next() {
            let mut codes = entry.chars();
            let (Some(staged), Some(unstaged), Some(' ')) =
                (codes.next(), codes.next(), codes.next())
            else {
                bail!("not a status entry: {entry:?}");
            };
            let from = if matches!(staged, 'R' | 'C') {
                let Some(source) = fields.next() else {
                    bail!("a rename without its source: {entry:?}");
                };
                Some(PathBuf::from(source))
            } else {
                None
            };
            changes.push(Change {
                staged,
                unstaged,
                path: PathBuf::from(codes.as_str()),
                from,
            });
        }
        Ok(changes)
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "status");
        argv.extend(["--porcelain=v1".to_owned(), "-z".to_owned()]);
        argv
    }
}

/// `git diff`: the patch in `repository` between `revisions` — one
/// revision, or a range like `main..topic`, or none for the working
/// tree against the index — narrowed to `path` when one is named.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diff {
    repository: Repository,
    revisions: Option<String>,
    path: Option<PathBuf>,
}

impl Diff {
    /// The patch across `revisions`, under `path`.
    ///
    /// # Errors
    ///
    /// Revisions beginning with `-`: git would read them as a flag.
    pub fn new(
        repository: Repository,
        revisions: Option<String>,
        path: Option<PathBuf>,
    ) -> Result<Self> {
        if let Some(revisions) = &revisions {
            plain("revision", revisions)?;
        }
        Ok(Self {
            repository,
            revisions,
            path,
        })
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "diff");
        if let Some(revisions) = &self.revisions {
            argv.push(revisions.clone());
        }
        argv.push("--".to_owned());
        if let Some(path) = &self.path {
            argv.push(path.to_string_lossy().into_owned());
        }
        argv
    }
}

/// `git commit`: what is staged in `repository`, under `message`, by
/// whatever identity the repository's config gives it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Commit {
    repository: Repository,
    message: String,
}

impl Commit {
    /// A commit of what is staged in `repository`, under `message`.
    ///
    /// # Errors
    ///
    /// A message that is empty, or only whitespace: git would refuse
    /// it, and this says so first.
    pub fn new(repository: Repository, message: String) -> Result<Self> {
        if message.trim().is_empty() {
            bail!("a commit needs a message");
        }
        Ok(Self {
            repository,
            message,
        })
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "commit");
        argv.push(format!("--message={}", self.message));
        argv
    }
}

/// `git rev-parse`: the commit `revision` names in `repository` — a
/// branch, a tag, `HEAD`, a hash or the front of one — as a full
/// [`Hash`](struct@Hash). A revision naming anything else fails.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevParse {
    repository: Repository,
    revision: String,
}

impl RevParse {
    /// The commit `revision` names.
    ///
    /// # Errors
    ///
    /// A revision beginning with `-`: git would read it as a flag.
    pub fn new(repository: Repository, revision: String) -> Result<Self> {
        plain("revision", &revision)?;
        Ok(Self {
            repository,
            revision,
        })
    }

    /// The revision asked about.
    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// The hash in `stdout`, as [`RevParse::argv`] asked for it: one
    /// line.
    ///
    /// # Errors
    ///
    /// [`Hash::new`]'s.
    pub fn hash(stdout: &str) -> Result<Hash> {
        Hash::new(stdout)
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "rev-parse");
        argv.extend([
            "--verify".to_owned(),
            format!("{}^{{commit}}", self.revision),
        ]);
        argv
    }
}

/// `git log`: the history behind `revision` in `repository` — `HEAD`
/// when none is named — newest first, at most `count` entries when a
/// count is given.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Log {
    repository: Repository,
    revision: Option<String>,
    count: Option<usize>,
}

/// One commit as `git log` reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub hash: Hash,
    /// The author's name.
    pub author: String,
    /// The author date, in the author's own offset.
    pub date: DateTime<FixedOffset>,
    /// The first line of the message.
    pub subject: String,
}

impl Log {
    /// The format each entry is asked for in: the four fields, NUL
    /// between them, which git keeps out of every one of them. The
    /// date is git's strict ISO 8601, which is RFC 3339.
    const FORMAT: &str = "--format=%H%x00%an%x00%aI%x00%s";

    /// The history behind `revision` in `repository`, at most `count`
    /// entries.
    ///
    /// # Errors
    ///
    /// A revision beginning with `-`: git would read it as a flag.
    pub fn new(
        repository: Repository,
        revision: Option<String>,
        count: Option<usize>,
    ) -> Result<Self> {
        if let Some(revision) = &revision {
            plain("revision", revision)?;
        }
        Ok(Self {
            repository,
            revision,
            count,
        })
    }

    /// The entries in `stdout`, as [`Log::argv`] asked for them: one
    /// per line, newest first.
    ///
    /// # Errors
    ///
    /// A line that is not four NUL-separated fields with a hash first
    /// and an RFC 3339 date third, which git does not produce for this
    /// format.
    pub fn entries(stdout: &str) -> Result<Vec<Entry>> {
        stdout
            .lines()
            .map(|line| {
                let fields: Vec<&str> = line.split('\0').collect();
                let [hash, author, date, subject] = fields[..] else {
                    bail!("not a log entry: {line:?}");
                };
                Ok(Entry {
                    hash: Hash::new(hash)?,
                    author: author.to_owned(),
                    date: DateTime::parse_from_rfc3339(date)
                        .with_context(|| format!("not a log entry's date: {date:?}"))?,
                    subject: subject.to_owned(),
                })
            })
            .collect()
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "log");
        argv.push(Self::FORMAT.to_owned());
        if let Some(count) = self.count {
            argv.push(format!("--max-count={count}"));
        }
        if let Some(revision) = &self.revision {
            argv.push(revision.clone());
        }
        argv
    }
}

/// `git show`: the commit `revision` names in `repository`, as git
/// prints it — the header, the message, and the patch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Show {
    repository: Repository,
    revision: String,
}

impl Show {
    /// The commit `revision` names, shown.
    ///
    /// # Errors
    ///
    /// A revision beginning with `-`: git would read it as a flag.
    pub fn new(repository: Repository, revision: String) -> Result<Self> {
        plain("revision", &revision)?;
        Ok(Self {
            repository,
            revision,
        })
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "show");
        argv.push(self.revision.clone());
        argv
    }
}

/// `git fetch`: `remote`'s branches brought into `repository`, under
/// its remote-tracking names; nothing checked out changes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fetch {
    repository: Repository,
    remote: Remote,
}

impl Fetch {
    #[must_use]
    pub const fn new(repository: Repository, remote: Remote) -> Self {
        Self { repository, remote }
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "fetch");
        argv.push(self.remote.to_string());
        argv
    }
}

/// `git pull`: the checked-out branch of `repository` brought up to
/// its upstream, fast-forward only — a branch that has diverged is
/// left as it is, and git says so.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pull {
    repository: Repository,
}

impl Pull {
    #[must_use]
    pub const fn new(repository: Repository) -> Self {
        Self { repository }
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "pull");
        argv.push("--ff-only".to_owned());
        argv
    }
}

/// `git merge`: `branch` merged into the checked-out branch of
/// `repository`, always as a merge commit with git's own message. A
/// conflict fails, and leaves the repository mid-merge for
/// [`AbortMerge`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Merge {
    repository: Repository,
    branch: String,
}

impl Merge {
    /// A merge of `branch`.
    ///
    /// # Errors
    ///
    /// A branch beginning with `-`: git would read it as a flag.
    pub fn new(repository: Repository, branch: String) -> Result<Self> {
        plain("branch", &branch)?;
        Ok(Self { repository, branch })
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "merge");
        argv.extend([
            "--no-ff".to_owned(),
            "--no-edit".to_owned(),
            self.branch.clone(),
        ]);
        argv
    }
}

/// `git merge --abort`: `repository` put back as it was before a merge
/// that conflicted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbortMerge {
    repository: Repository,
}

impl AbortMerge {
    #[must_use]
    pub const fn new(repository: Repository) -> Self {
        Self { repository }
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "merge");
        argv.push("--abort".to_owned());
        argv
    }
}

/// `git reset --hard`: the checked-out branch of `repository` moved to
/// `to`, and the index and working tree made to match — everything
/// uncommitted discarded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reset {
    repository: Repository,
    to: Hash,
}

impl Reset {
    #[must_use]
    pub const fn new(repository: Repository, to: Hash) -> Self {
        Self { repository, to }
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "reset");
        argv.extend(["--hard".to_owned(), self.to.to_string()]);
        argv
    }
}

/// `git push`: `branch` of `repository` sent to `remote` under the same
/// name, by the exact refspec, so no configuration decides where it
/// goes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Push {
    repository: Repository,
    remote: Remote,
    branch: String,
}

impl Push {
    /// `branch` pushed to `remote`.
    ///
    /// # Errors
    ///
    /// A branch beginning with `-`: git would read it as a flag.
    pub fn new(repository: Repository, remote: Remote, branch: String) -> Result<Self> {
        plain("branch", &branch)?;
        Ok(Self {
            repository,
            remote,
            branch,
        })
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "push");
        argv.extend([
            "--".to_owned(),
            self.remote.to_string(),
            format!("refs/heads/{0}:refs/heads/{0}", self.branch),
        ]);
        argv
    }
}

/// `git remote --verbose`: every remote configured in `repository`,
/// with where it fetches from and pushes to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListRemotes {
    repository: Repository,
}

/// One remote as `git remote --verbose` reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteEntry {
    pub name: String,
    pub fetch: Url,
    pub push: Url,
}

impl ListRemotes {
    #[must_use]
    pub const fn new(repository: Repository) -> Self {
        Self { repository }
    }

    /// The remotes in `stdout`, as [`ListRemotes::argv`] asked for
    /// them: a line per direction, `name`, a tab, the URL, and
    /// `(fetch)` or `(push)`.
    ///
    /// # Errors
    ///
    /// A line of another shape, a URL that is none, or a remote with
    /// one direction only — none of which git produces.
    pub fn remotes(stdout: &str) -> Result<Vec<RemoteEntry>> {
        let mut remotes: Vec<(String, Option<Url>, Option<Url>)> = Vec::new();
        for line in stdout.lines() {
            let Some((name, rest)) = line.split_once('\t') else {
                bail!("not a remote: {line:?}");
            };
            let Some((url, direction)) = rest.rsplit_once(' ') else {
                bail!("not a remote: {line:?}");
            };
            let url: Url = url.parse()?;
            let remote = match remotes.iter_mut().find(|(known, ..)| known == name) {
                Some(remote) => remote,
                None => {
                    remotes.push((name.to_owned(), None, None));
                    remotes.last_mut().expect("just pushed")
                }
            };
            match direction {
                "(fetch)" => remote.1 = Some(url),
                "(push)" => remote.2 = Some(url),
                _ => bail!("not a remote: {line:?}"),
            }
        }
        remotes
            .into_iter()
            .map(|(name, fetch, push)| match (fetch, push) {
                (Some(fetch), Some(push)) => Ok(RemoteEntry { name, fetch, push }),
                _ => bail!("the remote {name:?} has one direction only"),
            })
            .collect()
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "remote");
        argv.push("--verbose".to_owned());
        argv
    }
}

/// `git branch --delete --force`: `name` removed from `repository`,
/// merged or not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteBranch {
    repository: Repository,
    name: String,
}

impl DeleteBranch {
    /// The branch `name`, deleted.
    ///
    /// # Errors
    ///
    /// A name beginning with `-`: git would read it as a flag.
    pub fn new(repository: Repository, name: String) -> Result<Self> {
        plain("branch", &name)?;
        Ok(Self { repository, name })
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.repository, "branch");
        argv.extend([
            "--delete".to_owned(),
            "--force".to_owned(),
            self.name.clone(),
        ]);
        argv
    }
}

/// `git worktree remove`: `linked`, a linked worktree of `main`, taken
/// away — with `force`, even when it holds uncommitted work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoveWorktree {
    main: Repository,
    linked: Repository,
    force: bool,
}

impl RemoveWorktree {
    #[must_use]
    pub const fn new(main: Repository, linked: Repository, force: bool) -> Self {
        Self {
            main,
            linked,
            force,
        }
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = within(&self.main, "worktree");
        argv.push("remove".to_owned());
        if self.force {
            argv.push("--force".to_owned());
        }
        argv.extend([
            "--".to_owned(),
            self.linked.path().to_string_lossy().into_owned(),
        ]);
        argv
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        text.parse().unwrap()
    }

    fn date(text: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(text).unwrap()
    }

    fn repository() -> Repository {
        Repository(PathBuf::from("/work/repo"))
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn hash() -> Hash {
        Hash::new(SHA).unwrap()
    }

    #[test]
    fn a_hash_is_forty_or_sixty_four_hex_digits() {
        assert_eq!(hash().to_string(), SHA);
        assert_eq!(Hash::new(&format!(" {SHA}\n")).unwrap(), hash(), "trimmed");
        assert!(Hash::new(&"a".repeat(64)).is_ok());
        for text in ["", "abc", &"g".repeat(40), &"A".repeat(40), &"a".repeat(41)] {
            assert_eq!(
                Hash::new(text).unwrap_err().to_string(),
                format!("not a commit hash: {text:?}")
            );
        }
    }

    #[test]
    fn a_remote_is_a_name_or_a_url() {
        assert_eq!(
            Remote::named("origin".to_owned()).unwrap().to_string(),
            "origin"
        );
        assert_eq!(
            Remote::from(url("git@github.com:owner/repo.git")).to_string(),
            "git@github.com:owner/repo.git"
        );
        assert_eq!(
            Remote::named("--all".to_owned()).unwrap_err().to_string(),
            "a remote name may not begin with a dash: \"--all\""
        );
    }

    #[test]
    fn the_init_argv_makes_a_bare_repository_on_main() {
        let init = Init::new(PathBuf::from("/srv/origin.git")).unwrap();
        assert_eq!(
            Command::Init(init).argv(),
            [
                "init",
                "--quiet",
                "--bare",
                "--initial-branch=main",
                "--",
                "/srv/origin.git"
            ]
        );
        let error = Init::new(PathBuf::from("origin.git")).unwrap_err();
        assert_eq!(
            error.to_string(),
            "the repository must be an absolute path: origin.git"
        );
    }

    #[test]
    fn the_clone_argv_is_the_verb_then_the_flags_then_the_positionals() {
        let clone = Clone::new(
            url("git@github.com:owner/repo.git"),
            "/work/repo".into(),
            None,
        )
        .unwrap();
        assert_eq!(
            Command::Clone(clone).argv(),
            ["clone", "--", "git@github.com:owner/repo.git", "/work/repo"]
        );

        let clone = Clone::new(
            url("https://github.com/owner/repo.git"),
            "/work/repo".into(),
            Some("release".to_owned()),
        )
        .unwrap();
        assert_eq!(
            Command::Clone(clone).argv(),
            [
                "clone",
                "--branch",
                "release",
                "--",
                "https://github.com/owner/repo.git",
                "/work/repo"
            ]
        );
    }

    #[test]
    fn a_clone_is_checked_at_the_door() {
        let refused = |from: &str, into: &str, branch: Option<&str>| {
            Clone::new(url(from), into.into(), branch.map(str::to_owned))
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            refused("/srv/repo.git", "work/repo", None),
            "the destination must be an absolute path: work/repo"
        );
        assert_eq!(
            refused("srv/repo.git", "/work/repo", None),
            "a local source must be an absolute path: srv/repo.git"
        );
        assert_eq!(
            refused("/srv/repo.git", "/work/repo", Some("-x")),
            "a branch may not begin with a dash: \"-x\""
        );
    }

    #[test]
    fn a_clone_arrives_as_json_through_the_same_door() {
        let clone: Clone =
            serde_json::from_str(r#"{"from":"git@github.com:owner/repo.git","into":"/work/repo"}"#)
                .unwrap();
        assert_eq!(
            clone,
            Clone::new(
                url("git@github.com:owner/repo.git"),
                "/work/repo".into(),
                None
            )
            .unwrap()
        );
        let error = serde_json::from_str::<Clone>(r#"{"from":"/srv/repo.git","into":"work/repo"}"#)
            .unwrap_err();
        assert!(
            error.to_string().contains("must be an absolute path"),
            "{error}"
        );
    }

    #[test]
    fn the_set_config_argv_sets_a_local_key_behind_a_double_dash() {
        let config =
            SetConfig::new(repository(), "user.name".to_owned(), "-Ada-".to_owned()).unwrap();
        assert_eq!(
            Command::SetConfig(config).argv(),
            [
                "-C",
                "/work/repo",
                "config",
                "--local",
                "--",
                "user.name",
                "-Ada-"
            ]
        );
        let error =
            SetConfig::new(repository(), "--global".to_owned(), "x".to_owned()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "a config key may not begin with a dash: \"--global\""
        );
    }

    #[test]
    fn the_branch_argv_names_the_branch_then_where_it_starts() {
        let branch = Branch::new(repository(), "topic".to_owned(), None).unwrap();
        assert_eq!(
            Command::Branch(branch).argv(),
            ["-C", "/work/repo", "branch", "topic"]
        );
        let branch = Branch::new(repository(), "topic".to_owned(), Some(hash())).unwrap();
        assert_eq!(
            Command::Branch(branch).argv(),
            ["-C", "/work/repo", "branch", "topic", SHA]
        );
        let error = Branch::new(repository(), "-D".to_owned(), None).unwrap_err();
        assert_eq!(
            error.to_string(),
            "a branch may not begin with a dash: \"-D\""
        );
    }

    #[test]
    fn the_switch_argv_names_the_branch() {
        let switch = Switch::new(repository(), "topic".to_owned()).unwrap();
        assert_eq!(
            Command::Switch(switch).argv(),
            ["-C", "/work/repo", "switch", "topic"]
        );
        assert!(Switch::new(repository(), "-c".to_owned()).is_err());
    }

    #[test]
    fn the_worktree_argvs_add_list_and_remove() {
        let add = AddWorktree::new(repository(), "/work/topic".into(), "topic".to_owned()).unwrap();
        assert_eq!(
            Command::AddWorktree(add).argv(),
            [
                "-C",
                "/work/repo",
                "worktree",
                "add",
                "--",
                "/work/topic",
                "topic"
            ]
        );
        assert!(AddWorktree::new(repository(), "topic".into(), "topic".to_owned()).is_err());
        assert!(AddWorktree::new(repository(), "/work/topic".into(), "-b".to_owned()).is_err());

        assert_eq!(
            Command::ListWorktrees(ListWorktrees::new(repository())).argv(),
            ["-C", "/work/repo", "worktree", "list", "--porcelain"]
        );

        let topic = Repository(PathBuf::from("/work/topic"));
        let remove = RemoveWorktree::new(repository(), topic.clone(), false);
        assert_eq!(
            Command::RemoveWorktree(remove).argv(),
            [
                "-C",
                "/work/repo",
                "worktree",
                "remove",
                "--",
                "/work/topic"
            ]
        );
        let remove = RemoveWorktree::new(repository(), topic, true);
        assert_eq!(
            Command::RemoveWorktree(remove).argv(),
            [
                "-C",
                "/work/repo",
                "worktree",
                "remove",
                "--force",
                "--",
                "/work/topic"
            ]
        );
    }

    #[test]
    fn worktrees_read_one_paragraph_each() {
        let stdout = format!(
            "worktree /work/repo\nHEAD {SHA}\nbranch refs/heads/main\n\n\
             worktree /work/topic\nHEAD {SHA}\nbranch refs/heads/topic\n\n\
             worktree /srv/origin.git\nbare\n\n\
             worktree /work/loose\nHEAD {SHA}\ndetached\n\n"
        );
        let worktrees = ListWorktrees::worktrees(&stdout).unwrap();
        assert_eq!(
            worktrees,
            [
                WorktreeInfo {
                    path: "/work/repo".into(),
                    head: Some(hash()),
                    branch: Some("main".to_owned()),
                    bare: false,
                },
                WorktreeInfo {
                    path: "/work/topic".into(),
                    head: Some(hash()),
                    branch: Some("topic".to_owned()),
                    bare: false,
                },
                WorktreeInfo {
                    path: "/srv/origin.git".into(),
                    head: None,
                    branch: None,
                    bare: true,
                },
                WorktreeInfo {
                    path: "/work/loose".into(),
                    head: Some(hash()),
                    branch: None,
                    bare: false,
                },
            ]
        );
        assert_eq!(ListWorktrees::worktrees("").unwrap(), []);
        assert!(ListWorktrees::worktrees("HEAD abc\n").is_err());
    }

    #[test]
    fn the_add_argv_names_the_repository_then_the_paths_behind_a_double_dash() {
        let add = Add::new(repository(), vec!["hello.txt".into(), "-flagged".into()]).unwrap();
        assert_eq!(
            Command::Add(add).argv(),
            ["-C", "/work/repo", "add", "--", "hello.txt", "-flagged"]
        );

        let error = Add::new(repository(), vec![]).unwrap_err();
        assert_eq!(error.to_string(), "an add needs at least one path");
        let error = Add::new(repository(), vec![":(glob)*.rs".into()]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "a path may not begin with a colon: :(glob)*.rs"
        );
    }

    #[test]
    fn the_status_argv_asks_for_porcelain_with_nul_terminators() {
        assert_eq!(
            Command::Status(Status::new(repository())).argv(),
            ["-C", "/work/repo", "status", "--porcelain=v1", "-z"]
        );
    }

    #[test]
    fn status_changes_read_each_entry_and_a_renames_source() {
        let stdout = " M hello.txt\0A  new.txt\0R  moved.txt\0old.txt\0?? loose.txt\0UU both.txt\0";
        let change = |staged, unstaged, path: &str, from: Option<&str>| Change {
            staged,
            unstaged,
            path: path.into(),
            from: from.map(PathBuf::from),
        };
        assert_eq!(
            Status::changes(stdout).unwrap(),
            [
                change(' ', 'M', "hello.txt", None),
                change('A', ' ', "new.txt", None),
                change('R', ' ', "moved.txt", Some("old.txt")),
                change('?', '?', "loose.txt", None),
                change('U', 'U', "both.txt", None),
            ]
        );
        assert_eq!(Status::changes("").unwrap(), [], "clean");
        assert_eq!(
            Status::changes("M\0").unwrap_err().to_string(),
            "not a status entry: \"M\""
        );
        assert_eq!(
            Status::changes("R  moved.txt\0").unwrap_err().to_string(),
            "a rename without its source: \"R  moved.txt\""
        );
    }

    #[test]
    fn the_diff_argv_puts_revisions_before_the_double_dash_and_the_path_after() {
        assert_eq!(
            Command::Diff(Diff::new(repository(), None, None).unwrap()).argv(),
            ["-C", "/work/repo", "diff", "--"]
        );
        let diff = Diff::new(
            repository(),
            Some("main..topic".to_owned()),
            Some("src".into()),
        )
        .unwrap();
        assert_eq!(
            Command::Diff(diff).argv(),
            ["-C", "/work/repo", "diff", "main..topic", "--", "src"]
        );
        assert!(Diff::new(repository(), Some("--cached".to_owned()), None).is_err());
    }

    #[test]
    fn the_commit_argv_names_the_repository_then_the_verb_then_the_message() {
        let commit = Commit::new(repository(), "feat: the first\n\nA body.".to_owned()).unwrap();
        assert_eq!(
            Command::Commit(commit).argv(),
            [
                "-C",
                "/work/repo",
                "commit",
                "--message=feat: the first\n\nA body."
            ]
        );
    }

    #[test]
    fn a_commit_without_a_message_is_refused_at_the_door() {
        let error = Commit::new(repository(), "  \n".to_owned()).unwrap_err();
        assert_eq!(error.to_string(), "a commit needs a message");
    }

    #[test]
    fn the_show_prefix_argv_asks_where_a_path_sits() {
        let prefix = ShowPrefix::new(PathBuf::from("/work/repo/src")).unwrap();
        assert_eq!(
            Command::ShowPrefix(prefix).argv(),
            ["-C", "/work/repo/src", "rev-parse", "--show-prefix"]
        );
        assert!(ShowPrefix::new(PathBuf::from("src")).is_err());
        assert_eq!(ShowPrefix::prefix("\n"), "");
        assert_eq!(ShowPrefix::prefix("src/\n"), "src/");
    }

    #[test]
    fn the_rev_parse_argv_verifies_a_commit() {
        let rev_parse = RevParse::new(repository(), "main".to_owned()).unwrap();
        assert_eq!(
            Command::RevParse(rev_parse).argv(),
            ["-C", "/work/repo", "rev-parse", "--verify", "main^{commit}"]
        );
        assert!(RevParse::new(repository(), "--all".to_owned()).is_err());
        assert_eq!(RevParse::hash(&format!("{SHA}\n")).unwrap(), hash());
    }

    #[test]
    fn the_log_argv_names_the_repository_the_format_the_count_and_the_revision() {
        let log = Log::new(repository(), None, None).unwrap();
        assert_eq!(
            Command::Log(log).argv(),
            ["-C", "/work/repo", "log", "--format=%H%x00%an%x00%aI%x00%s"]
        );

        let log = Log::new(repository(), Some("main".to_owned()), Some(20)).unwrap();
        assert_eq!(
            Command::Log(log).argv(),
            [
                "-C",
                "/work/repo",
                "log",
                "--format=%H%x00%an%x00%aI%x00%s",
                "--max-count=20",
                "main"
            ]
        );
    }

    #[test]
    fn a_log_of_a_revision_that_reads_as_a_flag_is_refused_at_the_door() {
        let error = Log::new(repository(), Some("--all".to_owned()), None).unwrap_err();
        assert_eq!(
            error.to_string(),
            "a revision may not begin with a dash: \"--all\""
        );
    }

    #[test]
    fn log_entries_read_one_commit_per_line() {
        let stdout = "0123456789abcdef0123456789abcdef01234567\x00Ada\x002026-09-10T14:03:00-05:00\x00feat: later\n\
                      fedcba9876543210fedcba9876543210fedcba98\x00Bob\x002026-09-09T09:00:00+00:00\x00the first commit\n";
        assert_eq!(
            Log::entries(stdout).unwrap(),
            [
                Entry {
                    hash: hash(),
                    author: "Ada".to_owned(),
                    date: date("2026-09-10T14:03:00-05:00"),
                    subject: "feat: later".to_owned(),
                },
                Entry {
                    hash: Hash::new("fedcba9876543210fedcba9876543210fedcba98").unwrap(),
                    author: "Bob".to_owned(),
                    date: date("2026-09-09T09:00:00+00:00"),
                    subject: "the first commit".to_owned(),
                },
            ]
        );
        assert_eq!(Log::entries("").unwrap(), []);

        let error = Log::entries("abc\0Ada\n").unwrap_err();
        assert_eq!(error.to_string(), "not a log entry: \"abc\\0Ada\"");

        let error = Log::entries(&format!("{SHA}\x00Ada\x00yesterday\x00fix\n")).unwrap_err();
        assert_eq!(error.to_string(), "not a log entry's date: \"yesterday\"");
    }

    #[test]
    fn the_show_argv_names_the_revision() {
        let show = Show::new(repository(), "HEAD~1".to_owned()).unwrap();
        assert_eq!(
            Command::Show(show).argv(),
            ["-C", "/work/repo", "show", "HEAD~1"]
        );
        assert!(Show::new(repository(), "--stat".to_owned()).is_err());
    }

    #[test]
    fn the_fetch_and_pull_argvs_name_the_remote_and_fast_forward_only() {
        let fetch = Fetch::new(repository(), Remote::named("origin".to_owned()).unwrap());
        assert_eq!(
            Command::Fetch(fetch).argv(),
            ["-C", "/work/repo", "fetch", "origin"]
        );
        assert_eq!(
            Command::Pull(Pull::new(repository())).argv(),
            ["-C", "/work/repo", "pull", "--ff-only"]
        );
    }

    #[test]
    fn the_merge_argvs_merge_without_fast_forward_abort_and_reset() {
        let merge = Merge::new(repository(), "topic".to_owned()).unwrap();
        assert_eq!(
            Command::Merge(merge).argv(),
            ["-C", "/work/repo", "merge", "--no-ff", "--no-edit", "topic"]
        );
        assert!(Merge::new(repository(), "--squash".to_owned()).is_err());
        assert_eq!(
            Command::AbortMerge(AbortMerge::new(repository())).argv(),
            ["-C", "/work/repo", "merge", "--abort"]
        );
        assert_eq!(
            Command::Reset(Reset::new(repository(), hash())).argv(),
            ["-C", "/work/repo", "reset", "--hard", SHA]
        );
    }

    #[test]
    fn the_push_argv_names_the_remote_and_the_exact_refspec() {
        let push = Push::new(
            repository(),
            Remote::from(url("/srv/origin.git")),
            "topic".to_owned(),
        )
        .unwrap();
        assert_eq!(
            Command::Push(push).argv(),
            [
                "-C",
                "/work/repo",
                "push",
                "--",
                "/srv/origin.git",
                "refs/heads/topic:refs/heads/topic"
            ]
        );
        assert!(
            Push::new(
                repository(),
                Remote::named("origin".to_owned()).unwrap(),
                "-f".to_owned()
            )
            .is_err()
        );
    }

    #[test]
    fn remotes_read_a_line_per_direction() {
        assert_eq!(
            Command::ListRemotes(ListRemotes::new(repository())).argv(),
            ["-C", "/work/repo", "remote", "--verbose"]
        );
        let stdout = "origin\t/srv/origin.git (fetch)\norigin\t/srv/origin.git (push)\n\
                      fork\tgit@github.com:me/repo.git (fetch)\nfork\tgit@github.com:me/repo.git (push)\n";
        assert_eq!(
            ListRemotes::remotes(stdout).unwrap(),
            [
                RemoteEntry {
                    name: "origin".to_owned(),
                    fetch: url("/srv/origin.git"),
                    push: url("/srv/origin.git"),
                },
                RemoteEntry {
                    name: "fork".to_owned(),
                    fetch: url("git@github.com:me/repo.git"),
                    push: url("git@github.com:me/repo.git"),
                },
            ]
        );
        assert_eq!(ListRemotes::remotes("").unwrap(), []);
        assert_eq!(
            ListRemotes::remotes("origin /x (fetch)\n")
                .unwrap_err()
                .to_string(),
            "not a remote: \"origin /x (fetch)\""
        );
        assert_eq!(
            ListRemotes::remotes("origin\t/x (fetch)\n")
                .unwrap_err()
                .to_string(),
            "the remote \"origin\" has one direction only"
        );
    }

    #[test]
    fn the_delete_branch_argv_forces() {
        let delete = DeleteBranch::new(repository(), "topic".to_owned()).unwrap();
        assert_eq!(
            Command::DeleteBranch(delete).argv(),
            ["-C", "/work/repo", "branch", "--delete", "--force", "topic"]
        );
        assert!(DeleteBranch::new(repository(), "-r".to_owned()).is_err());
    }
}

/// These run the real `git` through the verbs in [`git`](super), so
/// they are compiled only where the tests can count on one.
#[cfg(all(test, unix, feature = "native"))]
mod live {
    use super::*;
    use crate::git::testing::{cloned, committed, head, identified, origin, seeded};
    use crate::git::{
        abort_merge, add, add_worktree, branch, clone, commit, delete_branch, diff, fetch, init,
        list_remotes, list_worktrees, log, merge, pull, push, remove_worktree, reset, rev_parse,
        show, status, switch,
    };
    use crate::temp::Scratch;

    fn url(text: &str) -> Url {
        text.parse().unwrap()
    }

    fn subjects(repository: &Repository) -> Vec<String> {
        log(Log::new(repository.clone(), None, None).unwrap())
            .unwrap()
            .into_iter()
            .map(|entry| entry.subject)
            .collect()
    }

    /// The whole vocabulary, end to end and through nothing else: a
    /// bare origin, a clone of it, a file written, staged, and
    /// committed, changed and committed again, and the history read
    /// back.
    #[test]
    fn a_repository_is_made_cloned_written_to_committed_and_read_back() {
        let scratch = Scratch::new("end-to-end");

        let origin = init(Init::new(scratch.join("origin.git").into()).unwrap()).unwrap();
        assert!(
            origin.path().join("HEAD").is_file(),
            "bare: HEAD at the top"
        );

        let work = cloned(&scratch, &origin, "work", None);
        identified(&work);

        let hello = work.path().join("hello.txt");
        let first = committed(&work, "hello.txt", "hello\n", "feat: say hello");
        let second = committed(
            &work,
            "hello.txt",
            "hello, world\n",
            "feat: say it to the world",
        );

        let entries = log(Log::new(work.clone(), None, None).unwrap()).unwrap();
        let subjects: Vec<&str> = entries.iter().map(|entry| entry.subject.as_str()).collect();
        assert_eq!(subjects, ["feat: say it to the world", "feat: say hello"]);
        assert_eq!(entries[0].hash, second);
        assert_eq!(entries[1].hash, first);
        assert!(entries.iter().all(|entry| entry.author == "Ada"));
        assert_eq!(std::fs::read_to_string(&hello).unwrap(), "hello, world\n");
    }

    #[test]
    fn a_clone_makes_a_repository_with_the_source_at_head() {
        let scratch = Scratch::new("clone");
        let (origin, _) = seeded(&scratch);
        let repository = cloned(&scratch, &origin, "clone", None);

        assert_eq!(repository.path(), Path::new(&scratch.join("clone")));
        assert!(repository.path().join(".git").is_dir());
        assert_eq!(
            std::fs::read_to_string(repository.path().join("hello.txt")).unwrap(),
            "hello\n"
        );
    }

    #[test]
    fn a_clone_checks_out_the_branch_it_was_asked_for() {
        let scratch = Scratch::new("clone-branch");
        let (origin, work) = seeded(&scratch);
        branch(Branch::new(work.clone(), "release".to_owned(), None).unwrap()).unwrap();
        push(Push::new(work, self::origin(), "release".to_owned()).unwrap()).unwrap();

        let release = cloned(&scratch, &origin, "clone", Some("release"));

        let worktrees = list_worktrees(ListWorktrees::new(release)).unwrap();
        assert_eq!(worktrees[0].branch.as_deref(), Some("release"));
    }

    #[test]
    fn a_clone_of_a_source_that_is_missing_fails_in_gits_own_words() {
        let scratch = Scratch::new("clone-missing");
        let into = scratch.join("clone");

        let error =
            clone(Clone::new(url("/nonexistent/repo.git"), into.clone().into(), None).unwrap())
                .unwrap_err()
                .to_string();

        assert!(error.contains("/nonexistent/repo.git"), "{error}");
        assert!(
            !Path::new(&into).exists(),
            "git leaves no half-made clone behind"
        );
    }

    #[test]
    fn a_branch_is_made_switched_to_worked_on_and_deleted() {
        let scratch = Scratch::new("branch");
        let (_, work) = seeded(&scratch);
        let main = head(&work);

        branch(Branch::new(work.clone(), "topic".to_owned(), Some(main.clone())).unwrap()).unwrap();
        switch(Switch::new(work.clone(), "topic".to_owned()).unwrap()).unwrap();
        let topic = committed(&work, "topic.txt", "topic\n", "feat: on topic");
        assert_eq!(subjects(&work), ["feat: on topic", "the first commit"]);

        switch(Switch::new(work.clone(), "main".to_owned()).unwrap()).unwrap();
        assert_eq!(head(&work), main, "main stayed where it was");
        assert!(!work.path().join("topic.txt").exists());
        assert_eq!(
            rev_parse(RevParse::new(work.clone(), "topic".to_owned()).unwrap()).unwrap(),
            topic
        );

        delete_branch(DeleteBranch::new(work.clone(), "topic".to_owned()).unwrap()).unwrap();
        let error = rev_parse(RevParse::new(work, "topic".to_owned()).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("topic"), "{error}");
    }

    #[test]
    fn a_worktree_is_added_listed_worked_in_and_removed() {
        let scratch = Scratch::new("worktree");
        let (_, work) = seeded(&scratch);
        branch(Branch::new(work.clone(), "topic".to_owned(), None).unwrap()).unwrap();

        let topic = add_worktree(
            AddWorktree::new(
                work.clone(),
                scratch.join("topic").into(),
                "topic".to_owned(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(topic.path(), Path::new(&scratch.join("topic")));

        let listed = list_worktrees(ListWorktrees::new(work.clone())).unwrap();
        let branches: Vec<Option<&str>> = listed.iter().map(|w| w.branch.as_deref()).collect();
        assert_eq!(branches, [Some("main"), Some("topic")]);
        // macOS answers through the temp dir's resolved name.
        assert_eq!(
            listed[1].path.canonicalize().unwrap(),
            topic.path().canonicalize().unwrap()
        );
        assert_eq!(listed[1].head, Some(head(&work)), "same commit, both");

        identified(&topic);
        let advanced = committed(&topic, "topic.txt", "topic\n", "feat: in the worktree");
        assert_eq!(
            rev_parse(RevParse::new(work.clone(), "topic".to_owned()).unwrap()).unwrap(),
            advanced,
            "the branch moved in the shared history"
        );

        std::fs::write(topic.path().join("loose.txt"), "loose\n").unwrap();
        let error = remove_worktree(RemoveWorktree::new(work.clone(), topic.clone(), false))
            .unwrap_err()
            .to_string();
        assert!(error.contains("untracked"), "{error}");
        remove_worktree(RemoveWorktree::new(work.clone(), topic.clone(), true)).unwrap();
        assert!(!topic.path().exists());
        assert_eq!(list_worktrees(ListWorktrees::new(work)).unwrap().len(), 1);
    }

    #[test]
    fn status_reads_the_tree_as_it_stands() {
        let scratch = Scratch::new("status");
        let (_, work) = seeded(&scratch);
        assert_eq!(
            status(Status::new(work.clone())).unwrap(),
            [],
            "clean after seeding"
        );

        std::fs::write(work.path().join("hello.txt"), "changed\n").unwrap();
        std::fs::write(work.path().join("new.txt"), "new\n").unwrap();
        add(Add::new(work.clone(), vec!["new.txt".into()]).unwrap()).unwrap();
        std::fs::write(work.path().join("loose.txt"), "loose\n").unwrap();

        let mut changes = status(Status::new(work)).unwrap();
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        let summary: Vec<(char, char, &str)> = changes
            .iter()
            .map(|c| (c.staged, c.unstaged, c.path.to_str().unwrap()))
            .collect();
        assert_eq!(
            summary,
            [
                (' ', 'M', "hello.txt"),
                ('?', '?', "loose.txt"),
                ('A', ' ', "new.txt"),
            ]
        );
    }

    #[test]
    fn diff_and_show_read_the_patch() {
        let scratch = Scratch::new("diff");
        let (_, work) = seeded(&scratch);
        std::fs::write(work.path().join("hello.txt"), "hello, world\n").unwrap();

        let patch = diff(Diff::new(work.clone(), None, None).unwrap()).unwrap();
        assert!(patch.contains("-hello\n+hello, world"), "{patch}");
        let narrowed =
            diff(Diff::new(work.clone(), None, Some("other.txt".into())).unwrap()).unwrap();
        assert_eq!(narrowed, "", "nothing under that path");

        add(Add::new(work.clone(), vec!["hello.txt".into()]).unwrap()).unwrap();
        commit(Commit::new(work.clone(), "feat: to the world".to_owned()).unwrap()).unwrap();
        let between =
            diff(Diff::new(work.clone(), Some("HEAD~1..HEAD".to_owned()), None).unwrap()).unwrap();
        assert!(between.contains("+hello, world"), "{between}");

        let shown = show(Show::new(work, "HEAD".to_owned()).unwrap()).unwrap();
        assert!(shown.contains("feat: to the world"), "{shown}");
        assert!(shown.contains("+hello, world"), "{shown}");
    }

    #[test]
    fn a_commit_with_nothing_staged_fails_in_gits_own_words() {
        let scratch = Scratch::new("commit-nothing");
        let (_, work) = seeded(&scratch);

        let error = commit(Commit::new(work, "nothing".to_owned()).unwrap())
            .unwrap_err()
            .to_string();

        assert!(error.contains("nothing to commit"), "{error}");
    }

    #[test]
    fn rev_parse_names_the_commit_and_refuses_what_is_none() {
        let scratch = Scratch::new("rev-parse");
        let (_, work) = seeded(&scratch);
        let first = head(&work);

        let entries = log(Log::new(work.clone(), None, None).unwrap()).unwrap();
        assert_eq!(entries[0].hash, first, "log and rev-parse agree");
        assert_eq!(
            rev_parse(RevParse::new(work.clone(), "main".to_owned()).unwrap()).unwrap(),
            first
        );
        let front = first.to_string()[..8].to_owned();
        assert_eq!(
            rev_parse(RevParse::new(work.clone(), front).unwrap()).unwrap(),
            first,
            "the front of a hash"
        );
        let error = rev_parse(RevParse::new(work, "nowhere".to_owned()).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("nowhere"), "{error}");
    }

    #[test]
    fn a_log_reads_the_history_newest_first() {
        let scratch = Scratch::new("log");
        let (_, work) = seeded(&scratch);
        let later = committed(&work, "more.txt", "more\n", "feat: later");

        let entries = log(Log::new(work.clone(), None, None).unwrap()).unwrap();
        let [newer, older] = entries.as_slice() else {
            panic!("two commits: {entries:?}");
        };
        assert_eq!(newer.hash, later);
        assert_eq!(newer.subject, "feat: later");
        assert_eq!(newer.author, "Ada");
        assert!(
            newer.date >= older.date,
            "{} before {}",
            newer.date,
            older.date
        );
        assert_eq!(older.subject, "the first commit");

        let one = log(Log::new(work.clone(), Some("main".to_owned()), Some(1)).unwrap()).unwrap();
        assert_eq!(one, std::slice::from_ref(newer));

        let error = log(Log::new(work, Some("nowhere".to_owned()), None).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("nowhere"), "{error}");
    }

    #[test]
    fn push_fetch_and_pull_move_commits_between_clones() {
        let scratch = Scratch::new("push-fetch-pull");
        let (origin, ada) = seeded(&scratch);
        let bob = cloned(&scratch, &origin, "bob", None);

        let later = committed(&ada, "more.txt", "more\n", "feat: later");
        push(Push::new(ada.clone(), self::origin(), "main".to_owned()).unwrap()).unwrap();
        assert_eq!(
            log(Log::new(origin.clone(), Some("main".to_owned()), None).unwrap()).unwrap()[0].hash,
            later,
            "the origin has it"
        );

        fetch(Fetch::new(bob.clone(), self::origin())).unwrap();
        assert_eq!(
            rev_parse(RevParse::new(bob.clone(), "origin/main".to_owned()).unwrap()).unwrap(),
            later,
            "fetched, under the remote-tracking name"
        );
        assert_ne!(head(&bob), later, "and nothing checked out changed");

        pull(Pull::new(bob.clone())).unwrap();
        assert_eq!(head(&bob), later, "pulled");
        assert!(bob.path().join("more.txt").is_file());

        let remotes = list_remotes(ListRemotes::new(bob)).unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].fetch, Url::Local(origin.path().to_owned()));
        assert_eq!(remotes[0].push, remotes[0].fetch);
    }

    #[test]
    fn a_merge_lands_as_a_merge_commit_and_a_conflict_is_abortable() {
        let scratch = Scratch::new("merge");
        let (_, work) = seeded(&scratch);
        let before = head(&work);

        branch(Branch::new(work.clone(), "topic".to_owned(), None).unwrap()).unwrap();
        switch(Switch::new(work.clone(), "topic".to_owned()).unwrap()).unwrap();
        committed(&work, "topic.txt", "topic\n", "feat: on topic");
        switch(Switch::new(work.clone(), "main".to_owned()).unwrap()).unwrap();

        merge(Merge::new(work.clone(), "topic".to_owned()).unwrap()).unwrap();
        let entries = log(Log::new(work.clone(), None, Some(1)).unwrap()).unwrap();
        assert_eq!(
            entries[0].subject, "Merge branch 'topic'",
            "a merge commit, no fast-forward"
        );
        assert!(work.path().join("topic.txt").is_file());
        let merged = head(&work);

        // Both sides change hello.txt: a conflict.
        branch(Branch::new(work.clone(), "clash".to_owned(), Some(before)).unwrap()).unwrap();
        switch(Switch::new(work.clone(), "clash".to_owned()).unwrap()).unwrap();
        committed(&work, "hello.txt", "theirs\n", "feat: theirs");
        switch(Switch::new(work.clone(), "main".to_owned()).unwrap()).unwrap();
        committed(&work, "hello.txt", "ours\n", "feat: ours");
        let ours = head(&work);

        let error = merge(Merge::new(work.clone(), "clash".to_owned()).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("CONFLICT"), "{error}");
        let conflicted = status(Status::new(work.clone())).unwrap();
        assert!(
            conflicted
                .iter()
                .any(|c| c.staged == 'U' && c.unstaged == 'U' && c.path == Path::new("hello.txt")),
            "{conflicted:?}"
        );

        abort_merge(AbortMerge::new(work.clone())).unwrap();
        assert_eq!(
            status(Status::new(work.clone())).unwrap(),
            [],
            "clean again"
        );
        assert_eq!(head(&work), ours);

        reset(Reset::new(work.clone(), merged.clone())).unwrap();
        assert_eq!(head(&work), merged);
        assert_eq!(
            std::fs::read_to_string(work.path().join("hello.txt")).unwrap(),
            "hello\n"
        );
    }
}
