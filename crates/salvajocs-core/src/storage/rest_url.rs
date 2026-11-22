//! The password in the URL of a `rest:` repository (ADR-0009). It is kept apart from the URL, as
//! the secret option [`REST_PASSWORD_KEY`], so it never reaches the config file or a keyring
//! account, and put back in the URL only where rustic is given it.

/// The key of the secret option that holds the password of a `rest:` URL, stored percent-decoded.
pub const REST_PASSWORD_KEY: &str = "rest-password";

/// The parts of a `rest:` URL: what comes before the user info, the user info and the rest.
struct Parts<'a> {
    /// `rest:scheme://`.
    head: &'a str,
    /// The user info (`user` or `user:password`), when the URL has an `@` in its authority.
    userinfo: Option<&'a str>,
    /// The host, the port and the path.
    tail: &'a str,
}

/// Splits `repository` after `rest:` and `scheme://`; `None` for anything else. The authority
/// ends at the first `/`, `?` or `#`, and its user info at the last `@` in it.
fn parts(repository: &str) -> Option<Parts<'_>> {
    let url = repository.strip_prefix("rest:")?;
    let head_len = "rest:".len() + url.find("://")? + "://".len();
    let after = &repository[head_len..];
    let authority_len = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (userinfo, tail) = match after[..authority_len].rfind('@') {
        Some(at) => (Some(&after[..at]), &after[at + 1..]),
        None => (None, after),
    };
    Some(Parts {
        head: &repository[..head_len],
        userinfo,
        tail,
    })
}

/// Splits the password out of a `rest:` URL: the URL without it (the user name stays) and the
/// percent-decoded password. `None` when the URL has no password, or one that is not UTF-8 once
/// decoded.
pub fn split_rest_password(repository: &str) -> Option<(String, String)> {
    let parts = parts(repository)?;
    let (user, password) = parts.userinfo?.split_once(':')?;
    if password.is_empty() {
        return None;
    }
    let password = decode(password)?;
    let user = if user.is_empty() {
        String::new()
    } else {
        format!("{user}@")
    };
    Some((format!("{}{user}{}", parts.head, parts.tail), password))
}

/// `repository` with `password` in its user info, percent-encoded; a URL that is not a `rest:`
/// URL comes back as it is. A password already in the URL is replaced.
pub fn join_rest_password(repository: &str, password: &str) -> String {
    let Some(parts) = parts(repository) else {
        return repository.to_string();
    };
    let user = parts.userinfo.map_or("", |userinfo| {
        userinfo.split(':').next().unwrap_or_default()
    });
    format!("{}{user}:{}@{}", parts.head, encode(password), parts.tail)
}

/// `repository` with the password of its URL replaced by `***`, for output. Any other
/// repository string comes back as it is.
pub fn redact_rest_password(repository: &str) -> String {
    match parts(repository) {
        Some(Parts {
            head,
            userinfo: Some(userinfo),
            tail,
        }) if userinfo.contains(':') => {
            let user = userinfo.split(':').next().unwrap_or_default();
            format!("{head}{user}:***@{tail}")
        }
        _ => repository.to_string(),
    }
}

/// `text` with the percent-encoded `password` replaced by `***`, for an error message that quotes
/// the URL rustic was given.
pub(super) fn mask_password(text: &str, password: &str) -> String {
    match encode(password).as_str() {
        "" => text.to_string(),
        encoded => text.replace(encoded, "***"),
    }
}

/// Percent-encodes every byte but the unreserved ones, so the password cannot end the user info.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            byte => format!("%{byte:02X}"),
        })
        .collect()
}

/// Undoes percent-encoding; `None` when the result is not UTF-8. A `%` not followed by two hex
/// digits stays as it is, as in a URL parser.
fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(repository: &str) -> Option<(String, String)> {
        split_rest_password(repository)
    }

    #[test]
    fn the_password_leaves_the_url_and_the_user_stays() {
        assert_eq!(
            split("rest:https://user:pass@host:8000/repo?x=1"),
            Some(("rest:https://user@host:8000/repo?x=1".into(), "pass".into()))
        );
        assert_eq!(
            split("rest:http://:pass@host/"),
            Some(("rest:http://host/".into(), "pass".into()))
        );
    }

    #[test]
    fn a_url_without_a_password_is_not_split() {
        for url in [
            "rest:https://host:8000/repo",
            "rest:https://user@host/repo",
            "rest:https://user:@host/repo",
            "rest:https://host/a:b@c",
            "rest:[::1]:8000",
            "rest:https://[::1]:8000/repo",
            "/backups/user:pass@host",
            "rclone:remote:user:pass@x",
            "rest:no-scheme-user:pass@host",
        ] {
            assert_eq!(split(url), None, "{url}");
        }
    }

    #[test]
    fn an_ipv6_host_keeps_its_port_and_a_password_may_hold_at_and_colon() {
        assert_eq!(
            split("rest:https://u:p%40ss:w%3Ard@[::1]:8000/r"),
            Some(("rest:https://u@[::1]:8000/r".into(), "p@ss:w:rd".into()))
        );
        // A raw `@` in the password: the user info ends at the last `@` of the authority.
        assert_eq!(
            split("rest:https://u:p@ss@host/r"),
            Some(("rest:https://u@host/r".into(), "p@ss".into()))
        );
    }

    #[test]
    fn a_password_that_is_not_utf8_is_left_in_the_url() {
        assert_eq!(split("rest:https://u:%FF%FE@host/"), None);
    }

    #[test]
    fn join_puts_the_encoded_password_back_and_split_undoes_it() {
        for password in ["pass", "p@ss:w/rd#?%41 é", "~-._"] {
            for url in [
                "rest:https://user@host:8000/repo",
                "rest:https://host/repo",
                "rest:https://[::1]:8000/",
            ] {
                let joined = join_rest_password(url, password);
                let (stripped, found) = split(&joined).expect("a password is in it");
                assert_eq!(
                    (stripped.as_str(), found.as_str()),
                    (url, password),
                    "{joined}"
                );
            }
        }
        assert_eq!(
            join_rest_password("rest:https://user@host/", "p@ss"),
            "rest:https://user:p%40ss@host/"
        );
        assert_eq!(
            join_rest_password("rest:https://user:old@host/", "new"),
            "rest:https://user:new@host/"
        );
        assert_eq!(join_rest_password("/backups", "x"), "/backups");
    }

    #[test]
    fn redact_hides_only_the_password() {
        assert_eq!(
            redact_rest_password("rest:https://user:pass@host/r"),
            "rest:https://user:***@host/r"
        );
        for url in ["rest:https://user@host/", "/backups", "opendal:s3"] {
            assert_eq!(redact_rest_password(url), url);
        }
    }
}
