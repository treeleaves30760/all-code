//! The reports alc prints are already styled with SGR escapes; this reads
//! them back into ratatui text, so the interactive views show exactly what
//! the plain report would, colours included.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// One line of report text, with its escapes resolved into styles.
pub(crate) fn line(text: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut style = Style::default();
    let mut run = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            if ch != '\r' {
                run.push(ch);
            }
            continue;
        }
        // CSI: ESC [ params final. Anything else is a two-byte escape.
        if chars.peek() != Some(&'[') {
            chars.next();
            continue;
        }
        chars.next();
        let mut params = String::new();
        let mut terminator = None;
        for next in chars.by_ref() {
            if next.is_ascii_alphabetic() || next == '~' {
                terminator = Some(next);
                break;
            }
            params.push(next);
        }
        if terminator != Some('m') {
            continue;
        }
        if !run.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut run), style));
        }
        style = apply_sgr(style, &params);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, style));
    }
    Line::from(spans)
}

/// The text with every escape removed, for searching and measuring.
pub(crate) fn strip(text: &str) -> String {
    line(text)
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn apply_sgr(mut style: Style, params: &str) -> Style {
    let codes: Vec<u16> = if params.is_empty() {
        vec![0]
    } else {
        params
            .split(';')
            .map(|code| code.parse().unwrap_or(0))
            .collect()
    };
    let mut index = 0;
    while index < codes.len() {
        let code = codes[index];
        match code {
            0 => style = Style::default(),
            1 => style = style.add_modifier(Modifier::BOLD),
            2 => style = style.add_modifier(Modifier::DIM),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            7 => style = style.add_modifier(Modifier::REVERSED),
            22 => style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style = style.remove_modifier(Modifier::ITALIC),
            24 => style = style.remove_modifier(Modifier::UNDERLINED),
            27 => style = style.remove_modifier(Modifier::REVERSED),
            30..=37 => style = style.fg(basic(code - 30, false)),
            39 => style.fg = None,
            40..=47 => style = style.bg(basic(code - 40, false)),
            49 => style.bg = None,
            90..=97 => style = style.fg(basic(code - 90, true)),
            100..=107 => style = style.bg(basic(code - 100, true)),
            38 | 48 => {
                let (color, used) = extended(&codes[index + 1..]);
                if let Some(color) = color {
                    style = if code == 38 {
                        style.fg(color)
                    } else {
                        style.bg(color)
                    };
                }
                index += used;
            }
            _ => {}
        }
        index += 1;
    }
    style
}

fn extended(rest: &[u16]) -> (Option<Color>, usize) {
    match rest {
        [5, value, ..] => (Some(Color::Indexed(*value as u8)), 2),
        [2, red, green, blue, ..] => (Some(Color::Rgb(*red as u8, *green as u8, *blue as u8)), 4),
        _ => (None, rest.len()),
    }
}

fn basic(index: u16, bright: bool) -> Color {
    match (index, bright) {
        (0, false) => Color::Black,
        (1, false) => Color::Red,
        (2, false) => Color::Green,
        (3, false) => Color::Yellow,
        (4, false) => Color::Blue,
        (5, false) => Color::Magenta,
        (6, false) => Color::Cyan,
        (7, false) => Color::Gray,
        (0, true) => Color::DarkGray,
        (1, true) => Color::LightRed,
        (2, true) => Color::LightGreen,
        (3, true) => Color::LightYellow,
        (4, true) => Color::LightBlue,
        (5, true) => Color::LightMagenta,
        (6, true) => Color::LightCyan,
        _ => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_weights_become_styles() {
        let parsed = line("  \x1b[32m✓\x1b[0m  claude  \x1b[1mbold\x1b[0m");
        let spans: Vec<_> = parsed
            .spans
            .iter()
            .map(|span| (span.content.as_ref(), span.style))
            .collect();
        assert_eq!(spans[0], ("  ", Style::default()));
        assert_eq!(spans[1], ("✓", Style::default().fg(Color::Green)));
        assert_eq!(spans[2], ("  claude  ", Style::default()));
        assert_eq!(
            spans[3],
            ("bold", Style::default().add_modifier(Modifier::BOLD))
        );
    }

    #[test]
    fn grey_and_indexed_colours_parse_and_other_escapes_vanish() {
        let parsed = line("\x1b[90mdim\x1b[38;5;208m orange\x1b[2K!");
        assert_eq!(parsed.spans[0].style.fg, Some(Color::DarkGray));
        assert_eq!(parsed.spans[1].style.fg, Some(Color::Indexed(208)));
        assert_eq!(
            strip("\x1b[90mdim\x1b[38;5;208m orange\x1b[2K!"),
            "dim orange!"
        );
    }
}
