//! Workspaces (Blender-style): Beat, Compose, Sound, Mix and your own, as tabs in the top
//! bar. Each remembers its layout: which panels are open, the docked and split editors,
//! the floating windows, the browser tab. Switching saves the one you leave.

use eframe::egui::{Frame, Id, RichText, Ui};
use serde::{Deserialize, Serialize};

use crate::browser::Tab as BrowserTab;
use crate::shell::{EDITORS, Rename, Renaming};
use crate::theme::Theme;
use crate::{App, Tab, icons, kit};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub name: String,
    pub browser: bool,
    pub browser_tab: BrowserTab,
    pub editor: bool,
    pub keys: bool,
    pub tab: Tab,
    /// A second editor beside the docked one.
    pub split: Option<Tab>,
    pub floating: Vec<Tab>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::builtins().swap_remove(0)
    }
}

impl Workspace {
    pub fn builtins() -> Vec<Workspace> {
        let w = |name: &str, browser, browser_tab, keys, tab, split| Workspace { name: name.into(), browser, browser_tab, editor: true, keys, tab, split, floating: Vec::new() };
        vec![
            w("Beat", true, BrowserTab::Sounds, false, Tab::Rack, None),
            w("Compose", false, BrowserTab::Sounds, false, Tab::PianoRoll, Some(Tab::Rack)),
            w("Sound", true, BrowserTab::Instruments, false, Tab::Rack, Some(Tab::Modulation)),
            w("Mix", false, BrowserTab::Sounds, false, Tab::Mixer, None),
        ]
    }

    fn icon(&self) -> &'static str {
        match self.name.as_str() {
            "Beat" => icons::GRID_FOUR,
            "Compose" => icons::MUSIC_NOTES,
            "Sound" => icons::WAVE_SINE,
            "Mix" => icons::FADERS,
            _ => icons::LAYOUT,
        }
    }
}

impl App {
    /// Copies the live layout into the current workspace.
    pub fn keep_workspace(&mut self) {
        let s = &mut self.settings;
        if s.workspaces.is_empty() {
            s.workspaces = Workspace::builtins();
        }
        s.workspace = s.workspace.min(s.workspaces.len() - 1);
        let w = &mut s.workspaces[s.workspace];
        (w.browser, w.editor, w.keys) = (s.show_browser, s.show_editor, s.show_keys);
        w.floating.clone_from(&s.floating);
        (w.browser_tab, w.tab, w.split) = (self.browser.tab, self.tab, self.split);
    }

    /// Lays the window out as workspace `i` says.
    pub fn load_workspace(&mut self, i: usize) {
        let s = &mut self.settings;
        if s.workspaces.is_empty() {
            s.workspaces = Workspace::builtins();
        }
        s.workspace = i.min(s.workspaces.len() - 1);
        let w = s.workspaces[s.workspace].clone();
        (s.show_browser, s.show_editor, s.show_keys) = (w.browser, w.editor, w.keys);
        // the Device tab became the rack's channel settings: older layouts get the rack
        let device = |t: Tab| if t == Tab::Device { Tab::Rack } else { t };
        s.floating = w.floating.into_iter().filter(|t| *t != Tab::Device).collect();
        self.browser.tab = w.browser_tab;
        self.tab = device(w.tab);
        self.split = w.split.map(device).filter(|t| *t != self.tab);
    }

    pub fn switch_workspace(&mut self, i: usize) {
        if i != self.settings.workspace && i < self.settings.workspaces.len() {
            self.keep_workspace();
            self.load_workspace(i);
        }
    }

    /// Next (+1) or previous (-1) workspace, wrapping around.
    pub fn step_workspace(&mut self, step: isize) {
        let n = self.settings.workspaces.len() as isize;
        self.switch_workspace((self.settings.workspace as isize + step).rem_euclid(n.max(1)) as usize);
    }

    /// The workspace tabs in the top bar (the active one shows its name) and their menu.
    pub fn workspace_tabs(&mut self, ui: &mut Ui, t: &Theme) {
        let names: Vec<(usize, &str, String)> = self.settings.workspaces.iter().enumerate().map(|(i, w)| (i, w.icon(), crate::i18n::t(&w.name).to_string())).collect();
        let items: Vec<(usize, &str, &str)> = names.iter().map(|(i, icon, name)| (*i, *icon, name.as_str())).collect();
        if let Some(i) = kit::tabs(ui, t, Id::unique("workspaces"), self.settings.workspace, &items, true) {
            self.switch_workspace(i);
        }
        let tip = self.tip(tr!("Workspaces: new, rename, reset, delete · switch"), "workspace.next");
        let r = kit::icon_button(ui, t, icons::CARET_DOWN, &tip, false);
        kit::menu(t, &r).show(|ui| self.workspace_menu(ui));
    }

    fn workspace_menu(&mut self, ui: &mut Ui) {
        let i = self.settings.workspace;
        let name = self.settings.workspaces[i].name.clone();
        let shown = crate::i18n::t(&name).to_string();
        if ui.button(format!("{}  {}", icons::PLUS, tr!("New workspace from this layout"))).clicked() {
            self.keep_workspace();
            let mut w = self.settings.workspaces[i].clone();
            w.name = tr!("Workspace {}", self.settings.workspaces.len() + 1);
            self.rename = Some(Renaming::new(Rename::Workspace(self.settings.workspaces.len()), w.name.clone()));
            self.settings.workspaces.push(w);
            self.settings.workspace = self.settings.workspaces.len() - 1;
        }
        if ui.button(format!("{}  {}", icons::PENCIL_SIMPLE, tr!("Rename “{}”…", shown))).clicked() {
            self.rename = Some(Renaming::new(Rename::Workspace(i), name.clone()));
        }
        if let Some(default) = Workspace::builtins().into_iter().find(|b| b.name == name)
            && ui.button(format!("{}  {}", icons::ARROW_COUNTER_CLOCKWISE, tr!("Back to its first layout"))).clicked()
        {
            self.settings.workspaces[i] = default;
            self.load_workspace(i);
        }
        if self.settings.workspaces.len() > 1 && ui.button(format!("{}  {}", icons::TRASH, tr!("Delete “{}”", shown))).clicked() {
            self.settings.workspaces.remove(i);
            self.load_workspace(i.saturating_sub(1)); // the deleted one isn't kept
        }
    }

    /// The split side of the editor area: its own compact tabs and a close button.
    pub fn split_area(&mut self, ui: &mut Ui, split: Tab) {
        let t = self.theme.clone();
        kit::region(ui);
        kit::bar(ui, &t, |ui| {
            let others: Vec<(Tab, &str, &str)> = EDITORS.iter().filter(|e| e.0 != self.tab).map(|e| (e.0, e.1, crate::i18n::t(e.2))).collect();
            if let Some(tab) = kit::tabs(ui, &t, Id::unique("split-tabs"), split, &others, true) {
                self.split = Some(tab);
            }
            kit::right(ui, |ui| {
                if kit::icon_button(ui, &t, icons::X, tr!("Close the split"), false).clicked() {
                    self.split = None;
                }
            });
        });
        if self.settings.floating.contains(&split) {
            Frame::new().inner_margin(t.pad).show(ui, |ui| {
                ui.label(RichText::new(tr!("This editor is open in its own window.")).color(t.fg_muted));
            });
        } else {
            self.editor(ui, split);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_never_split_an_editor_with_itself() {
        for w in Workspace::builtins() {
            assert_ne!(w.split, Some(w.tab), "{}", w.name);
            assert_ne!(w.icon(), icons::LAYOUT, "{} has its own icon", w.name);
        }
    }
}
