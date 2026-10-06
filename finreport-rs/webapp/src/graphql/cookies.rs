//! `fr_session` cookie formatting/parsing (§4). Pure string helpers — no DB,
//! no auth logic — so they sit in the GraphQL/HTTP layer rather than
//! `crate::auth`, which stays actix-free.

/// Builds the `Set-Cookie` header value for a fresh session (§4): `fr_session`,
/// `HttpOnly`, `SameSite=Lax`, `Path=/`, `Max-Age` = TTL, `Secure` controlled
/// by `APP_cookie_secure`.
pub fn cookie_header(raw_token: &str, ttl_days: i64, secure: bool) -> String {
    let max_age = ttl_days * 24 * 60 * 60;
    let secure_flag = if secure { "; Secure" } else { "" };
    format!("fr_session={raw_token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure_flag}")
}

/// Builds the `Set-Cookie` header value that clears the session cookie on
/// logout.
pub fn clear_cookie_header(secure: bool) -> String {
    let secure_flag = if secure { "; Secure" } else { "" };
    format!("fr_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{secure_flag}")
}

/// Reads `fr_session` out of a raw `Cookie` request header value.
pub fn extract_token(cookie_header: &str) -> Option<String> {
    cookie_header.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix("fr_session=").map(|v| v.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_header_carries_the_configured_ttl_and_secure_flag() {
        let header = cookie_header("tok", 30, true);
        assert!(header.contains("fr_session=tok"));
        assert!(header.contains("Max-Age=2592000"));
        assert!(header.contains("Secure"));
        assert!(header.contains("HttpOnly"));
        assert!(header.contains("SameSite=Lax"));

        let insecure = cookie_header("tok", 30, false);
        assert!(!insecure.contains("Secure"));
    }

    #[test]
    fn extract_token_reads_the_cookie_among_others() {
        assert_eq!(
            extract_token("foo=bar; fr_session=abc123; other=1"),
            Some("abc123".to_string())
        );
        assert_eq!(extract_token("foo=bar"), None);
    }
}
