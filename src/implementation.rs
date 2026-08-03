use crate::Persona;
use crate::repository::{Endpoint, Repository};
use crate::tree::Tree;

pub struct Feature {
    repository: Repository,
    issues: Tree<Issue>,
    reviewer: Option<Persona>,
}

pub struct Issue {
    id: u32,
    description: String,
}

trait Implementer {
    fn implement(&self, source: Endpoint, dest: Endpoint);
}