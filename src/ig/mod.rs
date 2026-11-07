//! The only module that talks HTTP to Instagram (and its CDN). Everything else
//! goes through the `InstagramApi` trait.

pub mod api;
pub mod http;
pub mod types;
pub mod wire;

#[cfg(test)]
pub mod fake;

pub use api::{IgResult, InstagramApi};
pub use http::HttpInstagram;
