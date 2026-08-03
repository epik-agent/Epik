use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::implementation::{Feature, Issue};

/// Where a repository lives: a working copy on the local filesystem, or a
/// hosted repository reached over the network. Everything downstream that
/// cares about the distinction dispatches on this.
#[derive(Debug, Deserialize, Serialize)]
pub enum Url {
    Local(PathBuf),
    Remote(String),
}

impl Url {
    #[must_use]
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local(path.into())
    }

    #[must_use]
    pub fn remote(url: impl Into<String>) -> Self {
        Self::Remote(url.into())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Branch(String);

impl Branch {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Endpoint {
    url: Url,
    branch: Branch,
}

impl Endpoint {
    #[must_use]
    pub const fn new(url: Url, branch: Branch) -> Self {
        Self { url, branch }
    }

    #[must_use]
    pub const fn url(&self) -> &Url {
        &self.url
    }

    #[must_use]
    pub const fn branch(&self) -> &Branch {
        &self.branch
    }
}

#[derive(Debug)]
pub struct Repository {
    url: Url,
}

impl Repository {
    #[must_use]
    pub const fn new(url: Url) -> Self {
        Self { url }
    }

    #[must_use]
    pub const fn url(&self) -> &Url {
        &self.url
    }

    #[must_use]
    pub fn feature(&self, _id: u32) -> Feature<Issue> {
        todo!()
    }

    #[must_use]
    pub fn issue(&self, _id: u32) -> Issue {
        todo!()
    }
}
