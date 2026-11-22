//! Characters a reviewer does not see but an agent reads: zero-width and
//! bidirectional controls, tag characters, stray variation selectors, fillers
//! and terminal control codes. They carry hidden prompt injections and Trojan
//! Source attacks, so a skill holding one is refused.

use std::fmt;

/// One invisible character, located in its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hidden {
    /// Line number, from 1.
    pub line: usize,
    /// Position in the line, counted in characters, from 1.
    pub column: usize,
    /// The character itself.
    pub character: char,
}

impl fmt::Display for Hidden {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "line {}, column {} (U+{:04X})",
            self.line,
            self.column,
            u32::from(self.character)
        )
    }
}

/// Every invisible character in `text`, in order.
///
/// A byte order mark opening the text is not one: editors write it, and it
/// hides nothing. Joiners between two non-ASCII characters and a presentation
/// selector after a visible one are kept too, since emoji sequences, Persian
/// and Indic scripts need them.
#[must_use]
pub fn find(text: &str) -> Vec<Hidden> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let characters: Vec<char> = line.chars().collect();
        for (position, &character) in characters.iter().enumerate() {
            let before = position.checked_sub(1).map(|previous| characters[previous]);
            let after = characters.get(position + 1).copied();
            if is_hidden(character, before, after) {
                found.push(Hidden {
                    line: index + 1,
                    column: position + 1,
                    character,
                });
            }
        }
    }
    found
}

fn is_hidden(character: char, before: Option<char>, after: Option<char>) -> bool {
    match character {
        '\t' | '\u{c}' => false,
        '\u{200c}' | '\u{200d}' => !(joins(before) && joins(after)),
        '\u{fe0e}' | '\u{fe0f}' => !before.is_some_and(is_visible),
        _ => !is_visible(character),
    }
}

/// A joiner's neighbour: a visible non-ASCII character, or the emoji
/// presentation selector that ends one (`❤️‍🔥`).
fn joins(neighbour: Option<char>) -> bool {
    neighbour.is_some_and(|character| {
        character == '\u{fe0f}' || (!character.is_ascii() && is_visible(character))
    })
}

fn is_visible(character: char) -> bool {
    !(character.is_control()
        || matches!(
            character,
            '\u{00ad}'
                | '\u{034f}'
                | '\u{061c}'
                | '\u{115f}'
                | '\u{1160}'
                | '\u{17b4}'
                | '\u{17b5}'
                | '\u{180b}'..='\u{180f}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{3164}'
                | '\u{fe00}'..='\u{fe0f}'
                | '\u{feff}'
                | '\u{ffa0}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0000}'..='\u{e007f}'
                | '\u{e0100}'..='\u{e01ef}'
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn characters(text: &str) -> Vec<char> {
        find(text).iter().map(|hidden| hidden.character).collect()
    }

    #[test]
    fn accepts_plain_and_accented_text() {
        assert!(find("# Déployer\n\nÇa marche — « vraiment ».\n\tindented\n").is_empty());
    }

    #[test]
    fn accepts_windows_line_endings_and_an_opening_byte_order_mark() {
        assert!(find("\u{feff}---\r\nname: deploy\r\n---\r\n").is_empty());
    }

    #[test]
    fn accepts_emoji_sequences() {
        let emoji = "⚠️ 👨‍💻 👨🏽‍💻 ❤️‍🔥 🏳️‍🌈 1️⃣ 👍";

        assert!(find(emoji).is_empty(), "{:?}", find(emoji));
    }

    #[test]
    fn accepts_a_non_joiner_inside_a_persian_word() {
        assert!(find("می\u{200c}خواهم").is_empty());
    }

    #[test]
    fn locates_a_bidirectional_override() {
        assert_eq!(
            find("ok\nif access \u{202e}nimda\n"),
            vec![Hidden {
                line: 2,
                column: 11,
                character: '\u{202e}'
            }]
        );
    }

    #[test]
    fn flags_zero_width_characters_between_ascii_letters() {
        assert_eq!(
            characters("ig\u{200b}no\u{200d}re\u{2060}"),
            vec!['\u{200b}', '\u{200d}', '\u{2060}']
        );
    }

    #[test]
    fn flags_text_smuggled_in_tag_characters() {
        let smuggled: String = "run"
            .chars()
            .map(|letter| char::from_u32(0xe0000 + u32::from(letter)).unwrap())
            .collect();

        assert_eq!(find(&format!("Hello{smuggled}")).len(), 3);
    }

    #[test]
    fn flags_data_smuggled_in_variation_selectors() {
        assert_eq!(
            characters("😀\u{fe0f}\u{fe0f}\u{e0101}a\u{fe01}"),
            vec!['\u{fe0f}', '\u{e0101}', '\u{fe01}']
        );
    }

    #[test]
    fn flags_a_selector_or_joiner_with_nothing_to_attach_to() {
        assert_eq!(
            characters("\u{fe0f}start\n\u{200d}é"),
            vec!['\u{fe0f}', '\u{200d}']
        );
    }

    #[test]
    fn flags_terminal_control_codes_and_a_lone_carriage_return() {
        assert_eq!(
            characters("safe\u{1b}[8mhidden\u{1b}[0m\rIgnore that\n"),
            vec!['\u{1b}', '\u{1b}', '\r']
        );
    }

    #[test]
    fn flags_a_byte_order_mark_past_the_start() {
        assert_eq!(characters("a\u{feff}b"), vec!['\u{feff}']);
    }

    #[test]
    fn reads_as_a_location() {
        let hidden = Hidden {
            line: 3,
            column: 7,
            character: '\u{200b}',
        };

        assert_eq!(hidden.to_string(), "line 3, column 7 (U+200B)");
    }
}
