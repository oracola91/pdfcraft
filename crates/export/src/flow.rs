//! Text that runs on from one line to the next.
//!
//! The reading blocks keep style boundaries (#4158), so a sentence with a bold name or an italic
//! word in it reaches the exporter as several blocks, and `layout::paragraphs` joins only the ones
//! that run on along one line. A sentence that wraps onto the next line after
//! such a run came out as one paragraph per line. [`join_wrapped`] joins a paragraph whose last
//! line reaches the right edge of the page's text with the one that starts just below it at the
//! same left edge, in the same size: the text wrapped there. Headings, list items, a line that
//! ends a sentence and a line with paragraph space after it stay apart.

use super::layout::Run;

/// Bullets that mark a list item wherever they stand, including the Symbol and Wingdings bullets
/// Word writes into its PDFs (U+F0B7, U+F0A7).
const BULLETS: [char; 11] = ['•', '◦', '▪', '‣', '⁃', '●', '○', '■', '□', '\u{F0B7}', '\u{F0A7}'];

/// Dashes and a middle dot: list markers at the start of a line, punctuation inside one.
const DASHES: [char; 4] = ['-', '\u{2013}', '\u{2014}', '·'];

/// A run taller than this many times its font size is more than one line: a block of several
/// rows ends where its widest row does, not where its last one does.
const ONE_LINE: f64 = 1.6;

/// How far (as a multiple of the font size) the space between two lines may be from none, and
/// they still follow each other within a paragraph: more is paragraph spacing.
const LINE_GAP: f64 = 0.5;

/// How close to the right edge of the page's text a full line ends: within this many times the
/// font size, or this fraction of the text's width, whichever is more (a ragged line ends short
/// of the edge by up to a long word).
const FULL_LINE: (f64, f64) = (3.0, 0.12);

/// How far (points) the first line of a paragraph may start from the left edge of the line above
/// it and still be at the same left edge.
const SAME_LEFT: f64 = 2.0;

/// Whether two boxes ([x0, y0, x1, y1]) share a line: they overlap vertically by more than half
/// the shorter one, as `layout::continued_from` has it.
fn on_one_line(a: [f64; 4], b: [f64; 4]) -> bool {
    a[3].min(b[3]) - a[1].max(b[1]) > 0.5 * (a[3] - a[1]).min(b[3] - b[1])
}

/// Whether `text` opens like a list item: a bullet, a dash or `*`; one to three digits followed
/// by `.` or `)`; or one letter followed by `)` ("a)", "а)"); then a space or nothing. "1.5 kg",
/// "i.e." and "2026. Year" are text.
pub(crate) fn list_item(text: &str) -> bool {
    let t = text.trim_start();
    let Some(first) = t.chars().next() else { return false };
    if BULLETS.contains(&first) || DASHES.contains(&first) || first == '*' {
        return true;
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    // ASCII digits are one byte each and `first` is one whole character, so both cuts fall on
    // character boundaries.
    let (rest, marks): (&str, &[char]) = if (1..=3).contains(&digits) {
        (t.get(digits..).unwrap_or_default(), &['.', ')'])
    } else if digits == 0 && first.is_alphabetic() {
        (t.get(first.len_utf8()..).unwrap_or_default(), &[')'])
    } else {
        return false;
    };
    let mut after = rest.chars();
    after.next().is_some_and(|c| marks.contains(&c)) && after.next().is_none_or(char::is_whitespace)
}

/// Whether a line's text ends a sentence (closing quotes and brackets aside). Text set without
/// paragraph spacing gives no other sign that a paragraph ends there, so such a line is left
/// apart from the next rather than joined to it wrongly.
fn ends_sentence(text: &str) -> bool {
    text.trim_end().trim_end_matches(['"', '\'', '”', '’', '“', '»', ')', ']']).ends_with(['.', '!', '?', ':', ';', '…'])
}

/// `paras` (a page's paragraphs, in any order) top to bottom, with each paragraph that wraps onto
/// the next line joined to it. `level` is a paragraph's heading level (0 for body text).
pub(crate) fn join_wrapped<'a>(mut paras: Vec<Vec<Run<'a>>>, level: impl Fn(&[Run<'a>]) -> u8) -> Vec<Vec<Run<'a>>> {
    // The left and right edges of the page's text: where a full line starts and ends.
    let (left, right) = paras
        .iter()
        .flatten()
        .map(|r| r.block.rect)
        .filter(|r| r[0].is_finite() && r[2].is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, r), x| (l.min(x[0]), r.max(x[2])));
    if right <= left {
        return paras;
    }
    // Top to bottom, then left to right, by each paragraph's first run.
    paras.sort_by(|a, b| {
        let top_left = |p: &[Run]| p.first().map_or([0.0; 2], |r| [r.block.rect[3], r.block.rect[0]]);
        let (a, b) = (top_left(a), top_left(b));
        b[0].total_cmp(&a[0]).then(a[1].total_cmp(&b[1]))
    });
    // Each paragraph with its heading level. A joined paragraph is body text: only body text joins.
    let mut out: Vec<(Vec<Run<'a>>, u8)> = Vec::with_capacity(paras.len());
    for para in paras {
        let lvl = level(&para);
        match out.last_mut() {
            Some((prev, 0)) if lvl == 0 && wraps_into(prev, &para, [left, right]) => {
                // The line break was a word space, unless the line ends in a hyphen; the hyphen
                // stays, as it may belong to the word ("well-known").
                if let Some(end) = prev.last_mut()
                    && !end.text.ends_with(char::is_whitespace)
                    && !end.text.ends_with('-')
                {
                    end.text.push(' ');
                }
                prev.extend(para);
            }
            _ => out.push((para, lvl)),
        }
    }
    out.into_iter().map(|(para, _)| para).collect()
}

/// Whether the body text paragraph `prev` ends in a full line whose text runs on in `next`, the
/// body text paragraph just below it. `[left, right]` are the edges of the page's text.
fn wraps_into(prev: &[Run], next: &[Run], [left, right]: [f64; 2]) -> bool {
    let (Some(last), Some(first)) = (prev.last(), next.first()) else { return false };
    let (a, b) = (last.block, first.block);
    let size = a.size;
    let alike = size.is_finite() && size > 0.0 && (a.size - b.size).abs() <= 0.5 && a.rotation.abs() <= 0.1 && b.rotation.abs() <= 0.1;
    if !alike || a.rect[3] - a.rect[1] > ONE_LINE * size || list_item(&first.text) || ends_sentence(&last.text) {
        return false;
    }
    // The last line: the runs at the end of `prev` that share the last run's line.
    let (line_left, line_right) = prev
        .iter()
        .rev()
        .take_while(|r| on_one_line(r.block.rect, a.rect))
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, r), x| (l.min(x.block.rect[0]), r.max(x.block.rect[2])));
    let full = line_right >= right - (FULL_LINE.0 * size).max(FULL_LINE.1 * (right - left));
    let same_left = (b.rect[0] - line_left).abs() <= SAME_LEFT;
    // Just below: no paragraph space between the two lines.
    let gap = a.rect[1] - b.rect[3];
    full && same_left && (-LINE_GAP * size..=LINE_GAP * size).contains(&gap)
}
