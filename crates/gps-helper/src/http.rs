//! A blocking HTTP client with retries, rate limiting, and a polite User-Agent.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// The default `User-Agent`. Public OSM services ask clients to identify
/// themselves, so tools built on this crate should set their own.
pub const DEFAULT_USER_AGENT: &str = concat!(
    "gps-helper/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/gps-helper; OSM map-matching client)"
);

const RETRY_STATUSES: [u16; 5] = [429, 500, 502, 503, 504];
const MAX_BACKOFF: Duration = Duration::from_secs(120);
const MAX_RESPONSE_BYTES: u64 = 1 << 30;
const MAX_REPORTED_URL_CHARS: usize = 200;
const MAX_REPORTED_BODY_CHARS: usize = 500;

/// Settings for an [`HttpClient`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct HttpConfig {
    /// The `User-Agent` header to send.
    pub user_agent: String,
    /// Timeout for each phase of a request: connecting, waiting for the
    /// response, and receiving the body.
    pub timeout: Duration,
    /// Maximum requests per second. Zero or less means no limit. The default
    /// of 1.0 respects the public services the backends point at by default.
    pub rate_limit: f64,
    /// How many times to retry after a connection failure or a 429/5xx status.
    pub max_retries: u32,
    /// Base delay for exponential backoff. The first retry is immediate, and
    /// later ones wait this long times 2, 4, 8 and so on, up to two minutes.
    /// A `Retry-After` header takes precedence.
    pub backoff_factor: Duration,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            timeout: Duration::from_secs(30),
            rate_limit: 1.0,
            max_retries: 5,
            backoff_factor: Duration::from_secs(1),
        }
    }
}

/// Spaces out requests to at most a set rate.
#[derive(Debug)]
struct RateLimiter {
    min_interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl RateLimiter {
    fn new(rate: f64) -> Result<Self> {
        let min_interval = if rate > 0.0 {
            Duration::try_from_secs_f64(rate.recip())
                .map_err(|_| Error::InvalidOption(format!("rate limit {rate} is out of range")))?
        } else {
            Duration::ZERO
        };
        Ok(Self {
            min_interval,
            last: Mutex::new(None),
        })
    }

    fn wait(&self) {
        if self.min_interval.is_zero() {
            return;
        }
        // Holding the lock while sleeping makes concurrent callers queue up.
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = *last {
            thread::sleep(self.min_interval.saturating_sub(previous.elapsed()));
        }
        *last = Some(Instant::now());
    }
}

/// A client shared by backends. Clones share one rate limit and connection pool.
#[derive(Debug, Clone)]
pub struct HttpClient {
    agent: ureq::Agent,
    rate_limiter: Arc<RateLimiter>,
    max_retries: u32,
    backoff_factor: Duration,
}

enum Attempt {
    Done(String),
    Failed {
        error: Error,
        retry_after: Option<Duration>,
    },
    Fatal(Error),
}

impl HttpClient {
    /// Creates a client from `config`.
    pub fn new(config: HttpConfig) -> Result<Self> {
        let timeout = Some(config.timeout);
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(config.user_agent)
            .timeout_connect(timeout)
            .timeout_recv_response(timeout)
            .timeout_recv_body(timeout)
            .build()
            .into();
        Ok(Self {
            agent,
            rate_limiter: Arc::new(RateLimiter::new(config.rate_limit)?),
            max_retries: config.max_retries,
            backoff_factor: config.backoff_factor,
        })
    }

    pub(crate) fn get(&self, url: &str, query: &[(&str, &str)]) -> Result<String> {
        self.with_retries(url, || {
            self.agent
                .get(url)
                .query_pairs(query.iter().copied())
                .call()
        })
    }

    pub(crate) fn post_json(&self, url: &str, body: &str) -> Result<String> {
        self.with_retries(url, || {
            self.agent
                .post(url)
                .content_type("application/json")
                .send(body)
        })
    }

    pub(crate) fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<String> {
        self.with_retries(url, || self.agent.post(url).send_form(form.iter().copied()))
    }

    fn with_retries(
        &self,
        url: &str,
        send: impl Fn() -> std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<String> {
        let mut retries = 0;
        loop {
            self.rate_limiter.wait();
            match attempt(url, send()) {
                Attempt::Done(body) => return Ok(body),
                Attempt::Fatal(error) => return Err(error),
                Attempt::Failed { error, .. } if retries >= self.max_retries => return Err(error),
                Attempt::Failed { retry_after, .. } => {
                    thread::sleep(retry_after.unwrap_or_else(|| self.backoff(retries)));
                    retries += 1;
                }
            }
        }
    }

    fn backoff(&self, retries: u32) -> Duration {
        if retries == 0 {
            return Duration::ZERO;
        }
        self.backoff_factor
            .saturating_mul(2u32.saturating_pow(retries))
            .min(MAX_BACKOFF)
    }
}

fn attempt(
    url: &str,
    response: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Attempt {
    let mut response = match response {
        Ok(response) => response,
        Err(error) => return transport_failure(url, error),
    };
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after);
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string();
    if response.status().is_success() {
        return match body {
            Ok(body) => Attempt::Done(body),
            Err(error) => transport_failure(url, error),
        };
    }
    let error = Error::HttpStatus {
        url: shorten(url, MAX_REPORTED_URL_CHARS),
        status,
        body: shorten(body.unwrap_or_default().trim(), MAX_REPORTED_BODY_CHARS),
    };
    if RETRY_STATUSES.contains(&status) {
        Attempt::Failed { error, retry_after }
    } else {
        Attempt::Fatal(error)
    }
}

fn transport_failure(url: &str, source: ureq::Error) -> Attempt {
    let retryable = matches!(
        source,
        ureq::Error::Io(_)
            | ureq::Error::Timeout(_)
            | ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
    );
    let error = Error::Transport {
        url: shorten(url, MAX_REPORTED_URL_CHARS),
        source: Box::new(source),
    };
    if retryable {
        Attempt::Failed {
            error,
            retry_after: None,
        }
    } else {
        Attempt::Fatal(error)
    }
}

/// Parses a `Retry-After` header: either delay seconds or an HTTP date.
fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let remaining = date.timestamp() - unix_now()?;
    Some(Duration::from_secs(remaining.max(0) as u64))
}

fn unix_now() -> Option<i64> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    i64::try_from(now.ok()?.as_secs()).ok()
}

fn shorten(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_seconds() {
        assert_eq!(parse_retry_after(" 7 "), Some(Duration::from_secs(7)));
    }

    #[test]
    fn retry_after_past_date_means_now() {
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn retry_after_garbage_is_ignored() {
        assert_eq!(parse_retry_after("soon"), None);
    }

    #[test]
    fn first_retry_is_immediate_then_doubles_up_to_the_cap() {
        let client = HttpClient::new(HttpConfig::default()).unwrap();
        let delays: Vec<_> = [0, 1, 2, 3, 30].map(|n| client.backoff(n).as_secs()).into();
        assert_eq!(delays, [0, 2, 4, 8, 120]);
    }

    #[test]
    fn extreme_rate_limit_is_rejected_not_a_panic() {
        assert!(RateLimiter::new(1e-320).is_err());
        assert!(RateLimiter::new(f64::NAN).unwrap().min_interval.is_zero());
        assert!(RateLimiter::new(-1.0).unwrap().min_interval.is_zero());
    }

    #[test]
    fn shorten_respects_char_boundaries() {
        assert_eq!(shorten("äöü", 2), "äö...");
        assert_eq!(shorten("äöü", 3), "äöü");
    }
}
