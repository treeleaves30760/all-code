//! What chatgpt.com says this account may drive.
//!
//! alc's bridge posts every turn to [`upstream::CODEX_RESPONSES_URL`]. The
//! model list has to come from the same party, or the two can disagree about
//! what exists - and they did: a model chatgpt.com streamed happily on
//! request was missing from every picker alc built, because the list came
//! from a locally installed Codex that alc does not route a single byte
//! through.
//!
//! This is a source, not an authority. [`crate::model_catalog`] puts the
//! models alc ships back over whatever comes out of here, so a refused
//! request, an expired token or a captive portal costs freshness and never
//! completeness - which is what lets this sit in front of a launch at all.

use std::time::Duration;

use crate::usage::accounts::CodexLogin;

use super::upstream;

pub(crate) const CODEX_MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";

/// Measured at just under 400 KiB: most of the payload is per-model prompt
/// text alc parses past and throws away. `usage::quota` caps its bodies at
/// 256 KiB because a quota answer larger than that is not the endpoint it
/// thinks it is; that reasoning does not carry over, and reusing the number
/// would truncate every single response and make the fetch fail on every
/// machine while looking like a network fault.
const CATALOG_BODY_LIMIT: u64 = 4 * 1024 * 1024;

/// Split rather than one global budget: this runs in front of a launch, so a
/// machine with no route has to give up in seconds, while a slow link
/// carrying four hundred kilobytes needs longer than that to finish.
const CATALOG_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const CATALOG_TIMEOUT: Duration = Duration::from_secs(12);

/// The catalog body, as JSON text for the caller to parse.
///
/// `client_version` is the caller's problem on purpose: chatgpt.com gates
/// each model on a `minimal_client_version` and compares it against whatever
/// the caller declares, so the number decides the answer, and deciding it
/// belongs next to the code that knows which Codex is installed.
///
/// The expiry is checked here and the token is never rotated. A Codex refresh
/// token is single-use and its rotation is single-flighted behind the
/// bridge's tokio mutex; a blocking caller cannot join that, so refreshing
/// from here could retire a token a live session is about to sign with.
pub(crate) fn fetch_account_models(
    login: &CodexLogin,
    client_version: &str,
    now_ms: u64,
) -> Result<String, String> {
    if login.expires_at_ms <= now_ms {
        // Word for word what `usage::quota::fetch_codex` says, so alc has one
        // sentence for this and the user is not told two different stories
        // about the same expired login.
        return Err("the Codex token has expired; run `codex login`, or start a bridged session once so the adapter refreshes it"
            .to_owned());
    }

    let url = format!("{}?client_version={client_version}", endpoint());
    let bearer = format!("Bearer {}", login.access_token);
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(CATALOG_CONNECT_TIMEOUT))
        .timeout_global(Some(CATALOG_TIMEOUT))
        // A 401 body says which login was refused; throwing it away would
        // leave alc guessing at the reason.
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut request = agent
        .get(&url)
        .header("authorization", &bearer)
        .header("accept", "application/json")
        // Both carry the Codex user agent, like every other authenticated
        // call alc makes to chatgpt.com. A different string would put alc in
        // a rate-limit bucket nobody has measured, for no gain.
        .header("user-agent", upstream::USER_AGENT)
        .header("originator", upstream::USER_AGENT);
    // Unlike the usage endpoint this one does not need the account id, so a
    // login that carries none must not be turned away before it is asked.
    if let Some(account) = login.account_id.as_deref() {
        request = request.header("chatgpt-account-id", account);
    }

    let mut response = request
        .call()
        .map_err(|error| format!("could not reach chatgpt.com: {error}"))?;
    let status = response.status().as_u16();
    match status {
        200..=299 => {}
        400 => {
            return Err(format!(
                "chatgpt.com rejected client_version {client_version}"
            ));
        }
        401 | 403 => {
            return Err("chatgpt.com refused the Codex login; run `codex login`".to_owned());
        }
        429 => return Err("chatgpt.com is rate limiting alc; try again later".to_owned()),
        other => return Err(format!("chatgpt.com answered {other}")),
    }
    response
        .body_mut()
        .with_config()
        .limit(CATALOG_BODY_LIMIT)
        .read_to_string()
        .map_err(|error| format!("could not read chatgpt.com's model catalog: {error}"))
}

/// The real endpoint, or the same path on `ALC_CODEX_API_BASE` in a debug
/// build.
///
/// Compiled out of release builds for the same reason `usage::quota` compiles
/// its override out: a shipped binary must not be pointable at a host that
/// did not issue the token it is about to send.
fn endpoint() -> String {
    #[cfg(debug_assertions)]
    if let Ok(base) = std::env::var("ALC_CODEX_API_BASE")
        && !base.is_empty()
    {
        return format!("{}/backend-api/codex/models", base.trim_end_matches('/'));
    }
    CODEX_MODELS_URL.to_owned()
}
