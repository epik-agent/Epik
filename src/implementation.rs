use anyhow::Result;

use crate::Persona;
use crate::repository::{Endpoint, Repository};
use crate::tree::Tree;

pub struct Feature {
    pub repository: Repository,
    pub issues: Tree<Issue>,
    pub reviewer: Option<Persona>,
}

pub struct Issue {
    pub id: u32,
    pub description: String,
}

impl Issue {
    pub fn new(id: u32, description: impl Into<String>) -> Self {
        Issue {
            id,
            description: description.into(),
        }
    }
}

pub trait Implementable {
    fn implement(&self, source: &Endpoint, dest: &Endpoint) -> Result<()>;
}
