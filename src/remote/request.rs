//! A narrow HTTP/1.1 request parser and the auth guard.
//!
//! Both halves are pure: `read_request` takes any `BufRead`, `Guard::check`
//! takes an already-parsed request. Nothing here opens a socket, so the
//! decisions that keep an unauthenticated peer out are unit tested directly
//! rather than through a live server.
//!
//! This is the first code such a peer reaches, which is why it parses only
//! what the session server actually serves and bounds every dimension of a
//! request *while* reading it rather than after: a peer must not be able to
//! make alc allocate by announcing something large. It is not a general HTTP
//! implementation and must not grow into one. Chunked bodies, header
//! continuation lines, absolute-form targets and pipelining are all refused
//! instead of handled, because each is a place where this parser and some
//! other one could disagree about where a request ends.
//!
//! A caller must close the connection on any error from `read_request`: once
//! a request has been refused, the stream position is no longer trustworthy.

use std::io::{BufRead, Read};

use anyhow::{Context, Result, bail};

/// Request line ceiling, terminator included. Generous next to the longest
/// route this server has (`/api/sessions/<id>`), and small enough that the
/// line can be buffered without a thought.
const MAX_REQUEST_LINE: usize = 8 * 1024;

/// Ceiling on the number of header fields.
const MAX_HEADERS: usize = 64;

/// Ceiling on the header block as a whole, terminators and the blank line
/// included. Counted across the block rather than per line so that 64 large
/// headers cannot add up to something this server would not accept as one.
const MAX_HEADER_BLOCK: usize = 16 * 1024;

/// Ceiling on a request body. Only the control routes take one, and their
/// payloads are a few hundred bytes.
const MAX_BODY: usize = 1024 * 1024;

/// The methods the session server answers. Anything else is refused at the
/// request line, before a route is ever looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    Get,
    Post,
    Delete,
}

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub method: Method,
    /// Percent-decoded, query stripped, guaranteed to start with `/` and to
    /// hold neither `..` nor a control byte.
    pub path: String,
    /// Raw, without the `?`. Decoded per parameter by `query_param`.
    pub query: String,
    /// Names lowercased; values with the surrounding whitespace trimmed.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The first value for `name`. `host` and `content-length` are rejected
    /// outright when repeated, so for those two "first" is also "only".
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The first `name=` value in the query string, percent-decoded.
    ///
    /// `+` is left alone rather than decoded as a space: the only values that
    /// travel here are session ids and other opaque tokens, where a literal
    /// plus has to survive the round trip.
    ///
    /// Names are compared RAW, and a malformed escape in the matching value
    /// is a refusal rather than a reason to keep looking. Both follow the
    /// same rule the path decoder does: one input must have exactly one
    /// reading. Decoding the name would make `%73ession` a second spelling
    /// of `session`, and skipping a bad value would let `?session=%zz&
    /// session=real` present two candidates and quietly take the later one.
    pub(crate) fn query_param(&self, name: &str) -> Option<String> {
        let pair = self
            .query
            .split('&')
            .find(|pair| pair.split_once('=').unwrap_or((pair, "")).0 == name)?;
        let (_, value) = pair.split_once('=').unwrap_or((pair, ""));
        let decoded = decode_utf8(value)?;
        // The same control-byte refusal `decode_path` applies. Nothing today
        // puts a query value anywhere a NUL or a CRLF would matter, but the
        // guarantee belongs on the accessor rather than on its callers.
        if decoded.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
            return None;
        }
        Some(decoded)
    }

    /// True when this is a WebSocket upgrade attempt.
    ///
    /// The `Upgrade` header alone decides it; `Connection: Upgrade` is not
    /// also required. A browser always sends both, so the only requests this
    /// classifies differently than a strict reading would are hand-made ones,
    /// and for those, counting as an upgrade is the stricter answer: it is
    /// what makes `check` insist on an `Origin`.
    pub(crate) fn is_upgrade(&self) -> bool {
        self.header("upgrade").is_some_and(|value| {
            value
                .split(',')
                .any(|protocol| protocol.trim().eq_ignore_ascii_case("websocket"))
        })
    }
}

/// Reads and parses one request. `reader` is anything `BufRead`.
///
/// A body is read only when `Content-Length` says how long it is. A POST
/// without one is refused rather than treated as empty: its body would
/// otherwise stay in the stream, where anything that reused the connection
/// would read it as the start of the next request.
pub(crate) fn read_request<R: BufRead>(reader: &mut R) -> Result<Request> {
    let line = read_line(reader, MAX_REQUEST_LINE, "request line")?;
    let (method, target) = parse_request_line(trim_terminator(&line))?;
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, query.to_owned()),
        None => (target, String::new()),
    };
    let path = decode_path(path)?;

    let headers = read_headers(reader)?;
    let body = read_body(reader, method, &headers)?;
    Ok(Request {
        method,
        path,
        query,
        headers,
        body,
    })
}

fn parse_request_line(line: &[u8]) -> Result<(Method, &str)> {
    let line = str::from_utf8(line).context("the request line is not valid UTF-8")?;
    let mut fields = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        bail!("the request line is not three space-separated fields");
    };
    let method = match method {
        "GET" => Method::Get,
        "POST" => Method::Post,
        "DELETE" => Method::Delete,
        // Never echoed: the method is attacker-chosen text up to the request
        // line limit, and this message goes to a log.
        _ => bail!("unsupported http method; this server serves GET, POST and DELETE"),
    };
    if !version.starts_with("HTTP/1.") {
        bail!("unsupported http version; this server speaks HTTP/1.1");
    }
    Ok((method, target))
}

/// Percent-decodes a request target's path and refuses everything the router
/// downstream would have to defend against itself.
///
/// The `..` check runs after decoding, so `%2e%2e%2f` is caught the same as a
/// literal `../`; it looks for the pair anywhere rather than only as a whole
/// segment, which costs nothing here because no asset this server hands out
/// has a dotted name.
fn decode_path(raw: &str) -> Result<String> {
    let decoded =
        decode_utf8(raw).context("the request path is not valid percent-encoded UTF-8")?;
    if !decoded.starts_with('/') {
        bail!("the request path must start with '/'; absolute-form targets are not served");
    }
    if decoded.contains("..") {
        bail!("the request path contains '..'");
    }
    if decoded.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        bail!("the request path contains a control byte");
    }
    Ok(decoded)
}

/// Reads the header block, holding one budget across every line so the block
/// as a whole cannot exceed `MAX_HEADER_BLOCK`.
fn read_headers<R: BufRead>(reader: &mut R) -> Result<Vec<(String, String)>> {
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut budget = MAX_HEADER_BLOCK;
    loop {
        let line = read_line(reader, budget, "header block")?;
        budget -= line.len();
        let line = trim_terminator(&line);
        if line.is_empty() {
            return Ok(headers);
        }
        if headers.len() == MAX_HEADERS {
            bail!("more than {MAX_HEADERS} request headers");
        }

        let line = str::from_utf8(line).context("a request header is not valid UTF-8")?;
        let (name, value) = line
            .split_once(':')
            .context("a request header has no ':' separator")?;
        // Rejecting a name that is not a token is what refuses both a folded
        // continuation line (it starts with whitespace) and the classic
        // `Content-Length : 0`, where a laxer parser sees a different header
        // than this one does.
        if !is_token(name) {
            bail!("a request header name is not a valid token");
        }
        let value = value.trim_matches([' ', '\t']);
        // HTAB is legal inside a field value per RFC 9110; only the trimming
        // above treats it as whitespace.
        if value
            .bytes()
            .any(|byte| (byte < 0x20 && byte != b'\t') || byte == 0x7f)
        {
            bail!("a request header value contains a control byte");
        }

        let name = name.to_ascii_lowercase();
        // Repeating any of these is a disagreement about which one counts,
        // which is the whole of request smuggling - and for `upgrade` and
        // `origin` it is worse than ambiguous: `header` returns the first
        // field line, so `Upgrade: h2c` followed by `Upgrade: websocket`
        // would make `is_upgrade` false and slip past the mandatory-Origin
        // rule that only upgrades get.
        if matches!(
            name.as_str(),
            "host" | "content-length" | "upgrade" | "origin" | "authorization"
        ) && headers.iter().any(|(known, _)| *known == name)
        {
            bail!("a request repeats its {name} header");
        }
        headers.push((name, value.to_owned()));
    }
}

/// Reads the body named by `Content-Length`, and nothing else.
///
/// The vector is sized only once the length is known to be within
/// `MAX_BODY`, so an announced 4 GiB costs an error rather than an
/// allocation.
fn read_body<R: BufRead>(
    reader: &mut R,
    method: Method,
    headers: &[(String, String)],
) -> Result<Vec<u8>> {
    let value = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    if value("transfer-encoding").is_some() {
        bail!("transfer-encoding is not supported; send a body with a content-length");
    }

    let Some(length) = value("content-length") else {
        if method == Method::Post {
            bail!("a POST needs a content-length");
        }
        return Ok(Vec::new());
    };
    // Not `parse` alone: it accepts a leading `+`, and a length another
    // parser would read differently is the same hazard as a repeated header.
    if length.is_empty() || !length.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("the content-length is not a plain decimal number");
    }
    let length: usize = length
        .parse()
        .ok()
        .filter(|length| *length <= MAX_BODY)
        .with_context(|| format!("the request body exceeds the {MAX_BODY}-byte limit"))?;

    let mut body = vec![0_u8; length];
    reader
        .read_exact(&mut body)
        .context("the connection closed before the request body was complete")?;
    Ok(body)
}

/// Reads one line, terminator included, refusing to buffer more than `limit`
/// bytes of it. `what` names the part of the request for the error message.
///
/// The limit is applied by reading through a `Take`, so an endless line is
/// never held in memory: the read stops `limit` bytes in and fails, rather
/// than growing a buffer until it succeeds.
fn read_line<R: BufRead>(reader: &mut R, limit: usize, what: &str) -> Result<Vec<u8>> {
    let mut line = Vec::new();
    // One byte past the limit, so a line that exactly fills it is still
    // distinguishable from one that overruns.
    (&mut *reader)
        .take(limit as u64 + 1)
        .read_until(b'\n', &mut line)
        .with_context(|| format!("failed to read the {what}"))?;
    if line.len() > limit {
        bail!("the {what} is larger than this server accepts");
    }
    if line.last() != Some(&b'\n') {
        bail!("the connection closed before the {what} was complete");
    }
    Ok(line)
}

/// Strips a trailing LF and the CR before it. A bare LF is accepted: nothing
/// stands between this parser and the socket, so there is no second parser
/// that could split the stream differently.
fn trim_terminator(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// RFC 9110 `token`: what a header field name is allowed to be.
fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// Percent-decodes to a `String`, or `None` for a malformed escape or bytes
/// that are not UTF-8. A malformed escape is refused rather than passed
/// through as a literal `%`, so there is only ever one reading of an input.
fn decode_utf8(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).copied().and_then(hex_digit)?;
            let low = bytes.get(index + 2).copied().and_then(hex_digit)?;
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

/// What a token is allowed to do. Ordered, so a route that needs input can
/// ask for `>= Grade::Operator` rather than matching every variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Grade {
    Viewer,
    Operator,
}

/// The one place a request is decided on. Every route goes through
/// `check`, including the ones that need no token, so adding a route cannot
/// accidentally add a way past the `Host` and `Origin` checks.
pub(crate) struct Guard {
    /// Every Host value this server answers to, INCLUDING the port.
    pub hosts: Vec<String>,
    /// Every allowed Origin, scheme included.
    pub origins: Vec<String>,
    pub operator: String,
    pub viewer: String,
}

/// Why a request was refused. Kept apart from `anyhow::Error` because the
/// caller has to turn it into a status code, and because these three are the
/// only reasons: a new one has to be added here deliberately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Denied {
    Host,
    Origin,
    Token,
}

impl Guard {
    /// ONE function used by the static routes, the JSON routes AND the
    /// WebSocket upgrade, so a route cannot be added that skips a check.
    /// `public` routes (/, /assets/*, /healthz) still get Host and
    /// Origin checked; only the token requirement is lifted.
    ///
    /// The upgrade passes `public: true` as well, because its token arrives
    /// in the first WebSocket frame and is graded there by `grade_token`;
    /// `is_upgrade` is what keeps it from inheriting the looser `Origin`
    /// rule a plain page load gets. On a public route the returned grade is
    /// the grade of whatever bearer token was presented, and `None` when
    /// there was none or it was not recognised - it is never a refusal.
    pub(crate) fn check(&self, request: &Request, public: bool) -> Result<Option<Grade>, Denied> {
        // The DNS-rebinding defence. An attacker's page at evil.com can be
        // made to resolve to 127.0.0.1, so the address a request arrived on
        // says nothing; the name the browser thinks it is talking to does,
        // and it has to match down to the port.
        let host = request.header("host").ok_or(Denied::Host)?;
        if !self
            .hosts
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(host))
        {
            return Err(Denied::Host);
        }

        match request.header("origin") {
            Some(origin)
                if self
                    .origins
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(origin)) => {}
            // A page somewhere else asked the browser to make this request.
            Some(_) => return Err(Denied::Origin),
            // Every browser sends an Origin on a WebSocket handshake, so one
            // without it is not a page - and a page is the only client whose
            // origin the browser would have policed for us.
            None if request.is_upgrade() => return Err(Denied::Origin),
            // A plain GET with no Origin is somebody typing the address in.
            None => {}
        }

        let grade = bearer(request).and_then(|token| self.grade_token(token));
        if public {
            return Ok(grade);
        }
        grade.map(Some).ok_or(Denied::Token)
    }

    /// Grades a bare token, for the WebSocket's first-frame auth.
    pub(crate) fn grade_token(&self, token: &str) -> Option<Grade> {
        // An empty candidate would match an empty configured secret, which
        // is a misconfiguration that would hand the session to anyone. Cheap
        // to refuse here rather than trusting every construction site.
        if token.is_empty() {
            return None;
        }
        // Both compares always run. Written as two bindings rather than an
        // `if … else if` over the calls so that neither the operator nor the
        // viewer comparison can be skipped by short-circuiting.
        let operator = constant_time_eq(token.as_bytes(), self.operator.as_bytes());
        let viewer = constant_time_eq(token.as_bytes(), self.viewer.as_bytes());
        match (operator, viewer) {
            (true, _) => Some(Grade::Operator),
            (false, true) => Some(Grade::Viewer),
            (false, false) => None,
        }
    }
}

/// The token out of an `Authorization: Bearer <t>` header. The scheme name
/// is case-insensitive per RFC 9110; the token itself is not.
fn bearer(request: &Request) -> Option<&str> {
    let value = request.header("authorization")?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim_start_matches(' '))
}

/// Compares two secrets without leaking where they first differ.
///
/// The lengths are compared up front and separately - that much is public,
/// since it is visible in the request size anyway - and the bytes are then
/// folded into one accumulator so the loop takes the same path whatever it
/// finds. Written out rather than `==` because the standard comparison stops
/// at the first differing byte, which is enough to recover a token one byte
/// at a time from timing alone.
pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Result<Request> {
        read_request(&mut raw.as_bytes())
    }

    fn guard() -> Guard {
        Guard {
            hosts: vec!["127.0.0.1:8787".to_owned(), "alc.example:8787".to_owned()],
            origins: vec!["http://127.0.0.1:8787".to_owned()],
            operator: "operator-token-0000000000000000".to_owned(),
            viewer: "viewer-token-000000000000000000".to_owned(),
        }
    }

    /// A request with the shape every browser page load has.
    fn get(headers: &str) -> Request {
        parse(&format!("GET /api/sessions HTTP/1.1\r\n{headers}\r\n")).expect("a well-formed GET")
    }

    #[test]
    fn a_well_formed_get_is_parsed_into_its_parts() {
        let request = parse(concat!(
            "GET /api/sessions/a%20b?session=x%2Fy&cols=120 HTTP/1.1\r\n",
            "Host: 127.0.0.1:8787\r\n",
            "Accept:   application/json  \r\n",
            "\r\n",
        ))
        .unwrap();

        assert_eq!(request.method, Method::Get);
        assert_eq!(request.path, "/api/sessions/a b");
        assert_eq!(request.query, "session=x%2Fy&cols=120");
        assert_eq!(request.header("HOST"), Some("127.0.0.1:8787"));
        assert_eq!(request.header("accept"), Some("application/json"));
        assert_eq!(request.header("origin"), None);
        assert_eq!(request.query_param("session").as_deref(), Some("x/y"));
        assert_eq!(request.query_param("rows"), None);
        assert!(request.body.is_empty());
        assert!(!request.is_upgrade());
    }

    #[test]
    fn a_post_reads_exactly_its_content_length() {
        let raw = "POST /api/x HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\nhello, next request";
        let mut reader = raw.as_bytes();
        let request = read_request(&mut reader).unwrap();

        assert_eq!(request.method, Method::Post);
        assert_eq!(request.body, b"hello");
        // Whatever followed the body stays in the stream rather than being
        // swallowed, so a caller can see there was more and close.
        assert_eq!(reader, b", next request");
    }

    #[test]
    fn an_upgrade_is_recognised_from_the_upgrade_header() {
        assert!(get("Host: h\r\nConnection: Upgrade\r\nUpgrade: WebSocket\r\n").is_upgrade());
        assert!(get("Host: h\r\nUpgrade: websocket, h2c\r\n").is_upgrade());
        assert!(!get("Host: h\r\nConnection: Upgrade\r\n").is_upgrade());
        assert!(!get("Host: h\r\n").is_upgrade());
    }

    #[test]
    fn every_refusal_names_its_own_reason() {
        let long_line = format!("GET /{} HTTP/1.1\r\nHost: h\r\n\r\n", "a".repeat(9000));
        let many_headers = format!(
            "GET / HTTP/1.1\r\nHost: h\r\n{}\r\n",
            (0..65)
                .map(|index| format!("X-Pad-{index}: v\r\n"))
                .collect::<String>()
        );
        let fat_block = format!(
            "GET / HTTP/1.1\r\nHost: h\r\nX-Pad: {}\r\n\r\n",
            "a".repeat(17_000)
        );
        let cases: Vec<(&str, String, &str)> = vec![
            ("request line over the limit", long_line, "request line"),
            ("65 headers", many_headers, "more than 64 request headers"),
            ("header block over the limit", fat_block, "header block"),
            (
                "chunked encoding",
                "POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"
                    .to_owned(),
                "transfer-encoding is not supported",
            ),
            (
                "a body larger than the limit",
                "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 2097152\r\n\r\n".to_owned(),
                "exceeds the 1048576-byte limit",
            ),
            (
                "a body that ends early",
                "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 12\r\n\r\nshort".to_owned(),
                "closed before the request body",
            ),
            (
                "a POST with no content-length",
                "POST /api/x HTTP/1.1\r\nHost: h\r\n\r\n".to_owned(),
                "a POST needs a content-length",
            ),
            (
                "a content-length that is not a plain number",
                "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: +5\r\n\r\nhello".to_owned(),
                "not a plain decimal number",
            ),
            (
                "two content-length headers",
                "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\nhello"
                    .to_owned(),
                "repeats its content-length header",
            ),
            (
                "two host headers",
                "GET / HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nHost: evil.com\r\n\r\n".to_owned(),
                "repeats its host header",
            ),
            (
                "an encoded traversal",
                "GET /assets/%2e%2e/config.toml HTTP/1.1\r\nHost: h\r\n\r\n".to_owned(),
                "contains '..'",
            ),
            (
                "an encoded NUL",
                "GET /assets/app.js%00.png HTTP/1.1\r\nHost: h\r\n\r\n".to_owned(),
                "contains a control byte",
            ),
            (
                "a malformed percent escape",
                "GET /assets/%zz HTTP/1.1\r\nHost: h\r\n\r\n".to_owned(),
                "not valid percent-encoded UTF-8",
            ),
            (
                "an absolute-form target",
                "GET http://evil.com/ HTTP/1.1\r\nHost: h\r\n\r\n".to_owned(),
                "must start with '/'",
            ),
            (
                "an unsupported method",
                "TRACE / HTTP/1.1\r\nHost: h\r\n\r\n".to_owned(),
                "unsupported http method",
            ),
            (
                "an unsupported version",
                "GET / HTTP/2.0\r\nHost: h\r\n\r\n".to_owned(),
                "unsupported http version",
            ),
            (
                "a header with no colon",
                "GET / HTTP/1.1\r\nHost: h\r\nBroken\r\n\r\n".to_owned(),
                "no ':' separator",
            ),
            (
                "whitespace before the colon",
                "GET / HTTP/1.1\r\nHost: h\r\nContent-Length : 5\r\n\r\n".to_owned(),
                "not a valid token",
            ),
            (
                "a folded continuation line",
                "GET / HTTP/1.1\r\nHost: h\r\n\tevil: 1\r\n\r\n".to_owned(),
                "not a valid token",
            ),
            (
                "a header block with no blank line",
                "GET / HTTP/1.1\r\nHost: h\r\n".to_owned(),
                "closed before the header block",
            ),
        ];

        for (name, raw, expected) in cases {
            let error = parse(&raw).expect_err(name).to_string();
            assert!(
                error.contains(expected),
                "{name}: unexpected error '{error}'"
            );
        }
    }

    #[test]
    fn a_request_at_each_limit_is_still_accepted() {
        // The boundary the negative cases sit just past: 64 headers and a
        // request line of exactly the ceiling must both go through.
        let padded = (0..63)
            .map(|index| format!("X-Pad-{index}: v\r\n"))
            .collect::<String>();
        let request = parse(&format!("GET / HTTP/1.1\r\nHost: h\r\n{padded}\r\n")).unwrap();
        assert_eq!(request.headers.len(), 64);

        let head = "GET /";
        let tail = " HTTP/1.1\r\n";
        let filler = "a".repeat(MAX_REQUEST_LINE - head.len() - tail.len());
        let request = parse(&format!("{head}{filler}{tail}Host: h\r\n\r\n")).unwrap();
        assert_eq!(request.path.len(), 1 + filler.len());
    }

    #[test]
    fn the_host_must_match_exactly_including_the_port() {
        let guard = guard();
        // A page at evil.com whose name resolves to 127.0.0.1 reaches this
        // server, and its Host header is the only thing that gives it away.
        let rebound = get("Host: evil.com:8787\r\n");
        assert_eq!(guard.check(&rebound, true), Err(Denied::Host));

        let no_port = get("Host: 127.0.0.1\r\n");
        assert_eq!(guard.check(&no_port, true), Err(Denied::Host));

        let missing = get("Accept: */*\r\n");
        assert_eq!(guard.check(&missing, true), Err(Denied::Host));

        let exact = get("Host: 127.0.0.1:8787\r\n");
        assert_eq!(guard.check(&exact, true), Ok(None));
        // Only the case differs, which DNS does not distinguish either.
        let cased = get("Host: ALC.Example:8787\r\n");
        assert_eq!(guard.check(&cased, true), Ok(None));
    }

    #[test]
    fn an_upgrade_needs_an_origin_and_a_plain_get_does_not() {
        let guard = guard();
        let upgrade = get("Host: 127.0.0.1:8787\r\nUpgrade: websocket\r\n");
        assert_eq!(guard.check(&upgrade, true), Err(Denied::Origin));

        let foreign =
            get("Host: 127.0.0.1:8787\r\nUpgrade: websocket\r\nOrigin: http://evil.com\r\n");
        assert_eq!(guard.check(&foreign, true), Err(Denied::Origin));

        let allowed =
            get("Host: 127.0.0.1:8787\r\nUpgrade: websocket\r\nOrigin: http://127.0.0.1:8787\r\n");
        assert_eq!(guard.check(&allowed, true), Ok(None));

        // Somebody typing the address into a phone's browser.
        let typed = get("Host: 127.0.0.1:8787\r\n");
        assert_eq!(guard.check(&typed, true), Ok(None));
    }

    #[test]
    fn a_foreign_origin_is_refused_even_on_a_public_route() {
        let request = get("Host: 127.0.0.1:8787\r\nOrigin: https://evil.com\r\n");
        assert_eq!(guard().check(&request, true), Err(Denied::Origin));
    }

    #[test]
    fn a_token_is_graded_and_anything_else_is_denied() {
        let guard = guard();
        let with = |token: &str| {
            get(&format!(
                "Host: 127.0.0.1:8787\r\nAuthorization: Bearer {token}\r\n"
            ))
        };

        assert_eq!(
            guard.check(&with(&guard.operator), false),
            Ok(Some(Grade::Operator))
        );
        assert_eq!(
            guard.check(&with(&guard.viewer), false),
            Ok(Some(Grade::Viewer))
        );

        // Right length, last byte wrong: the case a comparison that stops at
        // the first difference would answer measurably faster.
        let mut near = guard.operator.clone();
        near.pop();
        near.push('1');
        assert_eq!(near.len(), guard.operator.len());
        assert_eq!(guard.check(&with(&near), false), Err(Denied::Token));

        assert_eq!(guard.check(&with(""), false), Err(Denied::Token));
        assert_eq!(
            guard.check(&with("viewer-token"), false),
            Err(Denied::Token)
        );

        let no_header = get("Host: 127.0.0.1:8787\r\n");
        assert_eq!(guard.check(&no_header, false), Err(Denied::Token));
        let wrong_scheme = get("Host: 127.0.0.1:8787\r\nAuthorization: Basic operator-token\r\n");
        assert_eq!(guard.check(&wrong_scheme, false), Err(Denied::Token));

        assert_eq!(guard.grade_token(&guard.operator), Some(Grade::Operator));
        assert_eq!(guard.grade_token(&guard.viewer), Some(Grade::Viewer));
        assert_eq!(guard.grade_token(""), None);
        assert!(Grade::Operator > Grade::Viewer);
    }

    #[test]
    fn an_empty_configured_token_still_matches_nothing() {
        // A guard built before its secrets were loaded must not accept the
        // empty string as either grade.
        let guard = Guard {
            hosts: Vec::new(),
            origins: Vec::new(),
            operator: String::new(),
            viewer: String::new(),
        };
        assert_eq!(guard.grade_token(""), None);
    }

    #[test]
    fn a_public_route_lifts_only_the_token_check() {
        let guard = guard();
        let anonymous = get("Host: 127.0.0.1:8787\r\n");
        assert_eq!(guard.check(&anonymous, true), Ok(None));
        assert_eq!(guard.check(&anonymous, false), Err(Denied::Token));

        // A token that was sent is still graded on a public route.
        let carried = get(&format!(
            "Host: 127.0.0.1:8787\r\nAuthorization: Bearer {}\r\n",
            guard.viewer
        ));
        assert_eq!(guard.check(&carried, true), Ok(Some(Grade::Viewer)));

        // …but a bad Host is refused whether the route is public or not.
        let rebound = get("Host: evil.com:8787\r\n");
        assert_eq!(guard.check(&rebound, true), Err(Denied::Host));
    }

    #[test]
    fn the_constant_time_compare_agrees_with_eq() {
        let inputs = [
            "",
            "a",
            "b",
            "ab",
            "abc",
            "abd",
            "abcd",
            "operator-token-0000000000000000",
            "operator-token-0000000000000001",
            "viewer-token-000000000000000000",
            "\u{0}",
            "ünïcödé",
        ];
        for left in inputs {
            for right in inputs {
                assert_eq!(
                    constant_time_eq(left.as_bytes(), right.as_bytes()),
                    left == right,
                    "{left:?} vs {right:?}"
                );
            }
        }

        // Every single-byte difference at every position, since a fold that
        // dropped a byte would still agree with `==` on the cases above.
        let base = b"operator-token-0000000000000000";
        for index in 0..base.len() {
            let mut flipped = *base;
            flipped[index] ^= 0x20;
            assert!(!constant_time_eq(base, &flipped), "position {index}");
            assert!(constant_time_eq(base, base));
        }
    }
}
