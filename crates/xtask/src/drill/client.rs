//! One signed-in browser, more or less: a cookie jar and JSON calls, in
//! plain HTTP to the drill's servers on loopback.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};
use ureq::http::{Method, Request, header};

use super::DrillError;
use super::account::Account;

/// How long a connection, a write or a read may stall.
const STALL: Duration = Duration::from_secs(60);

/// A client of one server, with the cookies it has set.
pub struct Client {
    agent: ureq::Agent,
    base: String,
    cookies: BTreeMap<String, String>,
}

impl Client {
    /// A client with no cookies, for `base` (`http://127.0.0.1:7811`).
    #[must_use]
    pub fn new(base: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            // The drill's servers are on loopback: a proxy from the
            // environment never applies.
            .proxy(None)
            // Every status comes back as a response, and the API never
            // redirects: a redirect is a status like any other.
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_connect(Some(STALL))
            .timeout_per_write(Some(STALL))
            .timeout_per_read(Some(STALL))
            .build()
            .new_agent();
        Self {
            agent,
            base: base.trim_end_matches('/').to_owned(),
            cookies: BTreeMap::new(),
        }
    }

    /// A client for `base`, signed in as `account`.
    pub fn sign_in(base: &str, account: &Account) -> Result<Self, DrillError> {
        let mut client = Self::new(base);
        client.call(
            &Method::POST,
            "/api/auth/login",
            Some(&json!({ "username": account.username, "password": account.passphrase })),
        )?;
        Ok(client)
    }

    /// `METHOD path`, with `body` as JSON: the JSON it answers, or `null`
    /// for an empty answer. A status other than 2xx is an error that
    /// carries what the server said.
    pub fn call(
        &mut self,
        method: &Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, DrillError> {
        let bytes = self.bytes(method, path, body)?;
        json_of(method, path, &bytes)
    }

    /// `METHOD path`'s answer as bytes; a status other than 2xx is an error,
    /// as for [`Client::call`].
    pub fn bytes(
        &mut self,
        method: &Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Vec<u8>, DrillError> {
        let (status, bytes) = self.send(method, path, body)?;
        if !(200..300).contains(&status) {
            return Err(DrillError(format!(
                "{method} {path}: {status} {}",
                String::from_utf8_lossy(&bytes)
            )));
        }
        Ok(bytes)
    }

    /// `GET path`'s JSON, or `None` when the server answers with a status
    /// other than 2xx.
    pub fn get_or_miss(&mut self, path: &str) -> Result<Option<Value>, DrillError> {
        let (status, bytes) = self.send(&Method::GET, path, None)?;
        if !(200..300).contains(&status) {
            return Ok(None);
        }
        json_of(&Method::GET, path, &bytes).map(Some)
    }

    fn send(
        &mut self,
        method: &Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Vec<u8>), DrillError> {
        let failed = |e: &dyn std::fmt::Display| DrillError(format!("{method} {path}: {e}"));
        let mut request = Request::builder()
            .method(method.clone())
            .uri(format!("{}{path}", self.base));
        if !self.cookies.is_empty() {
            let pairs: Vec<String> = self
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            request = request.header(header::COOKIE, pairs.join("; "));
        }
        let sent = match body {
            Some(body) => {
                let data = serde_json::to_vec(body).map_err(|e| failed(&e))?;
                let request = request
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(data)
                    .map_err(|e| failed(&e))?;
                self.agent.run(request)
            }
            None => self.agent.run(request.body(()).map_err(|e| failed(&e))?),
        };
        let mut response = sent.map_err(|e| failed(&e))?;
        for line in response.headers().get_all(header::SET_COOKIE) {
            if let Ok(line) = line.to_str() {
                set_cookie(&mut self.cookies, line);
            }
        }
        let status = response.status().as_u16();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(u64::MAX)
            .read_to_vec()
            .map_err(|e| failed(&e))?;
        Ok((status, bytes))
    }
}

fn json_of(method: &Method, path: &str, bytes: &[u8]) -> Result<Value, DrillError> {
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(bytes)
        .map_err(|e| DrillError(format!("{method} {path}: the answer isn't JSON: {e}")))
}

/// Keeps what one `Set-Cookie` line sets, or forgets the cookie it clears
/// (an empty value, or a `Max-Age` of zero or less). Every cookie goes back
/// to the one server it came from, so its path and domain don't matter.
fn set_cookie(jar: &mut BTreeMap<String, String>, line: &str) {
    let mut parts = line.split(';');
    let Some((name, value)) = parts.next().and_then(|pair| pair.split_once('=')) else {
        return;
    };
    let (name, value) = (name.trim(), value.trim());
    let cleared = parts.any(|attribute| {
        attribute.split_once('=').is_some_and(|(key, age)| {
            key.trim().eq_ignore_ascii_case("max-age")
                && age.trim().parse::<i64>().is_ok_and(|age| age <= 0)
        })
    });
    if value.is_empty() || cleared {
        jar.remove(name);
    } else {
        jar.insert(name.to_owned(), value.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::set_cookie;

    #[test]
    fn a_cookie_is_kept_until_it_is_cleared() {
        let mut jar = BTreeMap::new();
        set_cookie(
            &mut jar,
            "consolebook_session=abc; Path=/; HttpOnly; SameSite=Strict",
        );
        set_cookie(&mut jar, "other = x=y ; Max-Age=60");
        assert_eq!(
            jar.get("consolebook_session").map(String::as_str),
            Some("abc")
        );
        assert_eq!(jar.get("other").map(String::as_str), Some("x=y"));
        set_cookie(&mut jar, "consolebook_session=def");
        assert_eq!(
            jar.get("consolebook_session").map(String::as_str),
            Some("def")
        );
        set_cookie(&mut jar, "consolebook_session=; Path=/");
        assert!(!jar.contains_key("consolebook_session"));
        set_cookie(&mut jar, "other=x; max-age=0");
        assert!(jar.is_empty());
        set_cookie(&mut jar, "not a cookie");
        assert!(jar.is_empty());
    }
}
