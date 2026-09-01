//! GitHub API client, DTOs, pagination, and rate-limit handling.

pub mod auth;
pub(crate) mod auth_header;
pub mod budget;
pub mod client;
pub mod dto;
pub mod pagination;
pub mod rate_limit;
pub(crate) mod route_template;
