//! Markdown in the AI panel: the parts replies use (headings, lists, quotes, rules, tables, fenced
//! code with a copy button; bold, italics, `code`, links, strikethrough), drawn with egui labels.
//! Not all of CommonMark. The one font has one weight: bold is drawn twice, a hair apart.

use std::hash::Hash;
use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{self, Color32, FontId, Frame, Galley, Label, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

use crate::theme::Theme;
use crate::{icons, kit};

#[derive(Debug, PartialEq)]
enum Block {
    Heading(usize, String),
    Paragraph(String),
    /// A list item: its depth, its marker ("•" or "3."), its text.
    Item(usize, String, String),
    Quote(String),
    /// Fenced code: its language, its lines.
    Code(String, String),
    Rule,
    Table(Vec<Vec<String>>),
}

/// A list item's marker and text: `- a`, `* a`, `+ a`, `1. a`, `1) a`.
fn item(line: &str) -> Option<(String, &str)> {
    if let Some(rest) = line.strip_prefix(['-', '*', '+']).and_then(|r| r.strip_prefix(' ')) {
        return Some(("•".into(), rest));
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let rest = line[digits..].strip_prefix(['.', ')'])?.strip_prefix(' ')?;
    (digits > 0).then(|| (format!("{}.", &line[..digits]), rest))
}

fn cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = line.strip_suffix('|').unwrap_or(line);
    line.split('|').map(|c| c.trim().to_string()).collect()
}

fn blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Paragraph(para.join(" ")));
            para.clear();
        }
    };
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        let rule = trimmed.len() >= 3 && ['-', '*', '_'].iter().any(|m| trimmed.starts_with(*m) && trimmed.chars().all(|c| c == *m || c == ' '));
        if let Some(lang) = trimmed.strip_prefix("```") {
            flush(&mut para, &mut out);
            let mut code = Vec::new();
            for l in lines.by_ref() {
                if l.trim_start().starts_with("```") {
                    break;
                }
                code.push(l);
            }
            out.push(Block::Code(lang.trim().into(), code.join("\n")));
        } else if trimmed.is_empty() {
            flush(&mut para, &mut out);
        } else if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush(&mut para, &mut out);
            out.push(Block::Heading(hashes, trimmed[hashes..].trim().trim_end_matches('#').trim_end().into()));
        } else if rule {
            flush(&mut para, &mut out);
            out.push(Block::Rule);
        } else if let Some((marker, rest)) = item(trimmed) {
            flush(&mut para, &mut out);
            out.push(Block::Item(indent / 2, marker, rest.trim().into()));
        } else if let Some(rest) = trimmed.strip_prefix('>') {
            flush(&mut para, &mut out);
            match out.last_mut() {
                Some(Block::Quote(q)) => {
                    q.push(' ');
                    q.push_str(rest.trim());
                }
                _ => out.push(Block::Quote(rest.trim().into())),
            }
        } else if trimmed.starts_with('|') && lines.peek().is_some_and(|next| next.contains('-') && cells(next).iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':')))) {
            flush(&mut para, &mut out);
            lines.next();
            let mut rows = vec![cells(trimmed)];
            while let Some(row) = lines.next_if(|l| l.trim_start().starts_with('|')) {
                rows.push(cells(row));
            }
            out.push(Block::Table(rows));
        } else if indent > 0
            && para.is_empty()
            && let Some(Block::Item(_, _, text)) = out.last_mut()
        {
            // an item's text going on under it
            text.push(' ');
            text.push_str(trimmed);
        } else {
            para.push(trimmed);
        }
    }
    flush(&mut para, &mut out);
    out
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Span {
    text: String,
    bold: bool,
    italic: bool,
    code: bool,
    strike: bool,
    link: Option<String>,
}

/// A line's inline parts. A `*` or `_` only counts when it closes later; `_` inside a word
/// (`list_nodes`) is a letter.
fn spans(text: &str) -> Vec<Span> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let find = |from: usize, marker: &[char]| (from..(n + 1).saturating_sub(marker.len())).find(|&j| chars[j..j + marker.len()] == *marker);
    let mut out = Vec::new();
    let mut cur = Span::default();
    let push = |cur: &mut Span, out: &mut Vec<Span>| {
        if !cur.text.is_empty() {
            out.push(cur.clone());
            cur.text.clear();
        }
    };
    let mut i = 0;
    while i < n {
        let c = chars[i];
        match c {
            '\\' if i + 1 < n && chars[i + 1].is_ascii_punctuation() => {
                cur.text.push(chars[i + 1]);
                i += 2;
            }
            '`' if let Some(end) = find(i + 1, &['`']) => {
                push(&mut cur, &mut out);
                out.push(Span { text: chars[i + 1..end].iter().collect(), code: true, ..Default::default() });
                i = end + 1;
            }
            '[' if let Some(mid) = find(i + 1, &[']', '('])
                && let Some(end) = find(mid + 2, &[')']) =>
            {
                push(&mut cur, &mut out);
                out.push(Span { text: chars[i + 1..mid].iter().collect(), link: Some(chars[mid + 2..end].iter().collect()), ..cur.clone() });
                i = end + 1;
            }
            '*' | '_' | '~' => {
                let double = i + 1 < n && chars[i + 1] == c;
                let len = if double { 2 } else { 1 };
                let marker = &chars[i..i + len];
                let before = i.checked_sub(1).map(|j| chars[j]);
                let after = chars.get(i + len).copied();
                let on = match (c, double) {
                    ('~', false) => None,
                    ('~', true) => Some(cur.strike),
                    (_, true) => Some(cur.bold),
                    _ => Some(cur.italic),
                };
                let word = |ch: Option<char>| ch.is_some_and(char::is_alphanumeric);
                let ok = match on {
                    None => false,
                    // opening: text right after it, and a closing one further on
                    Some(false) => after.is_some_and(|a| !a.is_whitespace()) && find(i + len + 1, marker).is_some() && !(c == '_' && word(before)),
                    Some(true) => before.is_some_and(|b| !b.is_whitespace()) && !(c == '_' && word(after)),
                };
                if ok {
                    push(&mut cur, &mut out);
                    match (c, double) {
                        ('~', _) => cur.strike ^= true,
                        (_, true) => cur.bold ^= true,
                        _ => cur.italic ^= true,
                    }
                } else {
                    cur.text.extend(marker);
                }
                i += len;
            }
            _ => {
                cur.text.push(c);
                i += 1;
            }
        }
    }
    push(&mut cur, &mut out);
    out
}

type Links = Vec<(std::ops::Range<usize>, String)>;

/// The text laid out, its bold parts alone (for the second pass) and where its links are
/// (char ranges).
fn lay_out(ui: &Ui, t: &Theme, text: &str, size: f32, color: Color32, all_bold: bool, width: f32) -> (Arc<Galley>, Option<Arc<Galley>>, Links) {
    let line = (size * 1.55).round();
    let (mut job, mut bold) = (LayoutJob::default(), LayoutJob::default());
    let mut links = Vec::new();
    let mut at = 0;
    let mut any_bold = false;
    for span in spans(text) {
        let mut f = TextFormat { font_id: FontId::proportional(size), color, line_height: Some(line), italics: span.italic, ..Default::default() };
        if span.code {
            f.font_id = Theme::mono(size - 1.5);
            f.background = t.bg_control;
            f.expand_bg = 1.0;
        }
        if span.strike {
            f.strikethrough = Stroke::new(1.0, color);
        }
        let n = span.text.chars().count();
        if let Some(url) = &span.link {
            f.color = t.accent;
            f.underline = Stroke::new(1.0, t.accent.gamma_multiply(0.5));
            links.push((at..at + n, url.clone()));
        }
        at += n;
        let strong = all_bold || span.bold;
        any_bold |= strong;
        let mut b = f.clone();
        b.background = Theme::CLEAR;
        b.underline = Stroke::NONE;
        b.strikethrough = Stroke::NONE;
        if !strong {
            b.color = Theme::CLEAR;
        }
        job.append(&span.text, 0.0, f);
        bold.append(&span.text, 0.0, b);
    }
    job.wrap.max_width = width;
    bold.wrap.max_width = width;
    let painter = ui.painter();
    (painter.layout_job(job), any_bold.then(|| painter.layout_job(bold)), links)
}

/// One run of text: selectable, its bold drawn twice, its links opening in the browser.
fn text(ui: &mut Ui, t: &Theme, text: &str, size: f32, color: Color32, all_bold: bool) {
    let (galley, bold, links) = lay_out(ui, t, text, size, color, all_bold, ui.available_width());
    let r = ui.add(Label::new(galley.clone()).selectable(true));
    if let Some(bold) = bold {
        ui.painter().galley(r.rect.min + vec2(0.5, 0.0), bold, color);
    }
    let link = r.hover_pos().and_then(|p| {
        let at = galley.cursor_from_pos(p - r.rect.min).index.0;
        links.iter().find(|(range, _)| range.contains(&at)).map(|(_, url)| url.clone())
    });
    if let Some(url) = link {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if r.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    }
}

/// Fenced code: its language and a copy button over it, the lines scrolling sideways. True
/// when it was copied.
fn code(ui: &mut Ui, t: &Theme, lang: &str, code: &str, id: egui::Id) -> bool {
    let mut copied = false;
    Frame::new().fill(t.bg_field).stroke(Stroke::new(1.0, t.line)).corner_radius(t.radius_md).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        let head = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover()).0;
        let name = if lang.is_empty() { "code" } else { lang };
        ui.painter().text(pos2(head.left() + 10.0, head.center().y), egui::Align2::LEFT_CENTER, name, Theme::mono(Theme::TEXT_XS), t.fg_muted);
        let button = Rect::from_center_size(pos2(head.right() - 15.0, head.center().y), vec2(22.0, 22.0));
        let r = ui.interact(button, id.with("copy"), Sense::click()).on_hover_text(tr!("Copy"));
        let k = kit::hover(ui, t, &r);
        if k > 0.0 {
            ui.painter().rect_filled(button, t.radius_sm, t.bg_control.gamma_multiply(k));
        }
        kit::icon_at(ui.painter(), button.center(), icons::COPY, 14.0, Theme::mix(t.fg_muted, t.fg_text, k));
        copied = r.clicked();
        ui.painter().hline(head.x_range(), head.bottom(), Stroke::new(1.0, t.line));
        egui::ScrollArea::horizontal().id_salt(id.with("lines")).show(ui, |ui| {
            Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                let lines = RichText::new(code).font(Theme::mono(Theme::TEXT_SM)).line_height(Some((Theme::TEXT_SM * 1.6).round())).color(t.fg_text);
                ui.add(Label::new(lines).extend().selectable(true));
            });
        });
    });
    if copied {
        ui.ctx().copy_text(code.to_string());
    }
    copied
}

/// Draws `markdown` at the width there is. True when a code block was copied (for a toast).
pub fn show(ui: &mut Ui, t: &Theme, markdown: &str, id: impl Hash + std::fmt::Debug) -> bool {
    let id = ui.scope_id().with(id);
    let size = Theme::TEXT_MD + 1.0;
    let mut copied = false;
    let blocks = blocks(markdown);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut last: Option<&Block> = None;
        for (n, block) in blocks.iter().enumerate() {
            // room between blocks: less between a list's items
            ui.add_space(match (last, block) {
                (None, _) => 0.0,
                (Some(Block::Item(..)), Block::Item(..)) => 3.0,
                (_, Block::Heading(..)) => 14.0,
                _ => 9.0,
            });
            match block {
                Block::Heading(level, line) => {
                    let s = match level {
                        1 => Theme::TEXT_LG + 3.0,
                        2 => Theme::TEXT_LG + 1.0,
                        _ => size,
                    };
                    text(ui, t, line, s, t.fg_text, true);
                }
                Block::Paragraph(line) => text(ui, t, line, size, t.fg_text, false),
                Block::Item(depth, marker, line) => {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.add_space(*depth as f32 * 16.0 + 2.0);
                        let w = if marker == "•" { 16.0 } else { 8.0 + 7.0 * marker.len() as f32 };
                        let (rect, _) = ui.allocate_exact_size(vec2(w, (size * 1.55).round()), Sense::hover());
                        ui.painter().text(pos2(rect.left(), rect.center().y), egui::Align2::LEFT_CENTER, marker, FontId::proportional(size), t.fg_muted);
                        ui.vertical(|ui| text(ui, t, line, size, t.fg_text, false));
                    });
                }
                Block::Quote(line) => {
                    let r = ui.horizontal_top(|ui| {
                        ui.add_space(12.0);
                        ui.vertical(|ui| text(ui, t, line, size, t.fg_muted, false));
                    });
                    let rect = r.response.rect;
                    ui.painter().vline(rect.left() + 2.0, rect.y_range(), Stroke::new(2.0, t.fg_muted.gamma_multiply(0.4)));
                }
                Block::Code(lang, lines) => copied |= code(ui, t, lang, lines, id.with(n)),
                Block::Rule => {
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
                    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, t.line));
                }
                Block::Table(rows) => {
                    egui::ScrollArea::horizontal().id_salt(id.with(n)).show(ui, |ui| {
                        egui::Grid::new(id.with(("table", n))).striped(true).spacing(vec2(14.0, 6.0)).show(ui, |ui| {
                            for (r, row) in rows.iter().enumerate() {
                                for cell in row {
                                    let (galley, bold, _) = lay_out(ui, t, cell, size - 1.0, t.fg_text, r == 0, f32::INFINITY);
                                    let resp = ui.add(Label::new(galley).selectable(true));
                                    if let Some(bold) = bold {
                                        ui.painter().galley(resp.rect.min + vec2(0.5, 0.0), bold, t.fg_text);
                                    }
                                }
                                ui.end_row();
                            }
                        });
                    });
                }
            }
            last = Some(block);
        }
    });
    copied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_found() {
        let text = "# Tunnel\n\nA **purple** tunnel\nthat pulses.\n\n- kick moves `zoom`\n  and the hue\n- bass\n  1. nested\n\n> a quote\n> goes on\n\n```wgsl\nfn main() {}\n```\n---\n| a | b |\n|---|:-:|\n| 1 | 2 |";
        assert_eq!(
            blocks(text),
            [
                Block::Heading(1, "Tunnel".into()),
                Block::Paragraph("A **purple** tunnel that pulses.".into()),
                Block::Item(0, "•".into(), "kick moves `zoom` and the hue".into()),
                Block::Item(0, "•".into(), "bass".into()),
                Block::Item(1, "1.".into(), "nested".into()),
                Block::Quote("a quote goes on".into()),
                Block::Code("wgsl".into(), "fn main() {}".into()),
                Block::Rule,
                Block::Table(vec![vec!["a".into(), "b".into()], vec!["1".into(), "2".into()]]),
            ]
        );
        assert_eq!(blocks("```\nunclosed"), [Block::Code(String::new(), "unclosed".into())], "a reply still typing in");
    }

    #[test]
    fn inline_marks_are_read() {
        let s = |text: &str| spans(text).into_iter().map(|s| (s.text, s.bold, s.italic, s.code, s.link)).collect::<Vec<_>>();
        let plain = |text: &str| (text.to_string(), false, false, false, None);
        assert_eq!(s("a **b** *c*"), [plain("a "), ("b".into(), true, false, false, None), plain(" "), ("c".into(), false, true, false, None)]);
        assert_eq!(s("call `list_nodes` then list_nodes"), [plain("call "), ("list_nodes".into(), false, false, true, None), plain(" then list_nodes")]);
        assert_eq!(s("5 * 3 and a_b"), [plain("5 * 3 and a_b")], "a lone star, a word's underscore");
        assert_eq!(s("[docs](https://x.y)"), [("docs".into(), false, false, false, Some("https://x.y".into()))]);
        assert_eq!(s(r"\*not\*"), [plain("*not*")]);
    }
}
