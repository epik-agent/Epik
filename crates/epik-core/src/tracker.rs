#![allow(dead_code)]
use crate::dag::Dag;
use anyhow::Result;

/// An ['IssueTracker'] tracks work on individual coding issues.
trait IssueTracker<S> {
    fn content(id: IssueId) -> Result<String>;
    fn get_status(id: IssueId) -> Result<S>;
    fn set_status(id: IssueId, status: S) -> Result<()>;
    fn dependents(id: IssueId) -> Result<Dag<IssueId>>;
}

type IssueId = String;
