use std::collections::HashMap;
use std::io::Read;
use std::panic::AssertUnwindSafe;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Error;
use http::Response;

use crate::HttpClient;
use crate::HttpOptions;

/// Whether an error or its causes represent a request or streaming-read timeout.
///
/// This also recognizes transport errors wrapped by `std::io::Read`, so callers do not need to
/// depend on the underlying HTTP implementation to distinguish deadlines from other failures.
pub fn is_timeout(mut error: &(dyn std::error::Error + 'static)) -> bool {
    loop {
        match error.downcast_ref::<ureq::Error>() {
            Some(ureq::Error::Timeout(_)) => return true,
            Some(ureq::Error::Io(inner)) => {
                error = inner;
                continue;
            }
            _ => {}
        }
        if let Some(inner) = error.downcast_ref::<std::io::Error>() {
            if inner.kind() == std::io::ErrorKind::TimedOut {
                return true;
            }
            if let Some(source) = inner.get_ref() {
                error = source;
                continue;
            }
        }
        let Some(source) = error.source() else {
            return false;
        };
        error = source;
    }
}

#[derive(Default)]
/// Blocking HTTP client for usage with [`HttpClient`].
pub struct Client {
    options: HttpOptions,
    timeout: Option<Duration>,
    http_status_as_error: Option<bool>,
    max_redirects: Option<u32>,
    reuse_agent: bool,
    agent: OnceLock<AssertUnwindSafe<ureq::Agent>>,
}

impl Client {
    pub fn new(options: HttpOptions) -> Self {
        Self {
            options,
            ..Default::default()
        }
    }

    pub fn new_with_timeout(options: HttpOptions, timeout: Duration) -> Self {
        Self {
            options,
            timeout: Some(timeout),
            ..Default::default()
        }
    }

    /// Select whether HTTP 4xx and 5xx statuses become errors.
    ///
    /// Defaults to true. Disable this to inspect error response headers and bodies, including
    /// Retry-After.
    pub fn with_http_status_as_error(mut self, enabled: bool) -> Self {
        self.http_status_as_error = Some(enabled);
        self.agent.take();
        self
    }

    /// Limit automatically followed redirects. Zero returns redirect responses without following
    /// them, allowing callers to enforce an origin policy before sending another request.
    pub fn with_max_redirects(mut self, limit: u32) -> Self {
        self.max_redirects = Some(limit);
        self.agent.take();
        self
    }

    /// Reuse the transport agent and its connection pool across requests. Without this, each
    /// request uses a fresh agent.
    pub fn with_reused_agent(mut self) -> Self {
        self.reuse_agent = true;
        self
    }

    fn agent(&self) -> Result<ureq::Agent, Error> {
        if let Some(agent) = self.agent.get() {
            return Ok(agent.0.clone());
        }
        let mut builder = ureq::Agent::config_builder()
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::NativeTls)
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .user_agent(self.options.user_agent.as_deref().unwrap_or(concat!(
                "proxmox-sync-http-client/",
                env!("CARGO_PKG_VERSION")
            )))
            .timeout_global(self.timeout);

        if let Some(proxy_config) = &self.options.proxy_config {
            builder = builder.proxy(Some(ureq::Proxy::new(&proxy_config.to_proxy_string()?)?));
        }

        if let Some(enabled) = self.http_status_as_error {
            builder = builder.http_status_as_error(enabled);
        }
        if let Some(limit) = self.max_redirects {
            builder = builder.max_redirects(limit);
        }

        let agent = builder.build().into();
        if self.reuse_agent {
            // The agent exposes no mutable state to callers across an unwind boundary.
            Ok(self.agent.get_or_init(|| AssertUnwindSafe(agent)).0.clone())
        } else {
            Ok(agent)
        }
    }

    fn add_headers(
        mut req: http::request::Builder,
        content_type: Option<&str>,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> http::request::Builder {
        if let Some(content_type) = content_type {
            req = req.header("Content-Type", content_type);
        }

        if let Some(extra_headers) = extra_headers {
            for (header, value) in extra_headers {
                req = req.header(header, value);
            }
        }

        req
    }

    fn convert_response_to_string(res: Response<ureq::Body>) -> Result<Response<String>, Error> {
        let (parts, mut body) = res.into_parts();
        let body = body.read_to_string()?;
        Ok(Response::from_parts(parts, body))
    }

    fn convert_response_to_vec(res: Response<ureq::Body>) -> Result<Response<Vec<u8>>, Error> {
        let (parts, mut body) = res.into_parts();
        let body = body.read_to_vec()?;
        Ok(Response::from_parts(parts, body))
    }

    fn convert_response_to_reader(res: Response<ureq::Body>) -> Response<Box<dyn Read>> {
        let (parts, body) = res.into_parts();
        Response::from_parts(parts, Box::new(body.into_reader()))
    }
}

impl HttpClient<String, String> for Client {
    fn get(
        &self,
        uri: &str,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<http::Response<String>, Error> {
        let agent = self.agent()?;
        let req = http::Request::get(uri);
        let req = Self::add_headers(req, None, extra_headers);
        let req = req.body(ureq::SendBody::none())?;

        agent
            .run(req)
            .map_err(Error::from)
            .and_then(Self::convert_response_to_string)
    }

    fn post(
        &self,
        uri: &str,
        body: Option<String>,
        content_type: Option<&str>,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<http::Response<String>, Error> {
        let agent = self.agent()?;
        let req = http::Request::post(uri);
        let req = Self::add_headers(req, content_type, extra_headers);

        match body {
            Some(body) => agent.run(req.body(body)?),
            None => agent.run(req.body(ureq::SendBody::none())?),
        }
        .map_err(Error::from)
        .and_then(Self::convert_response_to_string)
    }

    fn request(&self, request: http::Request<String>) -> Result<http::Response<String>, Error> {
        let (parts, body) = request.into_parts();

        let agent = self.agent()?;
        let mut req = http::Request::builder()
            .method(parts.method.as_str())
            .uri(parts.uri);

        for header in parts.headers.keys() {
            for value in parts.headers.get_all(header) {
                req = req.header(header.as_str(), value.to_str()?);
            }
        }

        agent
            .run(req.body(body)?)
            .map_err(Error::from)
            .and_then(Self::convert_response_to_string)
    }
}

impl HttpClient<&[u8], Vec<u8>> for Client {
    fn get(
        &self,
        uri: &str,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<http::Response<Vec<u8>>, Error> {
        let agent = self.agent()?;
        let req = http::Request::get(uri);
        let req = Self::add_headers(req, None, extra_headers);
        let req = req.body(ureq::SendBody::none())?;

        agent
            .run(req)
            .map_err(Error::from)
            .and_then(Self::convert_response_to_vec)
    }

    fn post(
        &self,
        uri: &str,
        body: Option<&[u8]>,
        content_type: Option<&str>,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<http::Response<Vec<u8>>, Error> {
        let agent = self.agent()?;
        let req = http::Request::post(uri);
        let req = Self::add_headers(req, content_type, extra_headers);

        match body {
            Some(body) => agent.run(req.body(body)?),
            None => agent.run(req.body(ureq::SendBody::none())?),
        }
        .map_err(Error::from)
        .and_then(Self::convert_response_to_vec)
    }

    fn request(&self, request: http::Request<&[u8]>) -> Result<http::Response<Vec<u8>>, Error> {
        let (parts, body) = request.into_parts();

        let agent = self.agent()?;
        let mut req = http::Request::builder()
            .method(parts.method.as_str())
            .uri(parts.uri);

        for header in parts.headers.keys() {
            for value in parts.headers.get_all(header) {
                req = req.header(header.as_str(), value.to_str()?);
            }
        }

        agent
            .run(req.body(body)?)
            .map_err(Error::from)
            .and_then(Self::convert_response_to_vec)
    }
}

impl HttpClient<Box<dyn Read>, Box<dyn Read>> for Client {
    fn get(
        &self,
        uri: &str,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<http::Response<Box<dyn Read>>, Error> {
        let agent = self.agent()?;
        let req = http::Request::get(uri);
        let req = Self::add_headers(req, None, extra_headers);
        let req = req.body(ureq::SendBody::none())?;

        agent
            .run(req)
            .map_err(Error::from)
            .map(Self::convert_response_to_reader)
    }

    fn post(
        &self,
        uri: &str,
        body: Option<Box<dyn Read>>,
        content_type: Option<&str>,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<http::Response<Box<dyn Read>>, Error> {
        let agent = self.agent()?;
        let req = http::Request::post(uri);
        let req = Self::add_headers(req, content_type, extra_headers);

        match body {
            Some(body) => agent.run(req.body(ureq::SendBody::from_owned_reader(body))?),
            None => agent.run(req.body(ureq::SendBody::none())?),
        }
        .map_err(Error::from)
        .map(Self::convert_response_to_reader)
    }

    fn request(
        &self,
        request: http::Request<Box<dyn Read>>,
    ) -> Result<http::Response<Box<dyn Read>>, Error> {
        let (parts, body) = request.into_parts();

        let agent = self.agent()?;
        let mut req = http::Request::builder()
            .method(parts.method.as_str())
            .uri(parts.uri);

        for header in parts.headers.keys() {
            for value in parts.headers.get_all(header) {
                req = req.header(header.as_str(), value.to_str()?);
            }
        }

        agent
            .run(req.body(ureq::SendBody::from_owned_reader(body))?)
            .map_err(Error::from)
            .map(Self::convert_response_to_reader)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    #[test]
    fn timeout_detection_handles_request_and_reader_error_wrappers() {
        for error in [
            ureq::Error::Timeout(ureq::Timeout::Global),
            ureq::Error::Io(std::io::ErrorKind::TimedOut.into()),
        ] {
            let error = Error::new(error).context("request failed");
            assert!(is_timeout(error.as_ref()));
        }
        let error = ureq::Error::Timeout(ureq::Timeout::RecvBody).into_io();
        assert!(is_timeout(&error));
        assert!(is_timeout(&std::io::Error::other(error)));
        assert!(!is_timeout(&ureq::Error::StatusCode(408)));
        assert!(!is_timeout(&std::io::Error::other("timeout")));
        assert!(!is_timeout(&ureq::Error::Io(
            std::io::ErrorKind::ConnectionReset.into()
        )));
    }

    #[test]
    fn client_preserves_auto_traits() {
        fn assert_traits<T: Send + Sync + UnwindSafe + RefUnwindSafe>() {}
        assert_traits::<Client>();
    }

    /// Reruns the named test in a child process without proxy variables and returns true, or
    /// returns false inside that child, where the test runs itself.
    ///
    /// ureq reads these variables for every agent, so a proxy set in the environment would receive
    /// the loopback requests. Removing them in this process would race with tests reading the
    /// environment on other threads.
    fn run_in_child_without_proxy_env(test: &str) -> bool {
        const CHILD: &str = "PROXMOX_HTTP_TEST_WITHOUT_PROXY";
        if std::env::var_os(CHILD).is_some() {
            return false;
        }

        // Test names are module paths without the crate name.
        let (_, module) = module_path!().split_once("::").unwrap();
        let test = format!("{module}::{test}");
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", &test]).env(CHILD, "1");
        for name in [
            "ALL_PROXY",
            "all_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
        ] {
            command.env_remove(name);
        }
        let output = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        // An unknown name runs no test and still exits successfully.
        assert!(
            output.status.success() && stdout.contains("test result: ok. 1 passed"),
            "{test} without proxy variables:\n{stdout}{}",
            String::from_utf8_lossy(&output.stderr),
        );
        true
    }

    fn accept(listener: &TcpListener) -> TcpStream {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    return stream;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "client did not connect");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept: {error}"),
            }
        }
    }

    /// Reads one request head, or returns false if the client closed the connection instead.
    fn read_request_head(stream: &mut TcpStream) -> bool {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            if stream.read(&mut byte).unwrap() == 0 {
                assert!(head.is_empty(), "connection closed within a request head");
                return false;
            }
            head.push(byte[0]);
            assert!(head.len() < 8192);
        }
        true
    }

    /// Serves the responses in order, on one connection as long as the client keeps it open. The
    /// worker returns the number of accepted connections.
    fn fixture(responses: &'static [&'static str]) -> (String, JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let worker = thread::spawn(move || {
            let mut connections = 0;
            let mut stream = None;
            for response in responses {
                loop {
                    let current = stream.get_or_insert_with(|| {
                        connections += 1;
                        accept(&listener)
                    });
                    if read_request_head(current) {
                        current.write_all(response.as_bytes()).unwrap();
                        break;
                    }
                    stream = None;
                }
            }
            connections
        });
        (url, worker)
    }

    fn client() -> Client {
        Client::new_with_timeout(HttpOptions::default(), Duration::from_secs(2))
    }

    fn get(client: &Client, url: &str) -> Result<Response<String>, Error> {
        <Client as HttpClient<String, String>>::get(client, url, None)
    }

    #[test]
    fn status_errors_remain_the_default() {
        if run_in_child_without_proxy_env("status_errors_remain_the_default") {
            return;
        }
        let (url, worker) =
            fixture(&["HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n"]);
        let error = get(&client(), &url).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ureq::Error>(),
            Some(ureq::Error::StatusCode(429))
        ));
        worker.join().unwrap();
    }

    #[test]
    fn error_responses_keep_status_headers_and_streaming_body() {
        if run_in_child_without_proxy_env("error_responses_keep_status_headers_and_streaming_body")
        {
            return;
        }
        let (url, worker) = fixture(&[
            "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 5\r\nRetry-After: 9\r\n\r\nlater",
        ]);
        let client = client().with_http_status_as_error(false);
        let response =
            <Client as HttpClient<Box<dyn Read>, Box<dyn Read>>>::get(&client, &url, None).unwrap();
        assert_eq!(response.status(), 429);
        assert_eq!(response.headers()["retry-after"], "9");
        let mut body = String::new();
        response.into_body().read_to_string(&mut body).unwrap();
        assert_eq!(body, "later");
        worker.join().unwrap();
    }

    #[test]
    fn zero_redirects_returns_location_without_following_it() {
        if run_in_child_without_proxy_env("zero_redirects_returns_location_without_following_it") {
            return;
        }
        let (url, worker) =
            fixture(&["HTTP/1.1 302 Found\r\nContent-Length: 0\r\nLocation: /target\r\n\r\n"]);
        let response = get(&client().with_max_redirects(0), &url).unwrap();
        assert_eq!(response.status(), 302);
        assert_eq!(response.headers()["location"], "/target");
        worker.join().unwrap();
    }

    #[test]
    fn finite_redirect_limit_stops_a_loop() {
        if run_in_child_without_proxy_env("finite_redirect_limit_stops_a_loop") {
            return;
        }
        const REDIRECT: &str =
            "HTTP/1.1 302 Found\r\nContent-Length: 0\r\nLocation: /again\r\n\r\n";
        let (url, worker) = fixture(&[REDIRECT, REDIRECT]);
        let error = get(&client().with_max_redirects(1), &url).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ureq::Error>(),
            Some(ureq::Error::TooManyRedirects)
        ));
        worker.join().unwrap();
    }

    #[test]
    fn agent_cache_is_opt_in_and_reset_by_policy_changes() {
        let client = client();
        client.agent().unwrap();
        assert!(client.agent.get().is_none());

        let client = client.with_reused_agent();
        client.agent().unwrap();
        assert!(client.agent.get().is_some());
        let client = client.with_http_status_as_error(false);
        assert!(client.agent.get().is_none());
        client.agent().unwrap();
        let client = client.with_max_redirects(0);
        assert!(client.agent.get().is_none());

        let agent = client.agent().unwrap();
        assert!(!agent.config().http_status_as_error());
        assert_eq!(agent.config().max_redirects(), 0);
        assert!(client.agent.get().is_some());
    }

    #[test]
    fn only_reused_agents_keep_the_connection() {
        if run_in_child_without_proxy_env("only_reused_agents_keep_the_connection") {
            return;
        }
        const OK: &str = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
        for (client, connections) in [(client(), 2), (client().with_reused_agent(), 1)] {
            let (url, worker) = fixture(&[OK, OK]);
            assert_eq!(get(&client, &url).unwrap().body(), "ok");
            assert_eq!(get(&client, &url).unwrap().body(), "ok");
            assert_eq!(worker.join().unwrap(), connections);
        }
    }
}
