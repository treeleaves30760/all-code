//! Stable Codex routing for one Claude Code conversation.
//!
//! Claude Code supplies a session id on every model request and a direct agent
//! id on requests made by a subagent. Codex uses the session for prompt-cache
//! routing and a separate thread lane for each root or agent. Bad identity is
//! not a bad model request: it simply gets no affinity metadata.

use reqwest::header::{HeaderMap, HeaderValue};

pub(crate) const CLAUDE_SESSION_HEADER: &str = "x-claude-code-session-id";
pub(crate) const CLAUDE_AGENT_HEADER: &str = "x-claude-code-agent-id";
pub(crate) const CLAUDE_PARENT_AGENT_HEADER: &str = "x-claude-code-parent-agent-id";

const MAX_IDENTITY_LEN: usize = 512;

/// The two independent identities Codex's Responses transport expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestAffinity {
    /// Shared by the root and every agent in one Claude Code session.
    pub cache_key: String,
    /// The root session or the direct agent that issued this request.
    pub thread_id: String,
}

impl RequestAffinity {
    pub(crate) fn from_headers(headers: &HeaderMap) -> Option<Self> {
        let session = read_identity_header(headers, CLAUDE_SESSION_HEADER);
        let agent = read_identity_header(headers, CLAUDE_AGENT_HEADER);
        let parent = read_identity_header(headers, CLAUDE_PARENT_AGENT_HEADER);

        if session.is_invalid() || agent.is_invalid() || parent.is_invalid() {
            return None;
        }

        match (session.value(), agent.value(), parent.value()) {
            (Some(session), Some(agent), _) => Some(Self {
                cache_key: session.to_owned(),
                thread_id: agent.to_owned(),
            }),
            (Some(session), None, None) => Some(Self {
                cache_key: session.to_owned(),
                thread_id: session.to_owned(),
            }),
            _ => None,
        }
    }

    /// Adds only the current headers emitted by the Codex client. The cache key
    /// doubles as its session id; the request id doubles as its thread id.
    pub(crate) fn apply_headers(&self, headers: &mut HeaderMap) -> Result<(), &'static str> {
        let session = HeaderValue::from_str(&self.cache_key)
            .map_err(|_| "Claude Code's session id cannot be sent as a Codex header")?;
        let thread = HeaderValue::from_str(&self.thread_id)
            .map_err(|_| "Claude Code's agent id cannot be sent as a Codex header")?;
        headers.insert("session-id", session);
        headers.insert("thread-id", thread.clone());
        headers.insert("x-client-request-id", thread);
        Ok(())
    }
}

#[derive(Debug)]
enum ParsedHeader<'a> {
    Missing,
    Valid(&'a str),
    Invalid,
}

impl ParsedHeader<'_> {
    fn value(&self) -> Option<&str> {
        match self {
            Self::Valid(value) => Some(value),
            Self::Missing | Self::Invalid => None,
        }
    }

    fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid)
    }
}

fn read_identity_header<'a>(headers: &'a HeaderMap, name: &str) -> ParsedHeader<'a> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return ParsedHeader::Missing;
    };
    if values.next().is_some() {
        return ParsedHeader::Invalid;
    }

    let Ok(value) = value.to_str() else {
        return ParsedHeader::Invalid;
    };
    let value = value.trim_matches(|character| matches!(character, ' ' | '\t'));
    if value.is_empty()
        || value.len() > MAX_IDENTITY_LEN
        || value.contains(',')
        || !value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return ParsedHeader::Invalid;
    }

    ParsedHeader::Valid(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};

    fn headers(values: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in values {
            headers.append(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn root_and_agents_share_cache_routing_but_have_distinct_threads() {
        let root = RequestAffinity::from_headers(&headers(&[(CLAUDE_SESSION_HEADER, "session-a")]))
            .unwrap();
        let first = RequestAffinity::from_headers(&headers(&[
            (CLAUDE_SESSION_HEADER, "session-a"),
            (CLAUDE_AGENT_HEADER, "agent-one"),
        ]))
        .unwrap();
        let second = RequestAffinity::from_headers(&headers(&[
            (CLAUDE_SESSION_HEADER, "session-a"),
            (CLAUDE_AGENT_HEADER, "agent-two"),
            (CLAUDE_PARENT_AGENT_HEADER, "agent-parent"),
        ]))
        .unwrap();

        assert_eq!(root.cache_key, "session-a");
        assert_eq!(first.cache_key, root.cache_key);
        assert_eq!(second.cache_key, root.cache_key);
        assert_eq!(root.thread_id, "session-a");
        assert_eq!(first.thread_id, "agent-one");
        assert_eq!(second.thread_id, "agent-two");
    }

    #[test]
    fn identity_is_stable_and_sessions_remain_distinct() {
        let affinity =
            |session| RequestAffinity::from_headers(&headers(&[(CLAUDE_SESSION_HEADER, session)]));
        assert_eq!(affinity("session-a"), affinity("session-a"));
        assert_ne!(affinity("session-a"), affinity("session-b"));
    }

    #[test]
    fn parent_is_validation_only() {
        let affinity = |parent: Option<&str>| {
            let mut values = vec![
                (CLAUDE_SESSION_HEADER, "session-a"),
                (CLAUDE_AGENT_HEADER, "agent-child"),
            ];
            if let Some(parent) = parent {
                values.push((CLAUDE_PARENT_AGENT_HEADER, parent));
            }
            RequestAffinity::from_headers(&headers(&values))
        };
        assert_eq!(affinity(None), affinity(Some("agent-parent")));
        assert_eq!(
            affinity(Some("agent-parent")),
            affinity(Some("other-parent"))
        );
    }

    #[test]
    fn ambiguous_or_absent_tuples_are_stateless() {
        for values in [
            vec![],
            vec![(CLAUDE_AGENT_HEADER, "agent-a")],
            vec![(CLAUDE_PARENT_AGENT_HEADER, "agent-parent")],
            vec![
                (CLAUDE_SESSION_HEADER, "session-a"),
                (CLAUDE_PARENT_AGENT_HEADER, "agent-parent"),
            ],
            vec![
                (CLAUDE_AGENT_HEADER, "agent-a"),
                (CLAUDE_PARENT_AGENT_HEADER, "agent-parent"),
            ],
        ] {
            assert_eq!(RequestAffinity::from_headers(&headers(&values)), None);
        }
    }

    #[test]
    fn trims_outer_space_and_tab() {
        assert_eq!(
            RequestAffinity::from_headers(&headers(&[
                (CLAUDE_SESSION_HEADER, " \tsession-a\t "),
                (CLAUDE_AGENT_HEADER, "\tagent-a "),
                (CLAUDE_PARENT_AGENT_HEADER, " parent\t"),
            ])),
            Some(RequestAffinity {
                cache_key: "session-a".to_owned(),
                thread_id: "agent-a".to_owned(),
            })
        );
    }

    #[test]
    fn malformed_text_in_any_identity_field_is_stateless() {
        for field in [
            CLAUDE_SESSION_HEADER,
            CLAUDE_AGENT_HEADER,
            CLAUDE_PARENT_AGENT_HEADER,
        ] {
            for value in [
                String::new(),
                "   ".to_owned(),
                "two values".to_owned(),
                "two\tvalues".to_owned(),
                "first,second".to_owned(),
                "x".repeat(MAX_IDENTITY_LEN + 1),
            ] {
                let mut values = vec![
                    (CLAUDE_SESSION_HEADER, "session-a"),
                    (CLAUDE_AGENT_HEADER, "agent-a"),
                    (CLAUDE_PARENT_AGENT_HEADER, "agent-parent"),
                ];
                values
                    .iter_mut()
                    .find(|(name, _)| *name == field)
                    .unwrap()
                    .1 = &value;
                assert_eq!(RequestAffinity::from_headers(&headers(&values)), None);
            }
        }
    }

    #[test]
    fn duplicate_or_nontext_identity_fields_are_stateless() {
        for field in [
            CLAUDE_SESSION_HEADER,
            CLAUDE_AGENT_HEADER,
            CLAUDE_PARENT_AGENT_HEADER,
        ] {
            let mut duplicate = headers(&[
                (CLAUDE_SESSION_HEADER, "session-a"),
                (CLAUDE_AGENT_HEADER, "agent-a"),
                (CLAUDE_PARENT_AGENT_HEADER, "agent-parent"),
            ]);
            duplicate.append(field, HeaderValue::from_static("duplicate"));
            assert_eq!(RequestAffinity::from_headers(&duplicate), None);

            let mut nontext = headers(&[
                (CLAUDE_SESSION_HEADER, "session-a"),
                (CLAUDE_AGENT_HEADER, "agent-a"),
                (CLAUDE_PARENT_AGENT_HEADER, "agent-parent"),
            ]);
            nontext.insert(field, HeaderValue::from_bytes(&[0x80]).unwrap());
            assert_eq!(RequestAffinity::from_headers(&nontext), None);
        }
    }

    #[test]
    fn a_malformed_agent_never_downgrades_to_the_main_session() {
        for agent in ["", "agent one", "agent-a,agent-b"] {
            assert_eq!(
                RequestAffinity::from_headers(&headers(&[
                    (CLAUDE_SESSION_HEADER, "session-a"),
                    (CLAUDE_AGENT_HEADER, agent),
                ])),
                None
            );
        }
    }

    #[test]
    fn current_codex_headers_share_session_and_match_thread_request_ids() {
        let affinity = RequestAffinity {
            cache_key: "session-a".to_owned(),
            thread_id: "agent-a".to_owned(),
        };
        let mut headers = HeaderMap::new();
        affinity.apply_headers(&mut headers).unwrap();
        assert_eq!(headers["session-id"], "session-a");
        assert_eq!(headers["thread-id"], "agent-a");
        assert_eq!(headers["x-client-request-id"], "agent-a");
        assert!(headers.get("session_id").is_none());
    }
}
