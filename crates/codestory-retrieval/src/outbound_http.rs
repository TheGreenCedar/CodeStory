use std::fmt;
use std::io::{self, Read};

#[derive(Debug, Clone)]
pub struct HttpResponse<T> {
    pub status: u16,
    pub body: T,
}

#[derive(Debug, Clone)]
pub struct OutboundHttpError {
    status: Option<u16>,
    body: Option<String>,
    message: String,
}

impl OutboundHttpError {
    pub fn status(&self) -> Option<u16> {
        self.status
    }

    pub fn is_status(&self, status: u16) -> bool {
        self.status == Some(status)
    }

    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }
}

impl fmt::Display for OutboundHttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OutboundHttpError {}

pub fn read_text(
    response: Result<ureq::Response, ureq::Error>,
) -> Result<HttpResponse<String>, OutboundHttpError> {
    read_response(response, |response| response.into_string())
}

pub fn read_bytes(
    response: Result<ureq::Response, ureq::Error>,
) -> Result<HttpResponse<Vec<u8>>, OutboundHttpError> {
    read_response(response, |response| {
        let mut body = Vec::new();
        response.into_reader().read_to_end(&mut body)?;
        Ok(body)
    })
}

pub fn truncate_http_body(body: &str) -> String {
    truncate_http_body_to(body, 512)
}

pub fn truncate_http_body_to(body: &str, max_chars: usize) -> String {
    let trimmed = body.trim();
    let mut truncated = trimmed.chars().take(max_chars).collect::<String>();
    if trimmed.chars().count() > max_chars {
        truncated.push_str("...");
    }
    truncated
}

fn read_response<T>(
    response: Result<ureq::Response, ureq::Error>,
    read_body: impl FnOnce(ureq::Response) -> io::Result<T>,
) -> Result<HttpResponse<T>, OutboundHttpError> {
    match response {
        Ok(response) => {
            let status = response.status();
            let body = read_body(response).map_err(|error| OutboundHttpError {
                status: Some(status),
                message: format!("failed to read response body: {error}"),
                body: None,
            })?;
            Ok(HttpResponse { status, body })
        }
        Err(ureq::Error::Status(status, response)) => {
            let body = response
                .into_string()
                .unwrap_or_else(|error| format!("failed to read error response body: {error}"));
            Err(OutboundHttpError {
                status: Some(status),
                message: format!("http {status}: {}", truncate_http_body(&body)),
                body: Some(body),
            })
        }
        Err(ureq::Error::Transport(error)) => Err(OutboundHttpError {
            status: None,
            message: error.to_string(),
            body: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    fn one_shot_server(status: &str, body: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let url = format!("http://{}", listener.local_addr().expect("server addr"));
        let status = status.to_string();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let read = stream.read(&mut buffer).expect("read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let text = String::from_utf8_lossy(&request);
                    let content_length = text
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if content_length == 0 {
                        break;
                    }
                    let header_end = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .expect("header end")
                        + 4;
                    if request.len().saturating_sub(header_end) >= content_length {
                        break;
                    }
                }
            }
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
            String::from_utf8(request).expect("request utf8")
        });
        (url, handle)
    }

    #[test]
    fn delete_text_preserves_status_and_error_body_for_404() {
        let (url, request) = one_shot_server("404 Not Found", "missing collection");

        let error = read_text(ureq::delete(&url).timeout(Duration::from_secs(5)).call())
            .expect_err("delete should fail");

        assert!(error.is_status(404));
        assert_eq!(error.body(), Some("missing collection"));
        assert!(error.to_string().contains("missing collection"));
        assert!(
            request
                .join()
                .expect("request thread")
                .starts_with("DELETE / ")
        );
    }

    #[test]
    fn post_json_text_sends_json_content_type_and_custom_headers() {
        let (url, request) = one_shot_server("200 OK", "accepted");
        let response = read_text(
            ureq::post(&format!("{url}/summary"))
                .timeout(Duration::from_secs(5))
                .set("Content-Type", "application/json")
                .set("Authorization", "Bearer test-token")
                .send_string(r#"{"hello":"world"}"#),
        )
        .expect("post succeeds");

        assert_eq!(response.status, 200);
        assert_eq!(response.body, "accepted");
        let request = request.join().expect("request thread");
        let lowercase = request.to_ascii_lowercase();
        assert!(request.starts_with("POST /summary "));
        assert!(lowercase.contains("content-type: application/json"));
        assert!(lowercase.contains("authorization: bearer test-token"));
        assert!(request.ends_with(r#"{"hello":"world"}"#));
    }

    #[test]
    fn truncate_http_body_to_trims_and_counts_chars() {
        assert_eq!(truncate_http_body_to("  abc  ", 8), "abc");
        assert_eq!(truncate_http_body_to("éclair", 3), "écl...");
    }
}
