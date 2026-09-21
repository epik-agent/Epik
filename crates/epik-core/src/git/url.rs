//! Git's addresses: the three spellings a remote comes in.
//!
//! Git names a repository three ways, and [`Url`] is one type for all
//! of them, sorted by the rule git itself applies, in the order git
//! applies it:
//!
//! 1. A string with no colon, or with a slash before its first colon,
//!    is a [local path](Url::Local): `/srv/git/repo.git`, `../repo`,
//!    `dir/with:colon`. On Windows a drive letter counts as a path too.
//! 2. Otherwise a string that begins `scheme://` is a [URL
//!    proper](Url::Scheme): `https://host/path`, `ssh://user@host:22/path`,
//!    `git://host/path`, `file:///path`. Any scheme is accepted, since
//!    git hands an unknown one to its remote helper.
//! 3. Everything else is the [scp-like](Url::Scp) spelling of ssh:
//!    `[user@]host:path`, the host bracketed when it is an IPv6 literal.
//!
//! Only a URL proper is parsed by the [`url`] crate, and it comes back
//! normalised as that crate has it: the host lowercased, the default
//! port dropped, the odd character percent-encoded. The other two are
//! kept as written. A host or path beginning with `-` is refused in the
//! two remote spellings, as git refuses it: either would otherwise reach
//! ssh as an option.

use std::borrow::Cow;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Where a repository is, in whichever spelling git was given.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Url {
    /// `scheme://…`, as the [`url`] crate reads it.
    Scheme(url::Url),
    /// `[user@]host:path`, ssh without saying so.
    Scp(Scp),
    /// A path on this machine, relative or absolute.
    Local(PathBuf),
}

/// The scp-like spelling, taken apart: `[user@]host:path`.
/// SCP stands for Secure Copy Protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scp {
    pub user: Option<String>,
    /// As written, brackets included for an IPv6 literal.
    pub host: String,
    /// As written: `repo.git`, `/srv/repo.git`, `~/repo.git`.
    pub path: String,
}

impl Url {
    /// The remote host, when there is one; a local path has none.
    #[must_use]
    pub fn host(&self) -> Option<&str> {
        match self {
            Self::Scheme(url) => url.host_str(),
            Self::Scp(scp) => Some(&scp.host),
            Self::Local(_) => None,
        }
    }

    /// The path part: after the host in the remote spellings, the
    /// whole thing in the local one.
    #[must_use]
    pub fn path(&self) -> Cow<'_, str> {
        match self {
            Self::Scheme(url) => url.path().into(),
            Self::Scp(scp) => scp.path.as_str().into(),
            Self::Local(path) => path.to_string_lossy(),
        }
    }
}

impl FromStr for Url {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> Result<Self> {
        if is_local(text) {
            return Ok(Self::Local(PathBuf::from(text)));
        }
        if has_scheme(text) {
            let url = url::Url::parse(text)?;
            refuse_options(url.host_str().unwrap_or_default(), url.path())?;
            return Ok(Self::Scheme(url));
        }
        Scp::from_str(text).map(Self::Scp)
    }
}

impl FromStr for Scp {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> Result<Self> {
        // An IPv6 literal's colons are inside its brackets, after any
        // `user@`; the colon that ends the host comes after them.
        let bracket = match text.strip_prefix('[') {
            Some(_) => Some(0),
            None => text.find("@[").map(|at| at + 1),
        };
        let host_end = match bracket {
            Some(open) => match text[open..].find(']') {
                Some(close) => open + close + 1,
                None => bail!("unclosed bracket in {text:?}"),
            },
            None => 0,
        };
        let Some(colon) = text[host_end..].find(':').map(|at| host_end + at) else {
            bail!("no path in {text:?}");
        };
        let (user, host) = match text[..colon].rsplit_once('@') {
            Some((user, host)) => (Some(user), host),
            None => (None, &text[..colon]),
        };
        let path = &text[colon + 1..];
        if host.is_empty() || path.is_empty() || user.is_some_and(str::is_empty) {
            bail!("not [user@]host:path: {text:?}");
        }
        refuse_options(host, path)?;
        Ok(Self {
            user: user.map(str::to_owned),
            host: host.to_owned(),
            path: path.to_owned(),
        })
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scheme(url) => f.write_str(url.as_str()),
            Self::Scp(scp) => scp.fmt(f),
            Self::Local(path) => path.display().fmt(f),
        }
    }
}

impl fmt::Display for Scp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(user) = &self.user {
            write!(f, "{user}@")?;
        }
        write!(f, "{}:{}", self.host, self.path)
    }
}

// On the wire a URL is its spelling, and it comes back through the
// same door as any other string.
impl Serialize for Url {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Url {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Git's `url_is_local_not_ssh`: no colon, or a slash before the first
/// one — and on Windows, a drive letter.
fn is_local(text: &str) -> bool {
    match (text.find(':'), text.find('/')) {
        (None, _) => true,
        (Some(colon), Some(slash)) if slash < colon => true,
        (Some(colon), _) => cfg!(windows) && colon == 1 && text.as_bytes()[0].is_ascii_alphabetic(),
    }
}

/// Git's `is_url`: a scheme — a letter, then letters, digits, `+`, `.`,
/// `-` — followed by `://`.
fn has_scheme(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Git's `looks_like_command_line_option`, applied where git applies
/// it: a host or path that ssh would read as a flag is refused.
fn refuse_options(host: &str, path: &str) -> Result<()> {
    if host.starts_with('-') {
        bail!("strange hostname {host:?} blocked");
    }
    if path.starts_with('-') {
        bail!("strange pathname {path:?} blocked");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Url {
        text.parse()
            .unwrap_or_else(|error| panic!("{text:?}: {error}"))
    }

    fn scp(user: Option<&str>, host: &str, path: &str) -> Url {
        Url::Scp(Scp {
            user: user.map(str::to_owned),
            host: host.to_owned(),
            path: path.to_owned(),
        })
    }

    #[test]
    fn a_url_proper_is_read_by_its_scheme() {
        for text in [
            "https://github.com/owner/repo.git",
            "ssh://git@github.com:22/owner/repo.git",
            "git://host.xz/path/to/repo.git/",
            "ftp://host.xz/repo",
            "file:///srv/git/repo.git",
            "git+ssh://host/repo",
            "hg-remote://host/repo",
        ] {
            let Url::Scheme(url) = parse(text) else {
                panic!("{text:?} should be a URL proper");
            };
            assert_eq!(url.as_str(), text);
        }
    }

    #[test]
    fn the_scp_like_spelling_comes_apart() {
        assert_eq!(
            parse("git@github.com:owner/repo.git"),
            scp(Some("git"), "github.com", "owner/repo.git")
        );
        assert_eq!(parse("host.xz:repo"), scp(None, "host.xz", "repo"));
        assert_eq!(
            parse("host.xz:/srv/repo.git"),
            scp(None, "host.xz", "/srv/repo.git")
        );
        assert_eq!(
            parse("me@host.xz:~/repo.git"),
            scp(Some("me"), "host.xz", "~/repo.git")
        );
        assert_eq!(parse("[::1]:repo"), scp(None, "[::1]", "repo"));
        assert_eq!(
            parse("git@[fe80::1%25eth0]:owner/repo"),
            scp(Some("git"), "[fe80::1%25eth0]", "owner/repo")
        );
    }

    #[test]
    fn a_path_is_local_by_gits_rule() {
        for text in [
            "/srv/git/repo.git",
            "../repo",
            "repo",
            "~/repo",
            "dir/with:colon",
            "./host:looks-like-scp",
        ] {
            assert_eq!(parse(text), Url::Local(PathBuf::from(text)), "{text:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_letter_is_a_local_path() {
        assert_eq!(parse(r"C:\repo"), Url::Local(PathBuf::from(r"C:\repo")));
        assert_eq!(parse("D:repo"), Url::Local(PathBuf::from("D:repo")));
    }

    #[test]
    fn a_scheme_needs_a_scheme_character_set() {
        // Outside git's scheme alphabet, so read as scp-like, with host
        // `weird_scheme` — as git reads it.
        assert_eq!(
            parse("weird_scheme://host/repo"),
            scp(None, "weird_scheme", "//host/repo")
        );
        assert_eq!(parse("1st://host/repo"), scp(None, "1st", "//host/repo"));
    }

    #[test]
    fn the_host_and_path_are_reachable_across_spellings() {
        let url = parse("https://GitHub.com/owner/repo.git");
        assert_eq!(url.host(), Some("github.com"), "normalised by url");
        assert_eq!(url.path(), "/owner/repo.git");

        let url = parse("git@github.com:owner/repo.git");
        assert_eq!(url.host(), Some("github.com"));
        assert_eq!(url.path(), "owner/repo.git");

        let url = parse("/srv/git/repo.git");
        assert_eq!(url.host(), None);
        assert_eq!(url.path(), "/srv/git/repo.git");
    }

    #[test]
    fn the_remote_spellings_display_as_written_and_a_url_as_normalised() {
        for text in [
            "git@github.com:owner/repo.git",
            "host:repo",
            "[::1]:repo",
            "/srv/git/repo.git",
            "dir/with:colon",
            "ssh://git@github.com/owner/repo.git",
        ] {
            assert_eq!(parse(text).to_string(), text);
        }
        assert_eq!(
            parse("HTTPS://GitHub.com:443/owner/repo.git").to_string(),
            "https://github.com/owner/repo.git"
        );
    }

    #[test]
    fn a_malformed_scp_spelling_is_refused() {
        for text in ["host:", ":repo", "@host:repo", "[::1:repo", "[::1]repo"] {
            assert!(text.parse::<Url>().is_err(), "{text:?}");
        }
    }

    #[test]
    fn a_host_or_path_that_reads_as_an_option_is_refused_as_git_refuses_it() {
        for text in [
            "-oProxyCommand=touch,pwned:repo",
            "host:-oProxyCommand=x",
            "ssh://-oProxyCommand=x/repo",
        ] {
            let error = text.parse::<Url>().expect_err(text).to_string();
            assert!(error.contains("blocked"), "{text:?}: {error}");
        }
    }

    /// A URL's path begins with `/`, so git passes it, and so does this.
    #[test]
    fn a_url_path_that_begins_with_a_dash_after_the_slash_is_fine() {
        assert_eq!(parse("ssh://host/-x").path(), "/-x");
    }

    #[test]
    fn a_url_crosses_the_wire_as_its_spelling_and_comes_back_itself() {
        for text in [
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo.git",
            "/srv/git/repo.git",
        ] {
            let url = parse(text);
            let wire = serde_json::to_string(&url).unwrap();
            assert_eq!(wire, format!("{text:?}"));
            assert_eq!(serde_json::from_str::<Url>(&wire).unwrap(), url);
        }
        let error = serde_json::from_str::<Url>(r#""host:-x""#).unwrap_err();
        assert!(error.to_string().contains("blocked"), "{error}");
    }

    #[test]
    fn a_url_proper_the_url_crate_refuses_is_an_error() {
        assert!("https://".parse::<Url>().is_err());
    }
}
