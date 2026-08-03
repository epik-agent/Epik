use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::Persona;
use crate::logging::Log;
use crate::repository::{Endpoint, Repository};
use crate::tree::Tree;

#[derive(Debug)]
pub struct Feature<I> {
    pub repository: Repository,
    pub issues: Tree<I>,
    pub reviewer: Option<Persona>,
}

impl<I: Implementable> Implementable for Feature<I> {
    /// Implements every issue in the feature, parents before children.
    fn implement(&self, source: &Endpoint, dest: &Endpoint, log: &mut dyn Log) -> Result<()> {
        let mut order = Vec::new();
        self.issues.bfs(|issue| order.push(issue));
        for issue in order {
            issue.implement(source, dest, log)?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
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
    fn implement(&self, source: &Endpoint, dest: &Endpoint, log: &mut dyn Log) -> Result<()>;
}
