// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! A small Markdown subset for text Mica ships or downloads (release notes in What's new and
//! the update notification): `#`/`##`/`###` headings, `-`/`*` bullets (indented lines continue
//! a bullet), paragraphs, `**bold**`, `` `code` `` and `[links](https://…)`. Anything else is
//! shown as plain text, never dropped.

use crate::theme;
use eframe::egui::{self, RichText};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// Level 1–3 and the heading's text.
    Heading(u8, String),
    Bullet(String),
    Paragraph(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Span {
    Text(String),
    Bold(String),
    Code(String),
    /// The link text and its target.
    Link(String, String),
}

/// The blocks of `text`, in order.
pub fn blocks(text: &str) -> Vec<Block> {
    let mut out: Vec<Block> = vec![];
    // The block that following lines continue (a paragraph or a bullet), if any.
    let mut open = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            open = false;
            continue;
        }
        if let Some((level, rest)) = heading(trimmed) {
            out.push(Block::Heading(level, rest.to_string()));
            open = false;
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            out.push(Block::Bullet(rest.trim().to_string()));
            open = true;
            continue;
        }
        match out.last_mut() {
            Some(Block::Paragraph(p) | Block::Bullet(p)) if open => {
                p.push(' ');
                p.push_str(trimmed);
            }
            _ => {
                out.push(Block::Paragraph(trimmed.to_string()));
                open = true;
            }
        }
    }
    out
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if !(1..=3).contains(&hashes) {
        return None;
    }
    let rest = line[hashes..].strip_prefix(' ')?;
    Some((hashes as u8, rest.trim()))
}

/// The inline spans of one block's text.
pub fn spans(text: &str) -> Vec<Span> {
    let mut out = vec![];
    let mut plain = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let found = if let Some(after) = rest.strip_prefix("**") {
            after
                .find("**")
                .filter(|&end| end > 0)
                .map(|end| (Span::Bold(after[..end].to_string()), &after[end + 2..]))
        } else if let Some(after) = rest.strip_prefix('`') {
            after
                .find('`')
                .filter(|&end| end > 0)
                .map(|end| (Span::Code(after[..end].to_string()), &after[end + 1..]))
        } else if let Some(after) = rest.strip_prefix('[') {
            after.find("](").and_then(|mid| {
                let target = &after[mid + 2..];
                let end = target.find(')')?;
                let url = &target[..end];
                (mid > 0 && is_url(url)).then(|| {
                    (
                        Span::Link(after[..mid].to_string(), url.to_string()),
                        &target[end + 1..],
                    )
                })
            })
        } else {
            None
        };
        match found {
            Some((span, next)) => {
                if !plain.is_empty() {
                    out.push(Span::Text(std::mem::take(&mut plain)));
                }
                out.push(span);
                rest = next;
            }
            None => {
                let c = rest.chars().next().expect("not empty");
                plain.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    if !plain.is_empty() {
        out.push(Span::Text(plain));
    }
    out
}

/// Only web links open from shipped or downloaded text.
fn is_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && !url.contains(char::is_whitespace)
}

/// Draws `text` at `size` px (headings larger). A clicked link opens in the browser.
pub fn show(ui: &mut egui::Ui, text: &str, size: f32) {
    let p = theme::of(ui);
    let mut first = true;
    for block in blocks(text) {
        match block {
            Block::Heading(level, t) => {
                ui.add_space(if first { 0. } else { 10. });
                let grow = match level {
                    1 => 5.,
                    2 => 3.,
                    _ => 1.,
                };
                ui.label(
                    RichText::new(t)
                        .font(theme::semibold(size + grow))
                        .color(p.text),
                );
                ui.add_space(2.);
            }
            Block::Paragraph(t) => {
                ui.add_space(if first { 0. } else { 6. });
                inline(ui, &t, size);
            }
            Block::Bullet(t) => {
                ui.add_space(3.);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.;
                    // A painted dot, centered on the first line.
                    let line = ui
                        .painter()
                        .layout_no_wrap("x".into(), theme::sans(size), p.muted)
                        .size()
                        .y
                        .max(ui.spacing().interact_size.y);
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(16., line), egui::Sense::hover());
                    ui.painter().circle_filled(
                        egui::pos2(r.left() + 5., r.center().y),
                        2.,
                        p.muted,
                    );
                    ui.vertical(|ui| inline(ui, &t, size));
                });
            }
        }
        first = false;
    }
}

fn inline(ui: &mut egui::Ui, text: &str, size: f32) {
    let p = theme::of(ui);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.;
        for span in spans(text) {
            match span {
                Span::Text(t) => {
                    ui.label(RichText::new(t).font(theme::sans(size)).color(p.label));
                }
                Span::Bold(t) => {
                    ui.label(RichText::new(t).font(theme::semibold(size)).color(p.text));
                }
                Span::Code(t) => {
                    ui.label(RichText::new(t).font(theme::mono(size - 1.)).color(p.text));
                }
                Span::Link(t, url) => {
                    let r = ui
                        .add(
                            egui::Label::new(
                                RichText::new(t)
                                    .font(theme::sans(size))
                                    .color(p.accent_bright),
                            )
                            .sense(egui::Sense::click())
                            .selectable(false),
                        )
                        .on_hover_text(&url);
                    if r.hovered() {
                        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                    }
                    if r.clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_and_spans() {
        let text = "# Title\n\nFirst line\ncontinued.\n\n### Fixes\n- One **bold** thing\n  \
                    wrapped on.\n* Two with [a link](https://example.com) and `code`\n\nEnd.";
        assert_eq!(
            blocks(text),
            [
                Block::Heading(1, "Title".into()),
                Block::Paragraph("First line continued.".into()),
                Block::Heading(3, "Fixes".into()),
                Block::Bullet("One **bold** thing wrapped on.".into()),
                Block::Bullet("Two with [a link](https://example.com) and `code`".into()),
                Block::Paragraph("End.".into()),
            ]
        );
        assert_eq!(
            spans("Two with [a link](https://example.com) and **bold** `x`"),
            [
                Span::Text("Two with ".into()),
                Span::Link("a link".into(), "https://example.com".into()),
                Span::Text(" and ".into()),
                Span::Bold("bold".into()),
                Span::Text(" ".into()),
                Span::Code("x".into()),
            ]
        );
    }

    #[test]
    fn odd_markup_stays_text() {
        // Unclosed bold, a non-web link, a lone bracket and #### are shown as typed.
        assert_eq!(spans("a **b"), [Span::Text("a **b".into())]);
        assert_eq!(
            spans("[x](file:///etc/passwd)"),
            [Span::Text("[x](file:///etc/passwd)".into())]
        );
        assert_eq!(spans("[x] y"), [Span::Text("[x] y".into())]);
        assert_eq!(
            blocks("#### deep\n#nospace"),
            [Block::Paragraph("#### deep #nospace".into())]
        );
        assert_eq!(spans("ünï **cödé**").len(), 2);
    }
}
