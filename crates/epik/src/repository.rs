use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::implementation::{Feature, Issue};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Url(pub PathBuf);

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
