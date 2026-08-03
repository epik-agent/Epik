use anyhow::Result;

use crate::Persona;
use crate::repository::{Endpoint, Repository};
use crate::tree::Tree;

#[derive(Debug)]
pub struct Feature {
    pub repository: Repository,
    pub issues: Tree<Issue>,
    pub reviewer: Option<Persona>,
}

#[derive(Debug)]
pub struct Issue {
    pub id: u32,
    pub description: String,
}

impl Issue {
    #[must_use]
    pub fn new(id: u32, description: impl Into<String>) -> Self {
        Self {
            id,
            description: description.into(),
        }
    }
}

pub trait Implementable {
    /// Makes this thing real: work flows from the source endpoint to the
    /// destination endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying repository operations fail.
    fn implement(&self, source: &Endpoint, dest: &Endpoint) -> Result<()>;
}
