//! A mock HTTP server on a loopback socket, so the client tests exercise the real
//! signing, header, and parsing path without touching the network.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

/// What the mock answers with.
#[derive(Debug, Clone)]
pub enum Reply {
    /// A JSON response with this status.
    Json(u16, String),
    /// Accept the connection and hang up without answering, which is what a
    /// network failure looks like to the client.
    HangUp,
}

impl Reply {
    /// A 200 carrying the gateway envelope around `data`.
    pub fn enveloped(data: &str) -> Reply {
        Reply::Json(
            200,
            format!(r#"{{"success":true,"data":{data},"metadata":{{}}}}"#),
        )
    }

    /// A gateway error envelope: the code lives at `error.code`.
    pub fn error_envelope(status: u16, code: &str, message: &str) -> Reply {
        Reply::Json(
            status,
            format!(
                r#"{{"success":false,"data":null,"error":{{"code":"{code}","message":"{message}"}}}}"#
            ),
        )
    }
}

/// One request as the server actually received it.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl Recorded {
    /// Header lookup is case-insensitive, like HTTP itself.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_lowercase()).map(String::as_str)
    }
}

/// A loopback server that answers a scripted list of replies, then repeats the
/// last one, and records every request it saw.
pub struct MockServer {
    base_url: String,
    received: Arc<Mutex<Vec<Recorded>>>,
}

impl MockServer {
    pub fn start(replies: Vec<Reply>) -> MockServer {
        assert!(!replies.is_empty(), "the mock needs at least one reply");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&received);

        std::thread::spawn(move || {
            for (served, stream) in listener.incoming().enumerate() {
                let Ok(stream) = stream else { break };
                // The last reply repeats, so a retry test can script "503 then 200"
                // and a polling test can script one answer.
                let reply = replies[served.min(replies.len() - 1)].clone();
                serve_one(stream, reply, &recorder);
            }
        });

        MockServer {
            base_url: format!("http://127.0.0.1:{port}"),
            received,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Everything the client sent, in order.
    pub fn requests(&self) -> Vec<Recorded> {
        self.received.lock().expect("mock lock").clone()
    }

    /// The single request the client sent. Panics when there was not exactly one.
    pub fn only_request(&self) -> Recorded {
        let requests = self.requests();
        assert_eq!(requests.len(), 1, "expected exactly one request");
        requests.into_iter().next().expect("one request")
    }
}

fn serve_one(mut stream: TcpStream, reply: Reply, recorder: &Arc<Mutex<Vec<Recorded>>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.trim().is_empty() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_lowercase(), value.trim().to_string());
        }
    }

    let length: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }

    recorder.lock().expect("mock lock").push(Recorded {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    });

    match reply {
        Reply::HangUp => {}
        Reply::Json(status, payload) => {
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{payload}",
                reason = reason_phrase(status),
                length = payload.len(),
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}
