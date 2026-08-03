use crate::implementation::{Feature, Issue};

struct Url(String);

struct Branch(String);

pub struct Endpoint {
    url: Url,
    branch: Branch,
}

pub struct Repository {
    url: Url,
}

impl Repository {
    fn feature(id: u32) -> Feature {
        todo!()
    }
    fn issue(id: u32) -> Issue {
        todo!()
    }
}
