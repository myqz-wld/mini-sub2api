//! Typed, bounded observations: never format untrusted error messages or debug data.
use std::error::Error;
use std::io;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ErrorDetails {
    pub kind: &'static str,
    pub io_kind: &'static str,
    pub os_code: i32,
    pub http_kind: &'static str,
    pub h2_code: i64,
    pub h2_reset: bool,
    pub h2_goaway: bool,
    pub h2_remote: bool,
    pub tls_kind: &'static str,
    pub tls_code: i32,
    pub tls_library_code: u64,
    pub source_depth: u8,
    pub source_truncated: bool,
}

impl Default for ErrorDetails {
    fn default() -> Self {
        Self {
            kind: "none",
            io_kind: "none",
            os_code: 0,
            http_kind: "none",
            h2_code: -1,
            h2_reset: false,
            h2_goaway: false,
            h2_remote: false,
            tls_kind: "none",
            tls_code: 0,
            tls_library_code: 0,
            source_depth: 0,
            source_truncated: false,
        }
    }
}

impl ErrorDetails {
    pub fn observe(error: &(dyn Error + 'static)) -> Self {
        let mut out = Self {
            kind: "other",
            ..Self::default()
        };
        let mut current = Some(error);
        while let Some(error) = current {
            if out.source_depth == 16 {
                out.source_truncated = true;
                break;
            }
            out.source_depth += 1;
            if let Some(error) = error.downcast_ref::<serde_json::Error>() {
                out.kind = match error.classify() {
                    serde_json::error::Category::Io => "json_io",
                    serde_json::error::Category::Syntax => "json_syntax",
                    serde_json::error::Category::Data => "json_data",
                    serde_json::error::Category::Eof => "json_eof",
                };
            }
            if error.is::<std::str::Utf8Error>() {
                out.kind = "utf8";
            }
            if let Some(error) = error.downcast_ref::<reqwest::Error>() {
                out.kind = if error.is_timeout() && error.is_connect() {
                    "connect_timeout"
                } else if error.is_timeout() {
                    "timeout"
                } else if error.is_connect() {
                    "connect"
                } else if error.is_body() {
                    "body"
                } else if error.is_decode() {
                    "decode"
                } else if error.is_status() {
                    "http_status"
                } else if error.is_redirect() {
                    "redirect"
                } else if error.is_builder() {
                    "request_builder"
                } else {
                    "request"
                };
            }
            if let Some(error) = error.downcast_ref::<io::Error>() {
                let kind = io_kind(error.kind());
                if out.io_kind == "none" || kind != "other" {
                    out.io_kind = kind;
                }
                if let Some(code) = error.raw_os_error() {
                    out.os_code = code;
                }
            }
            if let Some(error) = error.downcast_ref::<hyper::Error>() {
                out.http_kind = if error.is_incomplete_message() {
                    "incomplete_message"
                } else if error.is_timeout() {
                    "timeout"
                } else if error.is_parse() {
                    "parse"
                } else if error.is_canceled() {
                    "canceled"
                } else if error.is_closed() {
                    "closed"
                } else if error.is_body_write_aborted() {
                    "body_write_aborted"
                } else if error.is_shutdown() {
                    "shutdown"
                } else if error.is_user() {
                    "user"
                } else {
                    "other"
                };
            }
            if let Some(error) = error.downcast_ref::<h2::Error>() {
                out.h2_code = error.reason().map_or(-1, |code| i64::from(u32::from(code)));
                out.h2_reset = error.is_reset();
                out.h2_goaway = error.is_go_away();
                out.h2_remote = error.is_remote();
            }
            if error.is::<native_tls::Error>() {
                out.tls_kind = "native_tls";
            }
            if error.is::<rustls::Error>() {
                out.tls_kind = "rustls";
            }
            #[cfg(target_os = "linux")]
            if let Some(error) = error.downcast_ref::<openssl::ssl::Error>() {
                out.tls_kind = "openssl";
                out.tls_code = error.code().as_raw();
            }
            #[cfg(target_os = "linux")]
            if let Some(error) = error.downcast_ref::<openssl::error::ErrorStack>() {
                out.tls_kind = "openssl";
                if let Some(last) = error.errors().last() {
                    out.tls_library_code = last.code();
                }
            }
            if let Some(error) = error.downcast_ref::<tungstenite::Error>() {
                out.kind = match error {
                    tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
                        "websocket_closed"
                    }
                    tungstenite::Error::Io(_) => "websocket_io",
                    tungstenite::Error::Tls(_) => "websocket_tls",
                    tungstenite::Error::Protocol(_) => "websocket_protocol",
                    tungstenite::Error::Capacity(_) => "websocket_capacity",
                    tungstenite::Error::Http(_) => "http_status",
                    _ => "websocket_other",
                };
            }
            current = error.source();
        }
        out
    }

    pub fn log(self, request_id: &str, phase: &'static str) {
        tracing::warn!(
            event = "transport_error",
            request_id,
            phase,
            error_kind = self.kind,
            io_kind = self.io_kind,
            os_error = self.os_code,
            http_error = self.http_kind,
            h2_code = self.h2_code,
            h2_reset = self.h2_reset,
            h2_goaway = self.h2_goaway,
            h2_remote = self.h2_remote,
            tls_kind = self.tls_kind,
            tls_code = self.tls_code,
            tls_library_code = self.tls_library_code,
            source_depth = self.source_depth,
            source_truncated = self.source_truncated,
            "Transport operation failed"
        );
    }
}

fn io_kind(kind: io::ErrorKind) -> &'static str {
    match kind {
        io::ErrorKind::ConnectionReset => "connection_reset",
        io::ErrorKind::ConnectionAborted => "connection_aborted",
        io::ErrorKind::ConnectionRefused => "connection_refused",
        io::ErrorKind::UnexpectedEof => "unexpected_eof",
        io::ErrorKind::BrokenPipe => "broken_pipe",
        io::ErrorKind::TimedOut => "timeout",
        io::ErrorKind::NotConnected => "not_connected",
        io::ErrorKind::NetworkUnreachable => "network_unreachable",
        io::ErrorKind::HostUnreachable => "host_unreachable",
        io::ErrorKind::AddrNotAvailable => "address_unavailable",
        io::ErrorKind::PermissionDenied => "permission_denied",
        io::ErrorKind::InvalidData => "invalid_data",
        io::ErrorKind::InvalidInput => "invalid_input",
        io::ErrorKind::Interrupted => "interrupted",
        io::ErrorKind::WouldBlock => "would_block",
        _ => "other",
    }
}

#[cfg(test)]
#[path = "error_diagnostics_tests.rs"]
mod tests;
