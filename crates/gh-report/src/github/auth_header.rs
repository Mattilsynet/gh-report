//! Opaque wrapper for the outbound `Authorization` header value.
//!
//! SEC-0007:R2 requires secret values to be held in opaque wrapper types at
//! the type level rather than relying on no call site formatting them today.
//! [`AuthHeader`] has no [`std::fmt::Display`] impl and its [`std::fmt::Debug`]
//! impl is redacting, so the credential has no formatting escape path. The
//! only egress is [`AuthHeader::apply`], which attaches the value to an
//! outbound request. The wrapped value is marked sensitive at construction, so
//! the credential stays redacted after it leaves the wrapper: `HeaderValue`
//! renders as `Sensitive` wherever a `HeaderMap` is formatted, including the
//! `RequestBuilder` and `Request` returned from the attachment site.

use std::fmt;

use reqwest::RequestBuilder;
use reqwest::header::{AUTHORIZATION, HeaderValue};

const REDACTED: &str = "[REDACTED]";

pub(crate) struct AuthHeader(HeaderValue);

impl AuthHeader {
    pub(crate) fn new(mut value: HeaderValue) -> Self {
        value.set_sensitive(true);
        Self(value)
    }

    pub(crate) fn apply(&self, builder: RequestBuilder) -> RequestBuilder {
        builder.header(AUTHORIZATION, self.0.clone())
    }
}

impl fmt::Debug for AuthHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AuthHeader").field(&REDACTED).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "ghs_supersecrettokenvalue";

    #[test]
    fn debug_output_contains_no_token_substring() {
        let header =
            AuthHeader::new(HeaderValue::from_str(&format!("Bearer {TOKEN}")).expect("valid"));

        let rendered = format!("{header:?}");

        assert!(!rendered.contains(TOKEN), "token leaked: {rendered}");
        assert!(!rendered.contains("Bearer"), "scheme leaked: {rendered}");
        assert!(rendered.contains(REDACTED), "not redacted: {rendered}");
    }

    #[test]
    fn alternate_debug_output_contains_no_token_substring() {
        let header =
            AuthHeader::new(HeaderValue::from_str(&format!("Bearer {TOKEN}")).expect("valid"));

        let rendered = format!("{header:#?}");

        assert!(!rendered.contains(TOKEN), "token leaked: {rendered}");
    }

    fn applied_request() -> reqwest::Request {
        let header =
            AuthHeader::new(HeaderValue::from_str(&format!("Bearer {TOKEN}")).expect("valid"));
        let client = reqwest::Client::new();

        header
            .apply(client.get("https://api.github.com/meta"))
            .build()
            .expect("request builds")
    }

    #[test]
    fn debug_of_applied_request_contains_no_token_substring() {
        let rendered = format!("{:?}", applied_request());

        assert!(!rendered.contains(TOKEN), "token leaked: {rendered}");
    }

    #[test]
    fn alternate_debug_of_applied_request_contains_no_token_substring() {
        let rendered = format!("{:#?}", applied_request());

        assert!(!rendered.contains(TOKEN), "token leaked: {rendered}");
    }

    #[test]
    fn debug_of_applied_request_headers_contains_no_token_substring() {
        let request = applied_request();

        let rendered = format!("{:?}", request.headers());

        assert!(!rendered.contains(TOKEN), "token leaked: {rendered}");
    }

    #[test]
    fn applied_header_value_is_marked_sensitive() {
        let request = applied_request();

        let value = request
            .headers()
            .get(AUTHORIZATION)
            .expect("authorization header attached");

        assert!(value.is_sensitive(), "header value not marked sensitive");
    }
}
