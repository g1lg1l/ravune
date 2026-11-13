//! The start page: what Ravune opens on. Unsaved work from last time, the songs you had
//! open (find, open, show in their folder, take off the list), the templates, a lesson.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Layout, Rect, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use ravune_core::{PATTERN_COLORS, Project};

use crate::files::{self, Pending};
use crate::kit::{self, Variant};
use crate::lessons::LESSONS;
use crate::theme::Theme;
use crate::{App, icons};

/// The page while it's open: the recent songs as found on disk when it opened.
#[derive(Default)]
pub struct Start {
    songs: Vec<Song>,
    query: String,
    /// Your templates: where, called what, at what tempo.
    mine: Vec<(PathBuf, String, f32)>,
}

struct Song {
    path: PathBuf,
    name: String,
    folder: String,
    /// When it was last saved (seconds since 1970); `None` if it's gone.
    modified: Option<u64>,
    versions: usize,
}

impl Start {
    fn new(recent: &[String]) -> Self {
        let songs = recent
            .iter()
            .map(|r| {
                let path = PathBuf::from(r);
                let name = path.file_stem().map_or(r.clone(), |s| s.to_string_lossy().into_owned());
                let folder = path.parent().map_or(String::new(), |d| d.to_string_lossy().into_owned());
                // the home folder as ~, so the part that tells songs apart shows
                let folder = match std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
                    Ok(home) if !home.is_empty() && folder.starts_with(&home) => format!("~{}", &folder[home.len()..]),
                    _ => folder,
                };
                let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs());
                let versions = files::version_count(&path);
                Song { path, name, folder, modified, versions }
            })
            .collect();
        let mine = files::user_templates()
            .into_iter()
            .map(|p| {
                let name = p.file_stem().map_or(String::new(), |s| s.to_string_lossy().into_owned());
                let bpm = files::read(&p).map_or(0.0, |song| song.tempo);
                (p, name, bpm)
            })
            .collect();
        Start { songs, query: String::new(), mine }
    }

    /// A template deleted while the page shows it.
    pub fn forget_template(&mut self, path: &Path) {
        self.mine.retain(|(p, ..)| p != path);
    }
}

/// A card in the template grid: what it opens.
enum Pick {
    Bundled(&'static str),
    Mine(PathBuf),
}

/// The page's size: big, within the window.
fn page_size(ctx: &egui::Context) -> egui::Vec2 {
    let room = ctx.content_rect().size() - vec2(48.0, 48.0);
    vec2(room.x.min(920.0), room.y.min(620.0))
}

impl App {
    /// Opens the start page (File → Start Page, and at launch).
    pub fn show_start(&mut self) {
        self.start = Some(Start::new(&self.settings.recent));
    }

    pub fn start_page(&mut self, ctx: &egui::Context) {
        let Some(mut page) = self.start.take() else { return };
        let t = self.theme.clone();
        let size = page_size(ctx);
        let frame = Frame::new().fill(t.bg_base).stroke(Stroke::new(1.0, t.line)).corner_radius(t.radius_md + 4).shadow(ctx.global_style().visuals.window_shadow);
        let mut close = false;
        let modal = egui::Modal::new(Id::unique("start")).frame(frame).show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let side = Rect::from_min_size(rect.min, vec2(250.0, rect.height()));
            let main = Rect::from_min_max(pos2(side.right(), rect.top()), rect.max);
            // the sidebar's own shade, rounded on the outer corners only
            let r = t.radius_md + 4;
            ui.painter().rect_filled(side, CornerRadius { nw: r, sw: r, ne: 0, se: 0 }, t.bg_panel);
            ui.painter().vline(side.right(), side.y_range(), Stroke::new(1.0, t.line));
            let mut left = ui.new_child(UiBuilder::new().max_rect(side.shrink(t.pad * 2.0)).layout(Layout::top_down(Align::Min)));
            close |= self.start_sidebar(&mut left, &t);
            // the content starts under the close button
            let content = Rect::from_min_max(main.min + vec2(t.pad * 2.5, 44.0), main.max - vec2(t.pad * 2.5, t.pad * 2.0));
            let mut right = ui.new_child(UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Min)));
            close |= self.start_main(&mut right, &t, &mut page);
            // close, top right
            let x = Rect::from_min_size(pos2(rect.right() - 36.0, rect.top() + 10.0), vec2(26.0, 26.0));
            let mut corner = ui.new_child(UiBuilder::new().max_rect(x));
            if kit::icon_button(&mut corner, &t, icons::X, tr!("Close (Esc): carry on with this song"), false).clicked() {
                close = true;
            }
        });
        // Esc or a click outside: carry on with what's open (unsaved work stays offered)
        if !(close || modal.should_close()) {
            self.start = Some(page);
        }
    }

    /// New, open, learn; "show at startup" at the bottom. True when it picked something.
    fn start_sidebar(&mut self, ui: &mut Ui, t: &Theme) -> bool {
        let mut done = false;
        ui.spacing_mut().item_spacing = vec2(t.gap, t.gap);
        ui.horizontal(|ui| {
            let (mark, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
            ui.painter().rect_filled(mark, CornerRadius::same(t.radius_md), t.accent);
            kit::icon_at(ui.painter(), mark.center(), icons::WAVEFORM, 18.0, t.bg_base);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new("RAVUNE").font(FontId::proportional(Theme::TEXT_LG + 1.0)).extra_letter_spacing(2.0).strong().color(t.fg_text));
                ui.label(RichText::new(tr!("Version {}", env!("CARGO_PKG_VERSION"))).small().color(t.fg_muted));
            });
        });
        ui.add_space(t.pad * 2.0);
        let w = Some(ui.available_width());
        if kit::action_sized(ui, t, Variant::Primary, Some(icons::FILE_PLUS), tr!("New song"), w).clicked() {
            self.request(Pending::New);
            done = true;
        }
        let open = kit::action_sized(ui, t, Variant::Secondary, Some(icons::FOLDER_OPEN), tr!("Open…"), w);
        if open.on_hover_text(tr!("A Ravune song, a DAWproject from another DAW, or a MIDI file")).clicked() && self.open_dialog() {
            done = true;
        }
        ui.add_space(t.pad * 2.0);
        kit::caps(ui, t, tr!("LEARN"));
        for (k, lesson) in LESSONS.iter().enumerate() {
            let finished = self.settings.lessons_done.iter().any(|d| d == lesson.title);
            let icon = if finished { icons::CHECK } else { icons::GRADUATION_CAP };
            if side_item(ui, t, icon, crate::i18n::t(lesson.title)).on_hover_text(crate::i18n::t(lesson.about)).clicked() {
                self.start_lesson(k);
                done = true;
            }
        }
        if side_item(ui, t, icons::BOOK_OPEN, tr!("The manual")).clicked() {
            self.show_manual = true;
            done = true;
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.horizontal(|ui| {
                kit::switch(ui, t, &mut self.settings.show_start, tr!("Show this page when Ravune starts"));
                ui.label(RichText::new(tr!("Show at startup")).small().color(t.fg_muted));
            });
        });
        done
    }

    /// Unsaved work, the recent songs, the templates. True when it picked something.
    fn start_main(&mut self, ui: &mut Ui, t: &Theme, page: &mut Start) -> bool {
        let mut done = false;
        ui.spacing_mut().item_spacing = vec2(t.gap, t.gap);
        if let Some(project) = self.recovery.take() {
            match recovery_card(ui, t, &project) {
                Some(true) => {
                    self.load_project(project, None, Project::default());
                    self.say_as(crate::shell::Tone::Done, tr!("Your unsaved work is back: save it to keep it"));
                    done = true;
                }
                Some(false) => {}
                None => self.recovery = Some(project),
            }
            ui.add_space(t.pad);
        }

        // the recent songs: a search once there are a few
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr!("Recent songs")).font(FontId::proportional(Theme::TEXT_LG)).strong().color(t.fg_text));
            if page.songs.len() > 4 {
                kit::right(ui, |ui| ui.add(kit::field(&mut page.query).hint_text(tr!("Find a song")).desired_width(200.0)));
            }
        });
        let query = page.query.to_lowercase();
        let shown: Vec<usize> = (0..page.songs.len()).filter(|&k| query.is_empty() || page.songs[k].name.to_lowercase().contains(&query)).collect();
        let cols = 4;
        let rows = (files::TEMPLATES.len() + page.mine.len()).div_ceil(cols);
        // two rows show; more scroll
        let grid_h = rows.min(2) as f32 * (64.0 + t.gap);
        let templates_h = grid_h + t.gap + 30.0;
        let list_h = (ui.available_height() - templates_h - t.pad * 2.0).max(96.0);
        let mut forget = None;
        Frame::new().fill(t.bg_panel).stroke(Stroke::new(1.0, t.line)).corner_radius(t.radius_md).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if shown.is_empty() {
                    let text = if page.songs.is_empty() { tr!("No songs yet. Start a new one, or from a template below.") } else { tr!("No song by that name") };
                    ui.add_space(t.pad * 2.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new(text).color(t.fg_muted)));
                }
                for &k in &shown {
                    match song_row(ui, t, &page.songs[k]) {
                        Some(RowAction::Open) => {
                            self.request(Pending::Open(page.songs[k].path.clone()));
                            done = true;
                        }
                        Some(RowAction::Reveal) => {
                            let folder = page.songs[k].path.parent().map(Path::to_path_buf).unwrap_or_default();
                            ui.ctx().open_url(egui::OpenUrl::new_tab(format!("file:///{}", folder.display())));
                        }
                        Some(RowAction::Forget) => forget = Some(k),
                        None => {}
                    }
                }
            });
        });
        if let Some(k) = forget {
            let song = page.songs.remove(k);
            self.settings.recent.retain(|r| Path::new(r) != song.path);
        }

        ui.add_space(t.pad);
        ui.label(RichText::new(tr!("Start from a template")).font(FontId::proportional(Theme::TEXT_LG)).strong().color(t.fg_text));
        let w = (ui.available_width() - t.gap * (cols as f32 - 1.0) - 8.0) / cols as f32;
        // yours first: they're the ones you made for this
        let mut cards: Vec<(String, String, Pick)> = page.mine.iter().map(|(p, name, bpm)| (name.clone(), tr!("Yours · {} BPM", format!("{bpm:.0}")), Pick::Mine(p.clone()))).collect();
        cards.extend(files::TEMPLATES.iter().map(|(name, text)| {
            let (genre, bpm) = name.split_once(" · ").unwrap_or((name, ""));
            (genre.to_string(), bpm.to_string(), Pick::Bundled(text))
        }));
        let mut chosen = None;
        let mine = page.mine.len();
        egui::ScrollArea::vertical().id_salt("start-templates").max_height(grid_h).auto_shrink([false, true]).show(ui, |ui| {
            for (row, chunk) in cards.chunks(cols).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = t.gap;
                    for (c, (name, detail, pick)) in chunk.iter().enumerate() {
                        // a genre keeps its color however many of yours come before it
                        let k = (row * cols + c).saturating_sub(mine);
                        match template_card(ui, t, name, detail, k, w, matches!(pick, Pick::Mine(_))) {
                            Card::Open => chosen = Some(pick),
                            Card::Delete => {
                                if let Pick::Mine(path) = pick {
                                    self.doomed_template = Some(path.clone());
                                }
                            }
                            Card::None => {}
                        }
                    }
                });
            }
        });
        match chosen {
            Some(Pick::Bundled(text)) => {
                self.request(Pending::Bundled(text));
                done = true;
            }
            Some(Pick::Mine(path)) => {
                self.request(Pending::Template(path.clone()));
                done = true;
            }
            None => {}
        }
        done
    }
}

/// A line of the sidebar: an icon and its words from the left, a soft fill when hovered.
fn side_item(ui: &mut Ui, t: &Theme, icon: &str, text: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), Theme::CONTROL_H + 4.0), Sense::click());
    let h = kit::hover(ui, t, &resp);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(t.radius_sm), Theme::mix(t.bg_panel, t.fg_text, 0.06 * h));
    kit::icon_at(painter, pos2(rect.left() + 12.0, rect.center().y), icon, Theme::TEXT_MD + 2.0, Theme::mix(t.fg_muted, t.accent, h));
    painter.text(pos2(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER, text, Theme::body(), t.fg_text);
    kit::focus_ring(ui, t, &resp, t.radius_sm);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text));
    resp
}

/// "Unsaved work from last time": restore (true) or discard (false).
fn recovery_card(ui: &mut Ui, t: &Theme, project: &Project) -> Option<bool> {
    let mut answer = None;
    Frame::new()
        .fill(Theme::mix(t.bg_panel, t.accent, 0.08))
        .stroke(Stroke::new(1.0, t.accent.gamma_multiply(0.6)))
        .corner_radius(t.radius_md)
        .inner_margin(t.pad * 1.5)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (disc, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                ui.painter().circle_filled(disc.center(), 16.0, t.accent.gamma_multiply(0.18));
                kit::icon_at(ui.painter(), disc.center(), icons::CLOCK_COUNTER_CLOCKWISE, 16.0, t.accent);
                ui.add_space(t.gap);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(tr!("Unsaved work from last time")).strong().color(t.fg_text));
                    let what = tr!("{} channels · {} patterns · {} BPM", project.channels.len(), project.patterns.len(), format!("{:.0}", project.tempo));
                    ui.label(RichText::new(tr!("Ravune closed before it was saved: {}", what)).small().color(t.fg_muted));
                });
                kit::right(ui, |ui| {
                    if kit::action(ui, t, Variant::Primary, Some(icons::ARROW_COUNTER_CLOCKWISE), tr!("Restore")).clicked() {
                        answer = Some(true);
                    }
                    if kit::action(ui, t, Variant::Ghost, None, tr!("Discard")).clicked() {
                        answer = Some(false);
                    }
                });
            });
        });
    answer
}

enum RowAction {
    Open,
    Reveal,
    Forget,
}

/// A recent song: its name and folder, when it was saved and how many versions it keeps;
/// hovered, buttons to show it in its folder and take it off the list. A click opens it.
fn song_row(ui: &mut Ui, t: &Theme, song: &Song) -> Option<RowAction> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::hover());
    let resp = ui.interact(rect, Id::unique(("start-song", &song.path)), Sense::click());
    let gone = song.modified.is_none();
    let hot = ui.rect_contains_pointer(rect);
    let painter = ui.painter_at(rect);
    if hot {
        painter.rect_filled(rect, CornerRadius::ZERO, Theme::mix(t.bg_panel, t.fg_text, 0.05));
    }
    painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, t.line.gamma_multiply(0.6)));
    let ink = if gone { t.fg_muted } else { t.fg_text };
    let icon_at = pos2(rect.left() + t.pad * 2.0, rect.center().y);
    kit::icon_at(&painter, icon_at, if gone { icons::WARNING } else { icons::MUSIC_NOTES }, 18.0, if gone { t.fg_muted } else { t.accent });
    let x = icon_at.x + 22.0;
    painter.text(pos2(x, rect.center().y - 1.0), Align2::LEFT_BOTTOM, &song.name, Theme::body(), ink);
    let folder = if gone { tr!("Not found: moved or deleted").to_string() } else { song.folder.clone() };
    kit::fit_text(&painter, pos2(x, rect.center().y + 2.0), Align2::LEFT_TOP, folder, FontId::proportional(Theme::TEXT_SM), t.fg_muted, rect.width() * 0.6);
    // on the right: when, and the versions; hovered, the buttons take their place
    let mut action = None;
    if hot {
        let buttons = Rect::from_min_max(pos2(rect.right() - 90.0, rect.top()), pos2(rect.right() - t.pad, rect.bottom()));
        let mut child = ui.new_child(UiBuilder::new().max_rect(buttons).layout(Layout::right_to_left(Align::Center)));
        if kit::icon_button(&mut child, t, icons::X, tr!("Take it off the list (the file stays)"), false).clicked() {
            action = Some(RowAction::Forget);
        }
        if !gone && kit::icon_button(&mut child, t, icons::FOLDER_OPEN, tr!("Show in its folder"), false).clicked() {
            action = Some(RowAction::Reveal);
        }
    } else if let Some(at) = song.modified {
        let versions = match song.versions {
            0 => String::new(),
            1 => tr!(" · 1 version").to_string(),
            n => tr!(" · {} versions", n),
        };
        painter.text(pos2(rect.right() - t.pad * 2.0, rect.center().y), Align2::RIGHT_CENTER, format!("{}{versions}", files::ago(at)), FontId::proportional(Theme::TEXT_SM), t.fg_muted);
    }
    if action.is_none() && resp.clicked() && !gone {
        action = Some(RowAction::Open);
    }
    if hot && !gone {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    action
}

enum Card {
    None,
    Open,
    Delete,
}

/// A template as a card: its name, its tempo, a stripe of its color; one of yours has a
/// delete button when hovered.
#[allow(clippy::too_many_arguments)]
fn template_card(ui: &mut Ui, t: &Theme, name: &str, detail: &str, k: usize, width: f32, mine: bool) -> Card {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 64.0), Sense::click());
    let h = kit::hover(ui, t, &resp);
    let color = if mine { t.accent } else {
        let [r, g, b] = PATTERN_COLORS[(k * 3) % PATTERN_COLORS.len()];
        Color32::from_rgb(r, g, b)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(t.radius_md), Theme::mix(t.bg_panel, color, 0.06 + 0.1 * h));
    painter.rect_stroke(rect, CornerRadius::same(t.radius_md), Stroke::new(1.0, Theme::mix(t.line, color, 0.3 + 0.5 * h)), StrokeKind::Inside);
    painter.rect_filled(Rect::from_min_size(rect.min, vec2(4.0, rect.height())), CornerRadius { nw: t.radius_md, sw: t.radius_md, ne: 0, se: 0 }, color);
    kit::fit_text(painter, pos2(rect.left() + 16.0, rect.center().y - 1.0), Align2::LEFT_BOTTOM, name, Theme::body(), t.fg_text, rect.width() - 44.0);
    painter.text(pos2(rect.left() + 16.0, rect.center().y + 3.0), Align2::LEFT_TOP, detail, Theme::mono(Theme::TEXT_SM), t.fg_muted);
    kit::focus_ring(ui, t, &resp, t.radius_md);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    if mine && ui.rect_contains_pointer(rect) {
        let corner = Rect::from_min_size(pos2(rect.right() - 30.0, rect.top() + 4.0), vec2(26.0, 26.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(corner));
        if kit::icon_button(&mut child, t, icons::TRASH, tr!("Delete this template"), false).clicked() {
            return Card::Delete;
        }
    }
    if resp.clicked() { Card::Open } else { Card::None }
}
