//! A three-row block font for the big latency figure.

/// The text drawn as three rows of block characters, one space between glyphs. Characters
/// without a glyph become a gap of the same width as a digit.
pub fn big(text: &str) -> [String; 3] {
    let mut rows = [String::new(), String::new(), String::new()];
    for (i, c) in text.chars().enumerate() {
        let glyph = glyph(c);
        for (row, part) in rows.iter_mut().zip(glyph) {
            if i > 0 {
                row.push(' ');
            }
            row.push_str(part);
        }
    }
    rows
}

fn glyph(c: char) -> [&'static str; 3] {
    match c {
        '0' => ["█▀█", "█ █", "▀▀▀"],
        '1' => ["▀█ ", " █ ", "▀▀▀"],
        '2' => ["▀▀█", "█▀▀", "▀▀▀"],
        '3' => ["▀▀█", " ▀█", "▀▀▀"],
        '4' => ["█ █", "▀▀█", "  ▀"],
        '5' => ["█▀▀", "▀▀█", "▀▀▀"],
        '6' => ["█▀▀", "█▀█", "▀▀▀"],
        '7' => ["▀▀█", "  █", "  ▀"],
        '8' => ["█▀█", "█▀█", "▀▀▀"],
        '9' => ["█▀█", "▀▀█", "▀▀▀"],
        '.' => [" ", " ", "▀"],
        _ => ["   ", "   ", "   "],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width(row: &str) -> usize {
        row.chars().count()
    }

    #[test]
    fn digits_render() {
        let rows = big("9.0");
        assert_eq!(width(&rows[0]), width(&rows[1]));
        assert_eq!(width(&rows[1]), width(&rows[2]));
        // 3 + 1 + 1 + 1 + 3: digit, gap, dot, gap, digit.
        assert_eq!(width(&rows[0]), 9);
        assert!(rows.iter().all(|r| r.chars().all(|c| " ▀▄█".contains(c))));
        assert!(rows[0].contains('█'));
    }

    #[test]
    fn every_digit_has_a_glyph() {
        for d in '0'..='9' {
            let rows = big(&d.to_string());
            assert!(
                rows.iter()
                    .any(|r| r.contains('█') || r.contains('▀') || r.contains('▄')),
                "{d}"
            );
        }
    }

    #[test]
    fn unknown_chars_are_gaps() {
        let rows = big("?");
        assert_eq!(
            rows,
            ["   ".to_string(), "   ".to_string(), "   ".to_string()]
        );
        assert_eq!(big(""), [String::new(), String::new(), String::new()]);
    }
}
