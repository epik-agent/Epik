use std::path::PathBuf;

use crate::implementation::{Feature, Issue};

/// Where a repository lives: a working copy on the local filesystem, or a
/// hosted repository reached over the network. Everything downstream that
/// cares about the distinction dispatches on this.
pub enum Url {
    Local(PathBuf),
    Remote(String),
}

impl Url {
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Url::Local(path.into())
    }

    pub fn remote(url: impl Into<String>) -> Self {
        Url::Remote(url.into())
    }
}

pub struct Branch(String);

impl Branch {
    pub fn new(name: impl Into<String>) -> Self {
        Branch(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct Endpoint {
    url: Url,
    branch: Branch,
}

impl Endpoint {
    pub fn new(url: Url, branch: Branch) -> Self {
        Endpoint { url, branch }
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn branch(&self) -> &Branch {
        &self.branch
    }
}

pub struct Repository {
    url: Url,
}

impl Repository {
    pub fn new(url: Url) -> Self {
        Repository { url }
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn feature(&self, _id: u32) -> Feature {
        todo!()
    }

    pub fn issue(&self, _id: u32) -> Issue {
        todo!()
    }
}
