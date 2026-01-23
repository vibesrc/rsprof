//! Shared types.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Route {
    Search,
    Checkout,
    Analytics,
    Health,
}

pub struct Request {
    pub id: u64,
    pub user_id: u64,
    pub session_id: u64,
    pub key: String,
    pub payload: Vec<u8>,
    pub route: Route,
    pub flags: u8,
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub cacheable: bool,
}

pub struct Stats {
    pub requests: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub errors: u64,
}
