//! Help: the Manual (a window of chapters, each a few numbered steps) and About.

use eframe::egui::{self, Align2, Id, RichText, Ui, pos2, vec2};

use crate::theme::Theme;
use crate::{App, icons, kit, widgets};

const SOURCE: &str = "https://github.com/g1lg1l/ravune";
/// Velo, the sister app for live visuals.
const VELO: &str = "https://g1lg1l.github.io/velo/";

/// A chapter: icon, title, a line on what it's for, then its steps (title, how).
type Chapter = (&'static str, &'static str, &'static str, &'static [(&'static str, &'static str)]);

const MANUAL: [Chapter; 14] = [
    (
        icons::SQUARES_FOUR,
        "Your first beat",
        "From an empty project to a beat you can save, in six steps.",
        &[
            ("Pick sounds", "In the browser on the left, Sounds: click a sound to hear it, double-click it (or Enter) to add it as a channel. The kit's Kicks, Snares and Hi-hats are a good start."),
            ("Paint steps", "Channel Rack (F6): click the squares to place hits, drag across to paint, hold the right button to erase. Each row is a channel; 16 steps make a bar."),
            ("Play it", "Space plays and stops. PAT loops the pattern you're on, SONG plays the playlist."),
            ("Add a melody", "Synths in the browser: add a VA-1 preset. Then the Piano Roll (F7): click to draw notes, drag to move them, drag the right edge to resize. The lane underneath sets each note's velocity."),
            ("Arrange", "Right-click the playlist to insert the pattern, or drag it in from Patterns in the browser. Select clips and Ctrl+D to keep duplicating forward."),
            ("Mix and save", "Mixer (F9): faders, pan, and each track's effects on the right. Ctrl+S saves the project; File → Export renders it as WAV, FLAC, Ogg or MP3."),
        ],
    ),
    (
        icons::CHART_LINE,
        "Automate anything",
        "Make a knob move over time: on the playlist, inside a pattern, or on one clip.",
        &[
            ("On the playlist", "Right-click any knob → Create automation clip: a line under everything, as long as the song. Click it to add points, drag them, right-click one to delete it."),
            ("Inside a pattern", "Right-click a knob → Automate in \"pattern\": the line lives in the pattern and plays wherever the pattern does. Draw it under the piano roll, in AUTO."),
            ("On a sound's clip", "Double-click an audio clip: its Envelopes move its volume, pan and a DJ filter. You can also edit those lines right on the playlist."),
            ("Modulation and controllers", "Right-click a knob → a macro or an LFO (their amounts are on Modulation, F10), or MIDI learn to move it from a controller."),
        ],
    ),
    (
        icons::FOLDER_OPEN,
        "Find sounds",
        "The browser on the left: the built-in kit and genre packs, your folders, free sounds online.",
        &[
            ("Listen", "Sounds: click a sound or arrow onto it to hear it (▶ always plays it). The kit has drums, percussion, 808s, FX and packs for House, Techno, Hardgroove, Drum & Bass, Liquid DnB, IDM and Electronic."),
            ("Add it", "Double-click or Enter adds it as a sampler channel; or drag it onto the rack, or onto the playlist as an audio clip. ☆ keeps it in Favorites."),
            ("Your own folders", "Add a folder with your sounds… (or drop a folder onto the window). The search box filters the kit, favorites and every folder, subfolders too."),
            ("Free sounds online", "Online: Openverse, Wikimedia Commons, ccMixter, Freesound (free key) or a source of your own. Only free licenses; what you use is kept with its license, and File → Export credits… writes them down."),
            ("Instruments and plugins", "Synths holds VA-1 presets; .sfz and .dspreset instruments sit next to your samples. Plugins lists the CLAP and VST3 plugins it found; each runs in its own process."),
        ],
    ),
    (
        icons::GRID_FOUR,
        "Channel Rack and patterns",
        "Patterns are short loops: each channel has a row of steps, and its notes in the piano roll.",
        &[
            ("Channels", "Each row is a channel: a sampler, a VA-1 or a plugin. M mutes it, S solos it; drop another sound or a plugin on its row to replace what it plays."),
            ("Steps", "Click to toggle, drag to paint, hold the right button to erase. Each step is a 16th; louder and softer hits are the velocity lane in the piano roll."),
            ("Length and swing", "The pattern's length is in 16ths: drag it, type any length, right-click for bars. Swing pushes every second 16th late for a shuffle."),
            ("More patterns", "F4 makes a new pattern; all of them are in the browser's Patterns tab, where you rename, color and drag them onto the playlist."),
            ("Channel settings", "Select a channel: its settings open on the right of the rack and the piano roll (F8 shows or hides them). The sampler's knobs, the VA-1's, or the plugin; the mixer track it plays into and that track's effects."),
        ],
    ),
    (
        icons::PIANO_KEYS,
        "Piano roll",
        "Melodies, chords and basslines for the selected channel, in the current pattern.",
        &[
            ("Draw", "Click to draw, drag to move, drag the right edge to resize; snap follows the zoom (Alt: free). Hold the right button to erase. Ctrl+drag selects several, then drag them together."),
            ("Stay in key", "Set the song's key and scale on the piano roll's toolbar; with Fold to scale on, new and moved notes snap to it. Chord stamp draws a whole chord with one click."),
            ("The lane underneath", "VEL, PITCH, BRIGHT and AUTO pick what the bars below edit: velocity, a note's pitch bend, its brightness, or a knob automated inside the pattern."),
            ("Tools", "Quantize, legato, strum and humanize work on the channel's notes. Scripts adds arpeggios, Euclidean rhythms, random basslines in key, reverse, half and double time."),
            ("Play it in", "The computer keyboard plays the channel (A–K notes, Z / X octave, C / V velocity), and so does any MIDI controller: plug it in, it's picked up."),
        ],
    ),
    (
        icons::WAVE_SAWTOOTH,
        "Sound design",
        "Shape what a channel sounds like: the VA-1, the sampler, instruments and plugins.",
        &[
            ("VA-1", "The built-in synth: two oscillators (sine, triangle, saw, square; the second one tuned and detuned), a low-pass filter with resonance and its own envelope, and the amp envelope. Synths in the browser has presets; the dice rolls a new patch in musical ranges."),
            ("Sampler", "A sound in the rack. Its panel is laid out like an audio clip's: the sound on top (drag its handles, or turn Start and End, to cut it; press it to hear it), One-shot or Loop (round and round while the note is held), Pitch, Volume, then Attack, Release and Choke (open and closed hats in one group cut each other), then MIX: the track it plays into, and an EQ, a reverb, a delay and any effect for this sound alone (the first touch gives it a mixer track of its own)."),
            ("Instruments", ".sfz and .dspreset multi-sample instruments play across the keys, with round robins, loops and release, like a sampled piano."),
            ("Plugins", "CLAP and VST3 instruments and effects from the browser's Plugins tab, or dropped on a channel to replace what it plays. Each runs in its own process: if one crashes it goes quiet, Ravune carries on, and Reload on its device brings it back."),
            ("Groove", "Right-click a channel → Swing to give it a swing of its own: straight hats over a shuffled beat, or the other way round."),
        ],
    ),
    (
        icons::STACK,
        "Playlist and arrangement",
        "The song: clips of patterns, sounds and automation on lanes, from left to right.",
        &[
            ("Place clips", "Right-click a lane to insert a pattern, or drag one from the browser. The tools on top: select, draw, paint copies, erase, slice, mute. Ctrl+D duplicates forward."),
            ("Lanes", "Click a lane to fold it, double-click to rename; M and S mute and solo it. Right-click for color, insert and delete."),
            ("Loop and markers", "Right-drag along the ruler to loop a region. Markers sit on the ruler: click one to jump there, drag it, double-click to rename it."),
            ("Audio clips", "Double-click one for its properties, laid out like a sampler's: its sound on top (the handles, or Start and End, cut it; the envelope picked under it is drawn over it), One-shot or Loop, pitch, gain, fades, how it follows the tempo (Off, Resample, Stretch), and MIX: the track it plays into, and an EQ, a reverb, a delay and any effect for this sound alone (the first touch gives it a mixer track of its own). Right-click: split it into drums and tones, or turn a melody into notes."),
            ("A song from loops", "Create → Song from the patterns lays your patterns out as intro, build, drop, break, drop 2 and outro, with markers. Then change what you like."),
        ],
    ),
    (
        icons::MAGIC_WAND,
        "Smart helpers",
        "Small tools that listen to your sounds, all offline.",
        &[
            ("Drums and tones", "Right-click an audio clip → Split into drums and tones: two new clips under it, the hits and everything that holds a pitch, with the original muted."),
            ("Notes from a sound", "Right-click a clip with one voice (a bassline, a vocal, a lead) → Notes from this sound: its notes in a new pattern for a VA-1."),
            ("Tempo of a loop", "Ravune reads a loop's tempo from its name or its beat, so it follows the song: Stretch keeps the pitch, Resample plays it like a turntable."),
        ],
    ),
    (
        icons::FADERS,
        "Mixer and effects",
        "Every channel plays into a mixer track; tracks feed buses and the master.",
        &[
            ("Tracks", "Mixer (F9): a strip per track with pan, a three-band EQ (low, mid, high), fader and meter; M silences one, S solos it with whatever it feeds and whatever feeds it. + Track adds one."),
            ("Routing", "Pull a cable from a strip's jack onto another: that track plays into it (a bus). With Shift, a send instead, for a shared reverb or delay."),
            ("Effects", "Click a strip: its effects open on the right, top to bottom before the fader. + Effect adds an EQ Three, Compressor, Filter, Saturator, Chorus, Delay, Reverb, Limiter or a plugin."),
            ("Ducking", "Duck to… makes the track dip whenever another one plays: the bass under the kick, in one click."),
            ("Macros and LFOs", "Right-click any knob or fader to map a macro or an LFO onto it, or to MIDI-learn it; the amounts are on Modulation (F10)."),
        ],
    ),
    (
        icons::WAVE_SINE,
        "Modulation",
        "Knobs that move by themselves: macros, LFOs, recorded moves and controllers.",
        &[
            ("Macros", "Eight knobs on Modulation (F10), each moving whatever you map to it: right-click any knob → a macro, or Alt+drag a macro onto the knob."),
            ("Rand and variations", "Rand throws every mapped macro somewhere new; + keeps where they are as a variation, and a click on its number brings them back."),
            ("LFOs", "+ LFO adds one: sine, triangle, saw, square or random (a new level each cycle), synced to the bar or free in Hz. Drag its handle onto any knob to map it."),
            ("Amounts", "Every mapping is listed under the sources with its amount, a share of the knob's whole travel; a negative amount turns it upside down."),
            ("Record the moves", "Arm the automation button next to Record and turn knobs while the song plays: the moves land in automation clips, one undo step per turn."),
            ("Controllers", "Right-click → MIDI learn, then move a knob on your controller. MPE controllers bend, press and slide every note on its own."),
        ],
    ),
    (
        icons::EXPORT,
        "Record and export",
        "From the song to files: a mixdown, stems, or a project for another DAW.",
        &[
            ("Record", "The record button captures what plays into a new clip on the playlist."),
            ("Export", "File → Export renders the song as WAV (16, 24 or 32-bit float), FLAC, Ogg or MP3 (with ffmpeg). The format picked last keeps the export key."),
            ("Stems", "File → Export → Stems… writes a file per mixer track, with its reverb and delay tails, ready for mixing elsewhere."),
            ("Other DAWs", "File → Export → DAWproject… carries notes, audio and the mixer to Bitwig, Studio One, Cubase and others; File → Open reads a .dawproject back."),
        ],
    ),
    (
        icons::ROBOT,
        "Scripts and agents",
        "Let code or an AI assistant do the repetitive part: every change undoes in one step.",
        &[
            ("Scripts", "Luau scripts run on the selected channel: from the piano roll's Tools → Scripts or the command palette (Ctrl K). Put your own .luau files in Documents/Ravune/Scripts."),
            ("Write one", "A script calls ravune.run(command, props) with any command the palette lists, and reads ravune.notes(), ravune.pattern(), ravune.scale(). It stops after 2 seconds; a failed run leaves nothing behind."),
            ("Agents", "Preferences → Agents lets Claude and other MCP clients search commands, edit the project, find sounds and listen to a render. It's off until you turn it on."),
            ("Chat", "Ctrl I opens the agent chat on the right, full height. Write what you want and Claude Code does it in your project: it runs your own Claude Code (installed, and signed in once in a terminal) with nothing but Ravune's commands, and each reply undoes in one step."),
            ("Undo what it did", "Everything an agent changes between your own edits is one undo step: Ctrl Z, and it's gone. The log in Preferences → Agents shows every call."),
        ],
    ),
    (
        icons::PUZZLE_PIECE,
        "Templates and extensions",
        "Start from a genre, or add themes, scripts, sounds and templates others made.",
        &[
            ("Templates", "File → New from Template: Techno, House, Drum & Bass, Liquid DnB, Hardgroove, IDM, Hip-hop. Eight bars each, with a kit, a beat, a bass, chords, a loop from the pack and a mixer set up."),
            ("Extensions", "Preferences → Extensions installs a .ravunext from a file or the gallery. Its themes join the theme list, its scripts the palette, its templates New from Template, its sounds the browser."),
            ("Make one", "A .ravunext is a zip: extension.json (name, version, author, about) and folders themes, scripts, templates and sounds. Nothing else in it is unpacked."),
            ("Updates", "Ravune asks GitHub for the latest release when it starts (Preferences → Extensions turns that off) or on Help → Check for Updates. When a newer one is out, Preferences → Extensions shows it: Update downloads it and swaps it in, Restart now opens it. A copy installed for all users on Windows runs the new installer on restart instead."),
        ],
    ),
    (
        icons::PALETTE,
        "Make it yours",
        "Looks, keys and layouts: all in Preferences (Ctrl ,) and the View menu.",
        &[
            ("Themes", "Preferences → Look: pick a theme, or Edit theme… to change any color, live, and export it to share it; Import… brings one in. Reduce motion turns the animations off."),
            ("Keys", "Preferences → Keys: presets like FL Studio's and Ableton's, or click any shortcut to change it. F1 lists them all."),
            ("Layout", "Split an editor in two, or Shift with an editor's key (F6, F7, F9, F10) to open it in its own window. Workspaces keep a whole layout: make a new one from the current layout and switch between them."),
            ("Language", "Ravune speaks English and Italian: switch in Preferences → Look."),
        ],
    ),
];

impl App {
    /// Help → Manual: the chapters on the left, the one picked on the right.
    pub fn manual(&mut self, ctx: &egui::Context) {
        if !self.show_manual {
            return;
        }
        let t = self.theme.clone();
        let mut close = false;
        egui::Window::new("Manual")
            .id(Id::unique("manual"))
            .title_bar(false)
            .frame(kit::window_frame(ctx, &t))
            .resizable(true)
            .default_size(vec2(680.0, 460.0))
            .default_pos(pos2(ctx.content_rect().center().x - 340.0, 90.0))
            .show(ctx, |ui| {
                kit::region(ui);
                kit::bar(ui, &t, |ui| {
                    ui.label(RichText::new(format!("{}  {}", icons::INFO, tr!("Manual"))).strong());
                    kit::right(ui, |ui| close = kit::icon_button(ui, &t, icons::X, tr!("Close (Esc)"), false).clicked());
                });
                ui.horizontal_top(|ui| {
                    // chapters
                    ui.vertical(|ui| {
                        ui.set_width(190.0);
                        for (k, (icon, title, _, _)) in MANUAL.iter().enumerate() {
                            let r = kit::row(ui, &t, Id::unique(("manual-chapter", k)), self.manual_chapter == k);
                            let painter = ui.painter_at(r.rect);
                            painter.text(r.rect.left_center() + vec2(t.pad + 7.0, 0.0), Align2::CENTER_CENTER, *icon, egui::FontId::proportional(14.0), t.fg_muted);
                            painter.text(r.rect.left_center() + vec2(t.pad + 22.0, 0.0), Align2::LEFT_CENTER, crate::i18n::t(title), egui::FontId::proportional(12.5), t.fg_text);
                            if r.clicked() || r.gained_focus() {
                                self.manual_chapter = k;
                            }
                        }
                    });
                    let rect = ui.available_rect_before_wrap();
                    ui.painter().vline(rect.left() - 0.5, rect.y_range(), egui::Stroke::new(1.0, t.line));
                    // the chapter
                    kit::scroll(ui, &t, "manual-text", true, t.bg_base, |ui| self.chapter(ui, &t));
                });
            });
        if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_manual = false;
        }
    }

    fn chapter(&mut self, ui: &mut Ui, t: &Theme) {
        let (_, title, about, steps) = MANUAL[self.manual_chapter.min(MANUAL.len() - 1)];
        ui.label(RichText::new(crate::i18n::t(title)).size(17.0).strong().color(t.fg_text));
        ui.label(RichText::new(crate::i18n::t(about)).color(t.fg_muted));
        ui.add_space(t.gap * 2.0);
        for (k, (step, how)) in steps.iter().enumerate() {
            ui.horizontal_top(|ui| {
                // the step's number in a circle, then what to do
                let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 10.0, Theme::mix(t.bg_control, t.accent, 0.25));
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, (k + 1).to_string(), Theme::mono(11.0), t.accent);
                ui.vertical(|ui| {
                    ui.label(RichText::new(crate::i18n::t(step)).strong().color(t.fg_text));
                    ui.label(RichText::new(crate::i18n::t(how)).color(t.fg_text));
                });
            });
            ui.add_space(t.gap * 2.0);
        }
        ui.label(RichText::new(tr!("F1 lists every shortcut · {} finds any command", self.settings.keymap.label("app.palette"))).small().color(t.fg_muted));
    }

    /// Help → About Ravune.
    pub fn about(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let t = self.theme.clone();
        let (_, close) = kit::modal(ctx, &t, "about", tr!("About Ravune"), 380.0, |ui| {
            ui.horizontal(|ui| {
                widgets::logo(ui, &t, None);
                ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).font(Theme::mono(12.0)).color(t.fg_muted));
            });
            ui.label(RichText::new(tr!("A modular DAW for deep sound design.")).color(t.fg_text));
            ui.label(RichText::new(tr!("Made by gilgil")).strong().color(t.accent));
            ui.hyperlink_to(format!("{}  {}", icons::ARROW_SQUARE_OUT, tr!("Source on GitHub")), SOURCE);
            ui.label(RichText::new(tr!("Free and open source: MIT or Apache-2.0, your pick.")).small().color(t.fg_muted));
            ui.label(RichText::new(tr!("Built with egui, cpal, symphonia, clack and vst3 · icons by Phosphor · motion after React Bits · the built-in kit is CC0.")).small().color(t.fg_muted));
            // the sister app: same maker, same spirit, for the picture side of a set
            ui.add_space(t.gap);
            kit::caps(ui, &t, tr!("SISTER PROJECT"));
            ui.label(RichText::new(tr!("Making visuals too? Velo, from the same maker, turns your music into live visuals: every kick, bassline and melody moves the picture.")).color(t.fg_text));
            ui.hyperlink_to(format!("{}  {}", icons::ARROW_SQUARE_OUT, tr!("Velo · sound shapes light")), VELO);
        });
        if close {
            self.show_about = false;
        }
    }

    /// The Help menu (menu bar and the ? button).
    pub fn help_menu(&mut self, ui: &mut Ui) {
        if ui.button(format!("{}  {}", icons::INFO, tr!("Manual"))).clicked() {
            self.show_manual = true;
            ui.close();
        }
        if ui.add(egui::Button::new(format!("{}  {}", icons::KEYBOARD, tr!("Keyboard shortcuts"))).shortcut_text("F1")).clicked() {
            self.show_help = true;
            ui.close();
        }
        ui.menu_button(format!("{}  {}", icons::GRADUATION_CAP, tr!("Lessons")), |ui| self.lessons_menu(ui));
        ui.separator();
        ui.hyperlink_to(format!("{}  {}", icons::ARROW_SQUARE_OUT, tr!("Source on GitHub")), SOURCE);
        if ui.button(format!("{}  {}", icons::ARROW_CLOCKWISE, tr!("Check for Updates…"))).clicked() {
            (self.show_settings, self.prefs_tab) = (true, crate::shell::PrefsTab::Extensions);
            self.check_updates(ui.ctx());
            ui.close();
        }
        if ui.button(format!("{}  {}", icons::STAR, tr!("About Ravune"))).clicked() {
            self.show_about = true;
            ui.close();
        }
    }
}
