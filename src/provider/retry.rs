use std::time::{Duration, SystemTime};

use async_stream::try_stream;
use futures_util::{Stream, StreamExt};
use reqwest::{Response, StatusCode, header::RETRY_AFTER};

use crate::{
    error::ProviderError,
    types::{ProviderRetryEvent, ProviderRetryReason},
};

/// Default number of retries after a safely retryable HTTP request fails.
pub const DEFAULT_MAX_RETRIES: usize = 10;
/// Default deadline for connecting and receiving HTTP response headers.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Default maximum idle time between complete events on an established stream.
pub const DEFAULT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_millis(200);
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(10);
const DEFAULT_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(1);

/// Retry and timeout policy shared by all built-in HTTP providers.
///
/// `max_retries` counts retries after the initial request, so a value of `10`
/// permits at most eleven HTTP attempts for failures that are safe to retry.
/// Response-header timeouts are never retried because the server may already
/// have accepted the request. Set this value to zero to disable all retries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryConfig {
    max_retries: usize,
    request_timeout: Duration,
    stream_idle_timeout: Option<Duration>,
    initial_backoff: Duration,
    max_backoff: Duration,
    rate_limit_backoff: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            stream_idle_timeout: Some(DEFAULT_STREAM_IDLE_TIMEOUT),
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
            rate_limit_backoff: DEFAULT_RATE_LIMIT_BACKOFF,
        }
    }
}

impl RetryConfig {
    pub fn max_retries(&self) -> usize {
        self.max_retries
    }

    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    /// Returns the maximum time allowed between complete server-sent events.
    /// `None` disables the established-stream idle deadline.
    pub fn stream_idle_timeout(&self) -> Option<Duration> {
        self.stream_idle_timeout
    }

    pub fn initial_backoff(&self) -> Duration {
        self.initial_backoff
    }

    pub fn max_backoff(&self) -> Duration {
        self.max_backoff
    }

    pub fn rate_limit_backoff(&self) -> Duration {
        self.rate_limit_backoff
    }

    pub fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Sets the deadline for connecting and receiving HTTP response headers.
    ///
    /// A timeout is returned immediately and is never retried because receipt
    /// of the request by the server is ambiguous. The deadline intentionally
    /// does not cover an established event stream, because replaying a
    /// partially consumed stream can duplicate output and tool calls.
    pub fn with_request_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = request_timeout;
        self
    }

    /// Sets the maximum time to wait for the next complete event after HTTP
    /// response headers have arrived.
    ///
    /// Timing out an established stream is reported to the caller and is never
    /// retried, because replaying a partially consumed response can duplicate
    /// output or tool calls.
    pub fn with_stream_idle_timeout(mut self, stream_idle_timeout: Duration) -> Self {
        self.stream_idle_timeout = Some(stream_idle_timeout);
        self
    }

    /// Disables the established-stream idle deadline.
    pub fn without_stream_idle_timeout(mut self) -> Self {
        self.stream_idle_timeout = None;
        self
    }

    pub fn with_initial_backoff(mut self, initial_backoff: Duration) -> Self {
        self.initial_backoff = initial_backoff;
        self
    }

    pub fn with_max_backoff(mut self, max_backoff: Duration) -> Self {
        self.max_backoff = max_backoff;
        self
    }

    /// Sets the fixed fallback delay used for HTTP 429 without `Retry-After`.
    pub fn with_rate_limit_backoff(mut self, rate_limit_backoff: Duration) -> Self {
        self.rate_limit_backoff = rate_limit_backoff;
        self
    }

    fn exponential_delay(&self, retry_index: usize) -> Duration {
        let shift = retry_index.min(u32::BITS.saturating_sub(1) as usize) as u32;
        let multiplier = 1_u32.checked_shl(shift).unwrap_or(u32::MAX);
        let upper_bound = self
            .initial_backoff
            .saturating_mul(multiplier)
            .min(self.max_backoff);
        equal_jitter(upper_bound)
    }

    fn rate_limit_delay(&self, retry_after: Option<Duration>) -> Duration {
        retry_after
            .unwrap_or(self.rate_limit_backoff)
            .min(self.max_backoff)
    }
}

/// Waits for the next item in an established provider response stream while
/// enforcing the configured idle/read deadline. The caller owns protocol-level
/// completion checks and must not treat `Ok(None)` as successful completion
/// unless its terminal event has already been observed.
pub(crate) async fn next_stream_item<S>(
    stream: &mut S,
    idle_timeout: Option<Duration>,
) -> Result<Option<S::Item>, ProviderError>
where
    S: Stream + Unpin,
{
    match idle_timeout {
        Some(timeout) => tokio::time::timeout(timeout, stream.next())
            .await
            .map_err(|_| ProviderError::StreamIdleTimeout { timeout }),
        None => Ok(stream.next().await),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetryKind {
    Never,
    Exponential,
    RateLimit,
}

pub(crate) enum HttpRequestEvent {
    Retry(ProviderRetryEvent),
    Response(Response),
}

pub(crate) fn send_with_retry<F>(
    config: RetryConfig,
    make_request: F,
) -> impl Stream<Item = Result<HttpRequestEvent, ProviderError>> + Send
where
    F: Fn() -> reqwest::RequestBuilder + Send + Sync,
{
    try_stream! {
        let mut retries_used = 0;

        loop {
            let attempt = tokio::time::timeout(config.request_timeout, make_request().send()).await;
            let response = match attempt {
                Err(_) => {
                    Err(ProviderError::RequestTimeout {
                        timeout: config.request_timeout,
                    })?;
                    unreachable!("error propagation above exits the retry stream");
                }
                Ok(Err(error))
                    if retries_used < config.max_retries
                        && is_retryable_transport_error(&error) =>
                {
                    let delay = config.exponential_delay(retries_used);
                    yield HttpRequestEvent::Retry(ProviderRetryEvent {
                        retry_number: retries_used + 1,
                        max_retries: config.max_retries,
                        delay,
                        reason: ProviderRetryReason::Transport {
                            message: error.to_string(),
                        },
                    });
                    wait(delay).await;
                    retries_used += 1;
                    continue;
                }
                Ok(Err(error)) => {
                    Err(ProviderError::Http(error))?;
                    unreachable!("error propagation above exits the retry stream");
                }
                Ok(Ok(response)) => response,
            };

            if response.status().is_success() {
                yield HttpRequestEvent::Response(response);
                break;
            }

            let status = response.status();
            let retry_kind = retry_kind(status);
            let retry_after = parse_retry_after(&response);
            let body = read_error_body(response, config.request_timeout).await;

            if retry_kind == RetryKind::Never || retries_used >= config.max_retries {
                Err(ProviderError::from_api_response(status, body))?;
                unreachable!("error propagation above exits the retry stream");
            }

            let delay = match retry_kind {
                RetryKind::Exponential => config.exponential_delay(retries_used),
                RetryKind::RateLimit => config.rate_limit_delay(retry_after),
                RetryKind::Never => unreachable!("non-retryable statuses return above"),
            };
            yield HttpRequestEvent::Retry(ProviderRetryEvent {
                retry_number: retries_used + 1,
                max_retries: config.max_retries,
                delay,
                reason: ProviderRetryReason::HttpStatus {
                    status: status.as_u16(),
                    body,
                },
            });
            wait(delay).await;
            retries_used += 1;
        }
    }
}

fn retry_kind(status: StatusCode) -> RetryKind {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return RetryKind::RateLimit;
    }

    if matches!(
        status,
        StatusCode::REQUEST_TIMEOUT | StatusCode::CONFLICT | StatusCode::TOO_EARLY
    ) || (status.is_server_error()
        && !matches!(
            status,
            StatusCode::NOT_IMPLEMENTED | StatusCode::HTTP_VERSION_NOT_SUPPORTED
        ))
    {
        return RetryKind::Exponential;
    }

    RetryKind::Never
}

fn is_retryable_transport_error(error: &reqwest::Error) -> bool {
    // Once a request has reached an established connection, a transport error
    // cannot tell us whether the server processed the POST. Retrying that
    // ambiguous outcome can duplicate a generation and its charge. Connect
    // errors happen before a request can be delivered and are safe to retry.
    error.is_connect()
}

/// Keeps half of the exponential delay and randomizes the other half. This
/// preserves exponential growth while avoiding synchronized retry storms.
fn equal_jitter(upper_bound: Duration) -> Duration {
    let millis = u64::try_from(upper_bound.as_millis()).unwrap_or(u64::MAX);
    if millis <= 1 {
        return upper_bound;
    }
    Duration::from_millis(fastrand::u64((millis / 2)..=millis))
}

fn parse_retry_after(response: &Response) -> Option<Duration> {
    let value = response.headers().get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let retry_at = httpdate::parse_http_date(value).ok()?;
    Some(
        retry_at
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO),
    )
}

async fn read_error_body(response: Response, timeout: Duration) -> String {
    match tokio::time::timeout(timeout, response.text()).await {
        Ok(Ok(body)) => body,
        Ok(Err(error)) => format!("<failed to read provider error body: {error}>"),
        Err(_) => format!("<timed out reading provider error body after {timeout:?}>"),
    }
}

async fn wait(delay: Duration) {
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use futures_util::StreamExt;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::{JoinHandle, JoinSet},
    };

    use super::*;

    #[derive(Clone)]
    struct TestResponse {
        status: StatusCode,
        body: &'static str,
        retry_after: Option<&'static str>,
        delay: Duration,
    }

    impl TestResponse {
        fn immediate(status: StatusCode) -> Self {
            Self {
                status,
                body: "test response",
                retry_after: None,
                delay: Duration::ZERO,
            }
        }
    }

    async fn serve(responses: Vec<TestResponse>) -> (String, Arc<AtomicUsize>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let request_count = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&request_count);
        let handle = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                count.fetch_add(1, Ordering::SeqCst);
                handlers.spawn(async move {
                    let mut request = vec![0; 8 * 1024];
                    let _ = socket.read(&mut request).await;
                    tokio::time::sleep(response.delay).await;

                    let reason = response.status.canonical_reason().unwrap_or("Unknown");
                    let retry_after = response
                        .retry_after
                        .map(|value| format!("Retry-After: {value}\r\n"))
                        .unwrap_or_default();
                    let wire_response = format!(
                        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                        response.status.as_u16(),
                        reason,
                        response.body.len(),
                        retry_after,
                        response.body
                    );
                    let _ = socket.write_all(wire_response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
            while handlers.join_next().await.is_some() {}
        });

        (format!("http://{address}"), request_count, handle)
    }

    fn no_wait_config(max_retries: usize) -> RetryConfig {
        RetryConfig::default()
            .with_max_retries(max_retries)
            .with_initial_backoff(Duration::ZERO)
            .with_max_backoff(Duration::ZERO)
            .with_rate_limit_backoff(Duration::ZERO)
    }

    async fn run_request<F>(
        config: RetryConfig,
        make_request: F,
    ) -> Result<(Response, Vec<ProviderRetryEvent>), ProviderError>
    where
        F: Fn() -> reqwest::RequestBuilder + Send + Sync,
    {
        let mut events = Box::pin(send_with_retry(config, make_request));
        let mut retries = Vec::new();
        while let Some(event) = events.next().await {
            match event? {
                HttpRequestEvent::Retry(event) => retries.push(event),
                HttpRequestEvent::Response(response) => return Ok((response, retries)),
            }
        }
        Err(ProviderError::Stream(
            "HTTP retry stream ended without a response".to_owned(),
        ))
    }

    #[test]
    fn defaults_to_a_configurable_ten_retries() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries(), DEFAULT_MAX_RETRIES);
        assert_eq!(config.request_timeout(), DEFAULT_REQUEST_TIMEOUT);
        assert_eq!(
            config.stream_idle_timeout(),
            Some(DEFAULT_STREAM_IDLE_TIMEOUT)
        );
        assert_eq!(config.with_max_retries(3).max_retries(), 3);
        assert_eq!(
            config.without_stream_idle_timeout().stream_idle_timeout(),
            None
        );
    }

    #[tokio::test]
    async fn enforces_the_established_stream_idle_deadline() {
        let timeout = Duration::from_millis(10);
        let mut stream = futures_util::stream::pending::<()>();

        let error = next_stream_item(&mut stream, Some(timeout))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ProviderError::StreamIdleTimeout { timeout: observed } if observed == timeout
        ));
    }

    #[test]
    fn classifies_only_transient_statuses_as_retryable() {
        assert_eq!(retry_kind(StatusCode::FORBIDDEN), RetryKind::Never);
        assert_eq!(retry_kind(StatusCode::NOT_FOUND), RetryKind::Never);
        assert_eq!(retry_kind(StatusCode::BAD_REQUEST), RetryKind::Never);
        assert_eq!(
            retry_kind(StatusCode::TOO_MANY_REQUESTS),
            RetryKind::RateLimit
        );
        assert_eq!(
            retry_kind(StatusCode::REQUEST_TIMEOUT),
            RetryKind::Exponential
        );
        assert_eq!(
            retry_kind(StatusCode::SERVICE_UNAVAILABLE),
            RetryKind::Exponential
        );
        assert_eq!(retry_kind(StatusCode::NOT_IMPLEMENTED), RetryKind::Never);
    }

    #[test]
    fn rate_limit_delay_is_fixed_and_capped() {
        let config = RetryConfig::default()
            .with_initial_backoff(Duration::from_secs(5))
            .with_rate_limit_backoff(Duration::from_secs(2))
            .with_max_backoff(Duration::from_secs(10));

        assert_eq!(config.rate_limit_delay(None), Duration::from_secs(2));
        assert_eq!(
            config.rate_limit_delay(Some(Duration::from_secs(8))),
            Duration::from_secs(8)
        );
        assert_eq!(
            config.rate_limit_delay(Some(Duration::from_secs(30))),
            Duration::from_secs(10)
        );
        let exponential = config.exponential_delay(2);
        assert!(exponential >= Duration::from_secs(5));
        assert!(exponential <= Duration::from_secs(10));
    }

    #[tokio::test]
    async fn retries_transient_statuses_until_success() {
        let (url, request_count, server) = serve(vec![
            TestResponse::immediate(StatusCode::INTERNAL_SERVER_ERROR),
            TestResponse::immediate(StatusCode::BAD_GATEWAY),
            TestResponse::immediate(StatusCode::OK),
        ])
        .await;
        let client = reqwest::Client::new();
        let (response, retries) = run_request(no_wait_config(2), || client.get(&url))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(retries.len(), 2);
        assert_eq!(retries[0].retry_number, 1);
        assert_eq!(retries[1].retry_number, 2);
        assert!(matches!(
            retries[0].reason,
            ProviderRetryReason::HttpStatus { status: 500, .. }
        ));
        assert_eq!(request_count.load(Ordering::SeqCst), 3);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stops_after_the_configured_retry_budget() {
        let (url, request_count, server) = serve(vec![
            TestResponse::immediate(StatusCode::INTERNAL_SERVER_ERROR),
            TestResponse::immediate(StatusCode::INTERNAL_SERVER_ERROR),
            TestResponse::immediate(StatusCode::INTERNAL_SERVER_ERROR),
        ])
        .await;
        let client = reqwest::Client::new();
        let error = run_request(no_wait_config(2), || client.get(&url))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ProviderError::Api {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                ..
            }
        ));
        assert_eq!(request_count.load(Ordering::SeqCst), 3);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn does_not_retry_permanent_client_errors() {
        let (url, request_count, server) =
            serve(vec![TestResponse::immediate(StatusCode::FORBIDDEN)]).await;
        let client = reqwest::Client::new();
        let error = run_request(no_wait_config(DEFAULT_MAX_RETRIES), || client.get(&url))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ProviderError::Api {
                status: StatusCode::FORBIDDEN,
                ..
            }
        ));
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn classifies_context_length_errors_without_retrying() {
        let (url, request_count, server) = serve(vec![TestResponse {
            body: r#"{"error":{"code":"context_length_exceeded","message":"maximum context length reached"}}"#,
            ..TestResponse::immediate(StatusCode::BAD_REQUEST)
        }])
        .await;
        let client = reqwest::Client::new();
        let error = run_request(no_wait_config(DEFAULT_MAX_RETRIES), || client.post(&url))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ProviderError::ContextLengthExceeded { message }
                if message.contains("context_length_exceeded")
        ));
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn does_not_retry_response_header_timeouts() {
        let (url, request_count, server) = serve(vec![TestResponse {
            delay: Duration::from_millis(100),
            ..TestResponse::immediate(StatusCode::OK)
        }])
        .await;
        let client = reqwest::Client::new();
        let config = no_wait_config(1).with_request_timeout(Duration::from_millis(25));
        let error = run_request(config, || client.post(&url)).await.unwrap_err();

        assert!(matches!(
            error,
            ProviderError::RequestTimeout { timeout }
                if timeout == Duration::from_millis(25)
        ));
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn retries_only_connect_transport_errors() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);

        let attempts = Arc::new(AtomicUsize::new(0));
        let observed_attempts = Arc::clone(&attempts);
        let client = reqwest::Client::new();
        let url = format!("http://{address}");
        let error = run_request(no_wait_config(2), move || {
            observed_attempts.fetch_add(1, Ordering::SeqCst);
            client.post(&url)
        })
        .await
        .unwrap_err();

        assert!(matches!(error, ProviderError::Http(error) if error.is_connect()));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retries_429_using_retry_after() {
        let (url, request_count, server) = serve(vec![
            TestResponse {
                retry_after: Some("0"),
                ..TestResponse::immediate(StatusCode::TOO_MANY_REQUESTS)
            },
            TestResponse::immediate(StatusCode::OK),
        ])
        .await;
        let client = reqwest::Client::new();
        let (response, retries) = run_request(no_wait_config(1), || client.get(&url))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(matches!(
            retries.as_slice(),
            [ProviderRetryEvent {
                retry_number: 1,
                delay: Duration::ZERO,
                reason: ProviderRetryReason::HttpStatus { status: 429, .. },
                ..
            }]
        ));
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
        server.await.unwrap();
    }
}
