//! GitHub as a forge: where a repository's refs are, and the token
//! that writes them.

use super::{Credentials, Forge};
use crate::github::Repo;
use crate::keystore::Secret;

/// GitHub as a forge: one repository, written over HTTPS as the
/// `x-access-token` user with the token from the keystore. The API
/// client in [`github`](crate::github) is the same system's other face;
/// this one only knows where the refs are.
#[derive(Clone, Debug)]
pub struct GitHub {
    pub repo: Repo,
    pub token: Secret,
}

impl Forge for GitHub {
    fn remote(&self) -> String {
        format!("https://github.com/{}.git", self.repo)
    }

    fn credentials(&self) -> Option<Credentials> {
        Some(Credentials {
            username: "x-access-token".to_owned(),
            secret: self.token.clone(),
        })
    }
}
