pub mod event;
pub mod implementation;
pub mod logging;
pub mod repository;
pub mod tree;

#[derive(Debug)]
pub enum Persona {
    Human,
    Epik,
}
