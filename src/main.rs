use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use lettre::{
    Message, SmtpTransport, Transport,
    message::{Mailboxes, MultiPart, SinglePart},
};
use serde_json::{Value, json};
use std::{env, error::Error, io::Read, sync::Arc, thread, time::Duration};
use tiny_http::{Header, Method, Request, Response, Server};

type AnyError = Box<dyn Error + Send + Sync>;

const MAX_JSON_BODY_BYTES: usize = 1024 * 1024;
const JSON_BODY_TOO_LARGE: &str = "JSON body exceeds the 1 MiB limit";

struct Config {
    host: String,
    port: u16,
    from: String,
    to: String,
    prefix: String,
    timezone: Tz,
}

impl Config {
    fn from_env() -> Result<Self, AnyError> {
        let var = |name, default: &str| env::var(name).unwrap_or_else(|_| default.to_owned());
        Ok(Self {
            host: var("SMTP_HOST", "postfix"),
            port: var("SMTP_PORT", "25").parse()?,
            from: var("FROM_ADDR", "komodo@localhost"),
            to: var("TO_ADDR", ""),
            prefix: var("SUBJECT_PREFIX", "[Komodo]"),
            timezone: var("DISPLAY_TIMEZONE", "UTC")
                .parse()
                .unwrap_or(chrono_tz::UTC),
        })
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64().is_some_and(|n| n != 0.0),
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}

fn display(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}

fn field(value: &Value, key: &str, default: &str) -> String {
    value
        .get(key)
        .map(display)
        .unwrap_or_else(|| default.into())
}

fn parse_ts(value: &Value) -> Option<DateTime<Utc>> {
    if !truthy(value) {
        return None;
    }
    let milliseconds = match value {
        Value::Bool(true) => 1.0,
        _ => value.as_f64()?,
    };
    // Keep fractional milliseconds, as in datetime.fromtimestamp.
    let micros = (milliseconds * 1000.0).round();
    if !micros.is_finite() || micros < i64::MIN as f64 || micros >= i64::MAX as f64 {
        return None;
    }
    let dt = DateTime::from_timestamp_micros(micros as i64)?;
    // Python datetime's supported range.
    use chrono::Datelike;
    (1..=9999).contains(&dt.year()).then_some(dt)
}

fn format_ts(value: &Value, timezone: Tz) -> String {
    match parse_ts(value) {
        Some(dt) => dt
            .with_timezone(&timezone)
            .format("%Y-%m-%d %H:%M:%S %Z")
            .to_string(),
        None if !truthy(value) => "-".into(),
        None => display(value),
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn colors(level: &str, resolved: bool) -> (&'static str, &'static str, &'static str) {
    if resolved {
        return ("#e8f5e9", "#1b5e20", "#a5d6a7");
    }
    match level.to_uppercase().as_str() {
        "CRITICAL" | "ERROR" | "FAIL" | "FAILED" => ("#ffebee", "#b71c1c", "#ef9a9a"),
        "WARNING" | "WARN" => ("#fff8e1", "#8d6e00", "#ffe082"),
        "OK" | "INFO" => ("#e3f2fd", "#0d47a1", "#90caf9"),
        _ => ("#f5f5f5", "#424242", "#d0d0d0"),
    }
}

// Expand only template placeholders; alert text must never be reinterpreted as a template.
fn render(template: &str, fields: &[(&str, String)]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find("}}").expect("closed template placeholder");
        let key = &tail[..end];
        output.push_str(
            &fields
                .iter()
                .find(|(name, _)| *name == key)
                .expect("known template placeholder")
                .1,
        );
        rest = &tail[end + 2..];
    }
    output.push_str(rest);
    output
}

fn build_message(config: &Config, payload: &Value) -> Result<Message, AnyError> {
    let level = field(payload, "level", "INFO");
    let resolved = truthy(&payload["resolved"]);
    let status = if resolved { "RESOLVED" } else { "ACTIVE" };
    let data = &payload["data"];
    let inner = &data["data"];
    let alert_type = field(data, "type", "Unknown");
    let first = |values: &[&Value]| {
        values
            .iter()
            .find(|v| truthy(v))
            .map(|v| display(v))
            .unwrap_or_else(|| "-".into())
    };
    let alert_name = first(&[&inner["name"], &inner["id"], &data["name"]]);
    let alert_id = first(&[&inner["id"], &data["id"]]);
    let mut subject = format!("{} {status} {alert_type} {level}", config.prefix);
    if !alert_name.is_empty() && alert_name != "-" {
        subject.push(' ');
        subject.push_str(&alert_name);
    }
    if subject.contains(['\r', '\n']) {
        return Err("email subject contains a newline".into());
    }
    let mut fields = vec![
        ("status", status.into()),
        ("level", level.clone()),
        ("alert_type", alert_type),
        ("alert_name", alert_name),
        ("alert_id", alert_id),
        ("target_type", field(&payload["target"], "type", "-")),
        ("target_id", field(&payload["target"], "id", "-")),
        ("triggered", format_ts(&payload["ts"], config.timezone)),
        (
            "resolved_at",
            format_ts(&payload["resolved_ts"], config.timezone),
        ),
        ("raw_payload_pretty", serde_json::to_string_pretty(payload)?),
    ];
    let text = render(include_str!("text.txt"), &fields);
    for (_, value) in &mut fields {
        *value = escape(value);
    }
    // html.escape(None) in the original template rendered a dash.
    for (key, value) in &mut fields {
        let original = match *key {
            "level" => payload.get("level"),
            "alert_type" => data.get("type"),
            "target_type" => payload["target"].get("type"),
            "target_id" => payload["target"].get("id"),
            _ => None,
        };
        if original == Some(&Value::Null) {
            *value = "-".into();
        }
    }
    let (bg, fg, border) = colors(&level, resolved);
    fields.extend([
        ("bg", bg.into()),
        ("fg", fg.into()),
        ("border", border.into()),
        (
            "status_label",
            if resolved { "Resolved" } else { "Active" }.into(),
        ),
    ]);
    let html = render(include_str!("email.html"), &fields);
    let mut builder = Message::builder()
        .from(config.from.parse()?)
        .subject(subject)
        .date(parse_ts(&payload["ts"]).unwrap_or_else(Utc::now).into());
    for mailbox in config.to.parse::<Mailboxes>()? {
        builder = builder.to(mailbox);
    }
    Ok(builder.multipart(
        MultiPart::alternative()
            .singlepart(SinglePart::plain(text))
            .singlepart(SinglePart::html(html)),
    )?)
}

fn respond(request: Request, status: u16, body: Value) {
    let response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap());
    if let Err(error) = request.respond(response) {
        eprintln!("HTTP response failed: {error}");
    }
}

fn read_json_body(reader: impl Read) -> Result<Value, &'static str> {
    let mut body = Vec::new();
    // Read one extra byte to detect excess without parsing a truncated payload.
    let read_result = reader
        .take(MAX_JSON_BODY_BYTES as u64 + 1)
        .read_to_end(&mut body);
    if body.len() > MAX_JSON_BODY_BYTES {
        return Err(JSON_BODY_TOO_LARGE);
    }
    if read_result.is_err() {
        return Ok(json!({}));
    }
    Ok(serde_json::from_slice(&body).unwrap_or(json!({})))
}

fn handle(mut request: Request, config: &Config, smtp: &SmtpTransport) {
    let path = request.url().split('?').next().unwrap_or("");
    match (request.method(), path) {
        (&Method::Get | &Method::Head, "/health") => respond(request, 200, json!({"ok": true})),
        (&Method::Post, "/komodo") => {
            if config.to.is_empty() {
                respond(request, 400, json!({"error": "TO_ADDR is empty"}));
                return;
            }
            let is_json = request.headers().iter().any(|h| {
                h.field.equiv("Content-Type") && {
                    let mime = h
                        .value
                        .as_str()
                        .split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_ascii_lowercase();
                    mime == "application/json"
                        || (mime.starts_with("application/") && mime.ends_with("+json"))
                }
            });
            let payload: Value = if is_json {
                if request
                    .body_length()
                    .is_some_and(|length| length > MAX_JSON_BODY_BYTES)
                {
                    respond(request, 413, json!({"error": JSON_BODY_TOO_LARGE}));
                    return;
                }
                match read_json_body(request.as_reader()) {
                    Ok(payload) => payload,
                    Err(error) => {
                        respond(request, 413, json!({"error": error}));
                        return;
                    }
                }
            } else {
                json!({})
            };
            let payload = if !truthy(&payload) {
                json!({})
            } else {
                payload
            };
            if !payload.is_object() {
                respond(
                    request,
                    500,
                    json!({"error": "alert payload must be an object"}),
                );
                return;
            }
            let result = build_message(config, &payload)
                .and_then(|message| smtp.send(&message).map(|_| ()).map_err(Into::into));
            match result {
                Ok(()) => respond(request, 200, json!({"ok": true})),
                Err(error) => respond(request, 500, json!({"error": error.to_string()})),
            }
        }
        (_, "/health" | "/komodo") => respond(request, 405, json!({"error": "Method not allowed"})),
        _ => respond(request, 404, json!({"error": "Not found"})),
    }
}

fn main() -> Result<(), AnyError> {
    let config = Arc::new(Config::from_env()?);
    let server = Arc::new(Server::http("0.0.0.0:8000")?);
    // A small fixed pool bounds worker memory and allows concurrent SMTP requests.
    let mut workers = Vec::new();
    for _ in 0..4 {
        let server = server.clone();
        let config = config.clone();
        workers.push(
            thread::Builder::new()
                .stack_size(512 * 1024)
                .spawn(move || {
                    let smtp = SmtpTransport::builder_dangerous(&config.host)
                        .port(config.port)
                        .timeout(Some(Duration::from_secs(15)))
                        .build();
                    for request in server.incoming_requests() {
                        handle(request, &config, &smtp);
                    }
                })?,
        );
    }
    eprintln!("Komodo Mail Bridge listening on 0.0.0.0:8000");
    for worker in workers {
        worker.join().map_err(|_| "HTTP worker panicked")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_and_dst() {
        let tz = chrono_tz::Europe::Paris;
        assert_eq!(
            format_ts(&json!(1710000000000_i64), tz),
            "2024-03-09 17:00:00 CET"
        );
        assert_eq!(
            format_ts(&json!(1719792000000_i64), tz),
            "2024-07-01 02:00:00 CEST"
        );
        assert_eq!(format_ts(&json!(0), tz), "-");
        assert_eq!(format_ts(&json!("invalid"), tz), "invalid");
        assert!(parse_ts(&json!(1e30)).is_none());
        assert_eq!(
            format_ts(&json!(-1000), chrono_tz::UTC),
            "1969-12-31 23:59:59 UTC"
        );
    }
    #[test]
    fn template_values_are_not_expanded() {
        assert_eq!(
            render("{{a}} {{b}}", &[("a", "{{b}}".into()), ("b", "<x>".into())]),
            "{{b}} <x>"
        );
        assert_eq!(escape("<>&\"'"), "&lt;&gt;&amp;&quot;&#x27;");
    }

    #[test]
    fn json_body_limit_is_enforced_before_parsing() {
        use std::io::Cursor;

        let payload = json!({"level": "OK", "data": {"type": "Test"}});
        let mut body = payload.to_string().into_bytes();
        body.resize(MAX_JSON_BODY_BYTES, b' ');
        assert_eq!(read_json_body(body.as_slice()).unwrap(), payload);
        body.push(b' ');
        let mut oversized = Cursor::new(body);
        assert_eq!(read_json_body(&mut oversized), Err(JSON_BODY_TOO_LARGE));
        assert_eq!(oversized.position(), MAX_JSON_BODY_BYTES as u64 + 1);

        // Invalid JSON must not stop the size check or become an empty alert.
        let mut malformed = Cursor::new(vec![b'!'; MAX_JSON_BODY_BYTES * 2]);
        assert_eq!(read_json_body(&mut malformed), Err(JSON_BODY_TOO_LARGE));
        assert_eq!(malformed.position(), MAX_JSON_BODY_BYTES as u64 + 1);
        assert_eq!(read_json_body(&b"invalid"[..]).unwrap(), json!({}));
    }

    #[test]
    fn oversized_http_webhooks_are_rejected_before_smtp() {
        use std::{
            io::Write,
            net::{Shutdown, TcpListener, TcpStream},
        };

        for (chunked, headers_only) in [(false, false), (true, false), (false, true)] {
            let keep_alive = !chunked && !headers_only;
            let relay = TcpListener::bind("127.0.0.1:0").unwrap();
            relay.set_nonblocking(true).unwrap();
            let smtp = SmtpTransport::builder_dangerous("127.0.0.1")
                .port(relay.local_addr().unwrap().port())
                .timeout(Some(Duration::from_secs(1)))
                .build();
            let config = Config {
                host: "127.0.0.1".into(),
                port: relay.local_addr().unwrap().port(),
                from: "komodo@example.com".into(),
                to: "mail@example.com".into(),
                prefix: "[Komodo]".into(),
                timezone: chrono_tz::UTC,
            };
            let server = Server::http("127.0.0.1:0").unwrap();
            let address = server.server_addr().to_ip().unwrap();
            let worker = thread::spawn(move || {
                let request = server
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .unwrap();
                handle(request, &config, &smtp);
                if keep_alive {
                    let request = server
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap()
                        .unwrap();
                    handle(request, &config, &smtp);
                }
            });
            let mut client = TcpStream::connect(address).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            client
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            // An impossible length must not trigger a cleanup allocation or panic.
            let length = if headers_only {
                usize::MAX
            } else {
                MAX_JSON_BODY_BYTES + 1
            };
            let framing = if chunked {
                "Transfer-Encoding: chunked".to_owned()
            } else {
                format!("Content-Length: {length}")
            };
            let connection = if keep_alive { "keep-alive" } else { "close" };
            write!(
                client,
                "POST /komodo?test=1 HTTP/1.1\r\nHost: localhost\r\nContent-Type: Application/CloudEvents+Json; charset=utf-8\r\nConnection: {connection}\r\n{framing}\r\n\r\n"
            )
            .unwrap();
            if chunked {
                write!(client, "{length:X}\r\n").unwrap();
            }
            if headers_only {
                client.shutdown(Shutdown::Write).unwrap();
            } else {
                let mut body = vec![b' '; length];
                body[..2].copy_from_slice(b"{}");
                client.write_all(&body).unwrap();
                if chunked {
                    client.write_all(b"\r\n0\r\n\r\n").unwrap();
                }
                if keep_alive {
                    client
                        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                        .unwrap();
                }
            }
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            worker.join().unwrap();
            assert!(response.starts_with("HTTP/1.1 413"), "{response}");
            assert!(response.contains(JSON_BODY_TOO_LARGE), "{response}");
            if keep_alive {
                assert!(response.contains("HTTP/1.1 200"), "{response}");
                assert!(response.contains("{\"ok\":true}"), "{response}");
            }
            assert!(matches!(
                relay.accept(),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
            ));
        }
    }
}
