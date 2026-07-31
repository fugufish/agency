//! Mechanical helpers every provider needs to read skills and commands off
//! disk. The shapes are the same everywhere — a fenced frontmatter block
//! followed by markdown — so only the directory layouts and naming rules stay
//! provider specific.

/// What a completion row shows when a file offers no description at all.
pub const DEFAULT_DESCRIPTION: &str = "Agent skill or command";

/// The frontmatter keys that affect how a command is listed. Everything else
/// in the block governs the agent's own behavior and is none of Agency's
/// business.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
}

/// Reads the fenced frontmatter block, if the file opens with one.
///
/// Values are single-line scalars. A folded or block scalar (`>` or `|`)
/// yields nothing for that key rather than the marker character, so a file
/// using one falls back to its prose line.
pub fn frontmatter(contents: &str) -> Frontmatter {
    let mut parsed = Frontmatter::default();
    for line in frontmatter_lines(contents) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "name" => parsed.name = Some(value.to_owned()),
            "description" => parsed.description = Some(value.to_owned()),
            "argument-hint" => parsed.argument_hint = Some(value.to_owned()),
            _ => {}
        }
    }
    parsed
}

/// The description to list a command under: its frontmatter `description`, the
/// first prose line of its body, or a generic label.
pub fn describe(contents: &str) -> String {
    if let Some(description) = frontmatter(contents).description {
        return description;
    }
    body(contents)
        .lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .map_or_else(|| DEFAULT_DESCRIPTION.to_owned(), str::to_owned)
}

/// The lines inside the frontmatter fence, or none when the file does not open
/// with one. An unterminated fence is not frontmatter: treating it as such
/// would swallow a whole file that merely starts with a horizontal rule.
fn frontmatter_lines(contents: &str) -> impl Iterator<Item = &str> {
    let inside = contents
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---").map(|(block, _)| block))
        .unwrap_or_default();
    inside.lines()
}

/// Everything after the frontmatter fence, or the whole file without one.
fn body(contents: &str) -> &str {
    contents
        .strip_prefix("---\n")
        .map(|rest| {
            rest.split_once("\n---")
                .map(|(_, body)| body)
                .unwrap_or(rest)
        })
        .unwrap_or(contents)
}

/// Strips one matching pair of surrounding quotes. A value like
/// `"Use when: you are stuck"` has to survive intact, colon and all.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|value| value.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL: &str = "---\nname: brainstorming\ndescription: Turn ideas into designs\nargument-hint: [topic]\n---\n\n# Brainstorming\n\nBody text.\n";

    #[test]
    fn frontmatter_keys_are_read_by_name() {
        let parsed = frontmatter(SKILL);
        assert_eq!(parsed.name.as_deref(), Some("brainstorming"));
        assert_eq!(parsed.description.as_deref(), Some("Turn ideas into designs"));
        assert_eq!(parsed.argument_hint.as_deref(), Some("[topic]"));
    }

    /// Regression: the description used to be taken as the first line that was
    /// neither blank nor a fence, which is the `name` key in every real file.
    #[test]
    fn the_description_is_not_the_name_key() {
        assert_eq!(describe(SKILL), "Turn ideas into designs");
    }

    #[test]
    fn values_keep_colons_and_lose_surrounding_quotes() {
        let parsed = frontmatter("---\ndescription: \"Use when: you are stuck\"\n---\n");
        assert_eq!(
            parsed.description.as_deref(),
            Some("Use when: you are stuck")
        );
        let single = frontmatter("---\ndescription: 'Quoted'\n---\n");
        assert_eq!(single.description.as_deref(), Some("Quoted"));
    }

    #[test]
    fn a_file_without_frontmatter_falls_back_to_its_first_prose_line() {
        assert_eq!(describe("# Deploy\n\nShips the app.\n"), "Deploy");
        assert_eq!(describe("Ships the app.\n"), "Ships the app.");
    }

    #[test]
    fn an_unterminated_block_is_not_treated_as_frontmatter() {
        let parsed = frontmatter("---\ndescription: never closed\n");
        assert_eq!(parsed.description, None);
        assert_eq!(describe("---\ndescription: never closed\n"), "description: never closed");
    }

    #[test]
    fn an_empty_or_bodyless_file_falls_back_to_the_default() {
        assert_eq!(describe(""), DEFAULT_DESCRIPTION);
        assert_eq!(describe("---\nname: bare\n---\n"), DEFAULT_DESCRIPTION);
    }

    #[test]
    fn blank_values_and_unknown_keys_are_ignored() {
        let parsed = frontmatter("---\ndescription:\nmodel: opus\n---\nBody\n");
        assert_eq!(parsed.description, None);
        assert_eq!(describe("---\ndescription:\nmodel: opus\n---\nBody\n"), "Body");
    }
}
