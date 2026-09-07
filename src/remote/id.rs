//! Naming sessions.
//!
//! Two names, for two audiences. The id is what the wire and the URL use:
//! unguessable, stable, and safe in a path. The display name is what the
//! user reads on a phone, so it answers the only question they actually
//! have from the lock screen - which agent, in which project.

use std::path::Path;

use anyhow::{Result, bail};

use crate::config::Agent;

/// Crockford's alphabet: no I, L, O or U, so an id read aloud or typed from
/// a screenshot cannot become a different one.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// 50 bits. Not a credential - the token is - but long enough that ids do
/// not collide and short enough to read out.
const ID_LEN: usize = 10;

/// Builds a session id: the agent's name, then random characters.
///
/// The agent name is in the id on purpose. It shows up in the URL, in
/// `alc sessions`, and in any log a user pastes into an issue, and a bare
/// random string there tells nobody anything.
pub(crate) fn generate(agent: Agent) -> Result<String> {
    let mut bytes = [0_u8; ID_LEN];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("failed to read system randomness: {error}"))?;
    let suffix: String = bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte >> 3)]))
        .collect();
    Ok(format!("{}-{suffix}", agent.as_str()))
}

/// The name shown on a card. `claude@all-code` reads better on a phone than
/// a path or an id, and the working directory's own name is what the user
/// calls the project.
pub(crate) fn default_name(agent: Agent, cwd: &Path) -> String {
    let project = cwd
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("~");
    format!("{}@{project}", agent.as_str())
}

/// Session names are typed on a command line and rendered into HTML, so
/// they are held to the same charset as a provider profile: identifiers,
/// not free text.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("a session name cannot be empty");
    }
    if name.len() > 64 {
        bail!("a session name cannot be longer than 64 characters");
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-_.@".contains(character))
    {
        bail!(
            "session name '{name}' has characters other than letters, digits, '-', '_', '.' and '@'"
        );
    }
    Ok(())
}

/// Resolves an unambiguous id prefix, the way git resolves a short hash.
/// Typing ten random characters to stop a session is not something anyone
/// should have to do.
///
/// Matches the whole id OR the random part on its own, because the part a
/// person reads off a screen and retypes is the distinctive tail, not the
/// agent name they already know. Case-insensitive for the same reason: the
/// alphabet is upper case and nobody wants to hold shift for it.
pub(crate) fn resolve_prefix<'a, I>(ids: I, prefix: &str) -> Result<String>
where
    I: IntoIterator<Item = &'a String>,
{
    let wanted = prefix.to_ascii_lowercase();
    let matches: Vec<&String> = ids
        .into_iter()
        .filter(|id| {
            let full = id.to_ascii_lowercase();
            let tail = full.split_once('-').map(|(_, tail)| tail.to_owned());
            full.starts_with(&wanted) || tail.is_some_and(|tail| tail.starts_with(&wanted))
        })
        .collect();
    match matches.as_slice() {
        [single] => Ok((*single).clone()),
        [] => bail!("no session matches '{prefix}'; run `alc sessions` to list them"),
        many => {
            let list = many
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            bail!("'{prefix}' matches more than one session: {list}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_names_its_agent_and_is_the_expected_shape() {
        let id = generate(Agent::Claude).unwrap();
        assert!(id.starts_with("claude-"), "{id}");
        let suffix = id.trim_start_matches("claude-");
        assert_eq!(suffix.len(), ID_LEN);
        assert!(
            suffix.bytes().all(|byte| ALPHABET.contains(&byte)),
            "{suffix}"
        );
    }

    #[test]
    fn ids_do_not_repeat() {
        let first = generate(Agent::Codex).unwrap();
        let second = generate(Agent::Codex).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn the_alphabet_omits_the_letters_that_are_read_wrong() {
        for confusable in [b'I', b'L', b'O', b'U'] {
            assert!(
                !ALPHABET.contains(&confusable),
                "{} is ambiguous when read aloud",
                char::from(confusable)
            );
        }
    }

    #[test]
    fn a_default_name_says_which_agent_in_which_project() {
        assert_eq!(
            default_name(Agent::Claude, Path::new("/home/x/all-code")),
            "claude@all-code"
        );
    }

    #[test]
    fn a_default_name_survives_a_directory_with_no_basename() {
        assert_eq!(default_name(Agent::Goose, Path::new("/")), "goose@~");
    }

    #[test]
    fn a_name_with_html_or_shell_characters_is_refused() {
        // These names are rendered into a page and typed on a command line.
        for hostile in ["<script>", "a b", "a/b", "a;b", "a'b"] {
            assert!(validate_name(hostile).is_err(), "{hostile} was accepted");
        }
        assert!(validate_name("claude@all-code").is_ok());
    }

    #[test]
    fn a_prefix_resolves_only_when_it_is_unambiguous() {
        let ids = vec![
            "claude-ABCDEFGHJK".to_owned(),
            "claude-ABCDZZZZZZ".to_owned(),
            "codex-0123456789".to_owned(),
        ];
        assert_eq!(resolve_prefix(&ids, "codex").unwrap(), "codex-0123456789");
        assert!(
            resolve_prefix(&ids, "claude-ABCD")
                .unwrap_err()
                .to_string()
                .contains("more than one")
        );
        assert!(resolve_prefix(&ids, "nothing").is_err());
    }

    #[test]
    fn the_distinctive_tail_resolves_on_its_own() {
        // What a person reads off a phone and retypes is the random part,
        // not the agent name they already know.
        let ids = vec![
            "codex-68B8XMJ6F5".to_owned(),
            "claude-ABCDEFGHJK".to_owned(),
        ];
        assert_eq!(resolve_prefix(&ids, "68B8").unwrap(), "codex-68B8XMJ6F5");
        assert_eq!(resolve_prefix(&ids, "ABCD").unwrap(), "claude-ABCDEFGHJK");
    }

    #[test]
    fn a_prefix_does_not_have_to_be_shouted() {
        // The alphabet is upper case; nobody wants to hold shift for it.
        let ids = vec!["codex-68B8XMJ6F5".to_owned()];
        assert_eq!(resolve_prefix(&ids, "68b8").unwrap(), "codex-68B8XMJ6F5");
        assert_eq!(resolve_prefix(&ids, "CODEX").unwrap(), "codex-68B8XMJ6F5");
    }

    #[test]
    fn an_ambiguous_tail_is_still_refused() {
        let ids = vec![
            "codex-ABCDEFGHJK".to_owned(),
            "claude-ABCDEFGHJK".to_owned(),
        ];
        assert!(resolve_prefix(&ids, "ABCD").is_err());
    }
}
