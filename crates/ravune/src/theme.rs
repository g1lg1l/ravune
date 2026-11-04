//! Theme tokens: the only place colors, fonts and sizes are defined.
//! CI fails if `Color32::` shows up in any other file (see docs/THEMING.md §8).
//! Themes are data: four built in, the rest saved in Settings or `.ravune-theme` files
//! (RON, colors as "#rrggbb"). A file missing a token takes Ravune Dark's.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke, TextStyle,
};
use serde::{Deserialize, Serialize};

pub const THEME_EXTENSION: &str = "ravune-theme";
/// Themes saved while Ravune was still called Phasor open too.
pub const THEME_EXTENSIONS: [&str; 2] = [THEME_EXTENSION, "phasor-theme"];

/// Colors as "#rrggbb" in theme files.
mod hex {
    use eframe::egui::Color32;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    fn text(c: &Color32) -> String {
        format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
    }

    fn parse<E: Error>(s: &str) -> Result<Color32, E> {
        Color32::from_hex(s).map_err(|_| E::custom(format!("{s:?} is not a #rrggbb color")))
    }

    pub fn serialize<S: Serializer>(c: &Color32, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&text(c))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Color32, D::Error> {
        parse(&String::deserialize(d)?)
    }

    pub mod list {
        use super::*;

        pub fn serialize<S: Serializer, const N: usize>(c: &[Color32; N], s: S) -> Result<S::Ok, S::Error> {
            s.collect_seq(c.iter().map(text))
        }

        pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(d: D) -> Result<[Color32; N], D::Error> {
            let colors = Vec::<String>::deserialize(d)?.iter().map(|s| parse(s)).collect::<Result<Vec<_>, _>>()?;
            colors.try_into().map_err(|_| D::Error::custom(format!("expected {N} colors")))
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    /// Dark or light base for what egui draws itself (text cursor, scroll bars).
    pub dark: bool,
    /// Window background behind everything.
    #[serde(with = "hex")]
    pub bg_base: Color32,
    /// Bars and side panels.
    #[serde(with = "hex")]
    pub bg_panel: Color32,
    /// Device cards.
    #[serde(with = "hex")]
    pub bg_device: Color32,
    /// Buttons, knob bodies.
    #[serde(with = "hex")]
    pub bg_control: Color32,
    /// Recessed fields: knob tracks, displays, text inputs.
    #[serde(with = "hex")]
    pub bg_field: Color32,
    #[serde(with = "hex")]
    pub line: Color32,
    #[serde(with = "hex")]
    pub fg_text: Color32,
    #[serde(with = "hex")]
    pub fg_muted: Color32,
    /// The one accent: values, active toggles, held keys.
    #[serde(with = "hex")]
    pub accent: Color32,
    /// Selection and "connected" indicators.
    #[serde(with = "hex")]
    pub select: Color32,
    #[serde(with = "hex")]
    pub meter_ok: Color32,
    #[serde(with = "hex")]
    pub meter_warn: Color32,
    #[serde(with = "hex")]
    pub meter_hot: Color32,
    #[serde(with = "hex")]
    pub key_white: Color32,
    #[serde(with = "hex")]
    pub key_black: Color32,
    #[serde(with = "hex")]
    pub key_label: Color32,
    /// Waveform looks (colors from lightrb): mono goes from [0] to [1] as the highs rise.
    #[serde(with = "hex::list")]
    pub wave_mono: [Color32; 2],
    /// RGB look: low, mid and high mixed by their share of the sound.
    #[serde(with = "hex::list")]
    pub wave_rgb: [Color32; 3],
    /// 3-band look (CDJ-3000): low, mid, high layers.
    #[serde(with = "hex::list")]
    pub wave_bands: [Color32; 3],
    pub radius_sm: u8,
    pub radius_md: u8,
    /// Padding inside containers (4-px grid). There are no outer margins anywhere: regions
    /// touch and are split by 1-px dividers, and space lives inside elements.
    pub pad: f32,
    /// Space between siblings in a row or column.
    pub gap: f32,
    /// Header and tool bars.
    pub bar_h: f32,
    /// List rows (browser, palette, menus).
    pub row_h: f32,
    /// Seconds for hover / selection / tab transitions; 0 turns motion off.
    pub motion: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// `Color32::from_rgb` for a 0xrrggbb literal.
const fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

impl Theme {
    /// Nothing at all: where a fade ends.
    pub const CLEAR: Color32 = Color32::TRANSPARENT;
    /// Type scale beside egui's text styles: captions, small print, body, titles.
    pub const TEXT_XS: f32 = 10.0;
    pub const TEXT_SM: f32 = 11.0;
    pub const TEXT_MD: f32 = 13.0;
    pub const TEXT_LG: f32 = 15.0;
    /// Buttons and fields.
    pub const CONTROL_H: f32 = 24.0;

    /// The built-in themes, Ravune Dark first.
    pub fn builtins() -> [Theme; 4] {
        [Self::dark(), Self::light(), Self::scope(), Self::contrast()]
    }

    pub fn dark() -> Self {
        Self {
            name: "Ravune Dark".into(),
            dark: true,
            bg_base: Color32::from_rgb(0x17, 0x17, 0x17),
            bg_panel: Color32::from_rgb(0x21, 0x21, 0x21),
            bg_device: Color32::from_rgb(0x2a, 0x2a, 0x2a),
            bg_control: Color32::from_rgb(0x3a, 0x3a, 0x3a),
            bg_field: Color32::from_rgb(0x12, 0x12, 0x12),
            line: Color32::from_rgb(0x38, 0x38, 0x38),
            fg_text: Color32::from_rgb(0xd6, 0xd6, 0xd6),
            fg_muted: Color32::from_rgb(0x8a, 0x8a, 0x8a),
            accent: Color32::from_rgb(0xff, 0xa5, 0x45),
            select: Color32::from_rgb(0x5e, 0xa8, 0xff),
            meter_ok: Color32::from_rgb(0x3f, 0xcf, 0x6b),
            meter_warn: Color32::from_rgb(0xe6, 0xcf, 0x3a),
            meter_hot: Color32::from_rgb(0xff, 0x4a, 0x4a),
            key_white: Color32::from_rgb(0xc9, 0xc9, 0xc9),
            key_black: Color32::from_rgb(0x1e, 0x1e, 0x1e),
            key_label: Color32::from_rgb(0x6c, 0x6c, 0x6c),
            wave_mono: [Color32::from_rgb(32, 96, 220), Color32::from_rgb(225, 238, 255)],
            wave_rgb: [Color32::from_rgb(255, 0, 0), Color32::from_rgb(0, 230, 0), Color32::from_rgb(0, 0, 255)],
            wave_bands: [Color32::from_rgb(0x22, 0x51, 0xc9), Color32::from_rgb(0xd9, 0x8a, 0x1f), Color32::from_rgb(0xf2, 0xef, 0xe6)],
            radius_sm: 2,
            radius_md: 3,
            pad: 8.0,
            gap: 4.0,
            bar_h: 32.0,
            row_h: 22.0,
            motion: 0.16,
        }
    }

    /// Daylight grey, Ableton "Light"-style: fields recess lighter, the accent darkens.
    pub fn light() -> Self {
        Self {
            name: "Ravune Light".into(),
            dark: false,
            bg_base: rgb(0xcbcbcb),
            bg_panel: rgb(0xbcbcbc),
            bg_device: rgb(0xd6d6d6),
            bg_control: rgb(0xe4e4e4),
            bg_field: rgb(0xeeeeee),
            line: rgb(0xa2a2a2),
            fg_text: rgb(0x161616),
            fg_muted: rgb(0x575757),
            accent: rgb(0xd9660f),
            select: rgb(0x1f6fd1),
            meter_ok: rgb(0x1f9e4a),
            meter_warn: rgb(0xc29b00),
            meter_hot: rgb(0xd92b2b),
            key_white: rgb(0xf4f4f4),
            key_black: rgb(0x2a2a2a),
            key_label: rgb(0x7a7a7a),
            wave_mono: [rgb(0x2a5bd7), rgb(0x0c1630)],
            wave_rgb: [rgb(0xd00000), rgb(0x00a000), rgb(0x0000d0)],
            wave_bands: [rgb(0x2251c9), rgb(0xd98a1f), rgb(0x3a3a3a)],
            ..Self::dark()
        }
    }

    /// Green phosphor on black, like an analog oscilloscope.
    pub fn scope() -> Self {
        Self {
            name: "Scope".into(),
            bg_base: rgb(0x0a0e0b),
            bg_panel: rgb(0x111813),
            bg_device: rgb(0x16201a),
            bg_control: rgb(0x213027),
            bg_field: rgb(0x060907),
            line: rgb(0x22352a),
            fg_text: rgb(0xb9f3c9),
            fg_muted: rgb(0x5f8f6d),
            accent: rgb(0x3dff8c),
            select: rgb(0x2ad4ff),
            meter_ok: rgb(0x3dff8c),
            meter_warn: rgb(0xe6f03a),
            key_white: rgb(0x9fc9ac),
            key_black: rgb(0x0f1712),
            key_label: rgb(0x4c6e57),
            wave_mono: [rgb(0x1d7a45), rgb(0xc8ffd9)],
            wave_bands: [rgb(0x1d7a45), rgb(0x3dff8c), rgb(0xd8ffe5)],
            ..Self::dark()
        }
    }

    /// Black, white and one loud yellow: for bright rooms and tired eyes.
    pub fn contrast() -> Self {
        Self {
            name: "High Contrast".into(),
            bg_base: rgb(0x000000),
            bg_panel: rgb(0x0a0a0a),
            bg_device: rgb(0x111111),
            bg_control: rgb(0x2b2b2b),
            bg_field: rgb(0x000000),
            line: rgb(0x9a9a9a),
            fg_text: rgb(0xffffff),
            fg_muted: rgb(0xc8c8c8),
            accent: rgb(0xffd400),
            select: rgb(0x33b8ff),
            meter_ok: rgb(0x00e05a),
            meter_warn: rgb(0xffd400),
            meter_hot: rgb(0xff3030),
            key_white: rgb(0xffffff),
            key_black: rgb(0x000000),
            key_label: rgb(0x555555),
            wave_mono: [rgb(0x3a8bff), rgb(0xffffff)],
            wave_bands: [rgb(0x3a8bff), rgb(0xffb000), rgb(0xffffff)],
            ..Self::dark()
        }
    }

    /// Every color token with its name, for the theme editor.
    pub fn colors_mut(&mut self) -> Vec<(&'static str, &mut Color32)> {
        let [mono_lo, mono_hi] = &mut self.wave_mono;
        let [rgb_lo, rgb_mid, rgb_hi] = &mut self.wave_rgb;
        let [band_lo, band_mid, band_hi] = &mut self.wave_bands;
        vec![
            ("Background", &mut self.bg_base),
            ("Panels and bars", &mut self.bg_panel),
            ("Device cards", &mut self.bg_device),
            ("Buttons and knobs", &mut self.bg_control),
            ("Fields and displays", &mut self.bg_field),
            ("Dividers", &mut self.line),
            ("Text", &mut self.fg_text),
            ("Muted text", &mut self.fg_muted),
            ("Accent", &mut self.accent),
            ("Selection", &mut self.select),
            ("Meter", &mut self.meter_ok),
            ("Meter, loud", &mut self.meter_warn),
            ("Meter, clipping", &mut self.meter_hot),
            ("White keys", &mut self.key_white),
            ("Black keys", &mut self.key_black),
            ("Key labels", &mut self.key_label),
            ("Wave mono, lows", mono_lo),
            ("Wave mono, highs", mono_hi),
            ("Wave RGB, lows", rgb_lo),
            ("Wave RGB, mids", rgb_mid),
            ("Wave RGB, highs", rgb_hi),
            ("Wave 3-band, lows", band_lo),
            ("Wave 3-band, mids", band_mid),
            ("Wave 3-band, highs", band_hi),
        ]
    }

    /// A few colors that show what a theme looks like (theme lists).
    pub fn swatches(&self) -> [Color32; 5] {
        [self.bg_base, self.bg_device, self.fg_text, self.accent, self.select]
    }

    /// A pattern's own color (the user picks it; themes don't), else its palette color.
    pub fn pattern(index: usize, color: Option<[u8; 3]>) -> Color32 {
        let [r, g, b] = color.unwrap_or(ravune_core::PATTERN_COLORS[index % ravune_core::PATTERN_COLORS.len()]);
        Color32::from_rgb(r, g, b)
    }

    /// Mixes two token colors (`t` = 0 → a, 1 → b).
    pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
        a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
    }

    /// Sum of token colors scaled by weights (clamped): the RGB waveform look.
    pub fn blend(colors: [Color32; 3], weights: [f32; 3]) -> Color32 {
        let channel = |f: fn(&Color32) -> u8| {
            colors.iter().zip(weights).map(|(c, w)| f(c) as f32 * w).sum::<f32>().clamp(0.0, 255.0) as u8
        };
        Color32::from_rgb(channel(Color32::r), channel(Color32::g), channel(Color32::b))
    }

    /// Monospace for numbers, values and readouts.
    pub fn mono(size: f32) -> FontId {
        FontId::monospace(size)
    }

    pub fn body() -> FontId {
        FontId::proportional(Self::TEXT_MD)
    }

    pub fn small() -> FontId {
        FontId::proportional(Self::TEXT_SM)
    }

    /// A light made of token colors, premultiplied, 0..1 (the AI chat's border beam).
    pub fn premul([r, g, b, a]: [f32; 4]) -> Color32 {
        let to = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
        Color32::from_rgba_premultiplied(to(r), to(g), to(b), to(a))
    }

    /// Generates egui's visuals and style from our tokens.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.panel_fill = self.bg_panel;
        v.window_fill = self.bg_device;
        v.faint_bg_color = self.bg_device;
        v.extreme_bg_color = self.bg_field;
        v.code_bg_color = self.bg_field;
        v.hyperlink_color = self.select;
        v.warn_fg_color = self.meter_warn;
        v.error_fg_color = self.meter_hot;
        v.window_stroke = Stroke::new(1.0, self.line);
        v.window_corner_radius = CornerRadius::same(self.radius_md);
        v.menu_corner_radius = CornerRadius::same(self.radius_md);
        let shade = if self.dark { 120 } else { 50 };
        v.window_shadow = Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(shade) };
        v.popup_shadow = Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(shade / 5 * 4) };
        // Active toggles: amber with dark text, like a lit hardware button.
        v.selection.bg_fill = self.accent;
        v.selection.stroke = Stroke::new(1.0, self.bg_base);
        let hover = Self::mix(self.bg_control, self.fg_text, 0.1);
        let w = &mut v.widgets;
        for (state, fill, stroke, text) in [
            (&mut w.noninteractive, self.bg_device, self.line, self.fg_muted),
            (&mut w.inactive, self.bg_control, self.bg_control, self.fg_text),
            (&mut w.hovered, hover, Self::mix(self.line, self.fg_text, 0.3), self.fg_text),
            (&mut w.active, Self::mix(self.bg_control, self.accent, 0.4), self.accent, self.fg_text),
            (&mut w.open, self.bg_control, self.line, self.fg_text),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.fg_stroke = Stroke::new(1.0, text);
            state.corner_radius = CornerRadius::same(self.radius_sm);
            state.expansion = 0.0;
        }
        // egui keeps a style for the OS's dark mode and one for its light mode and follows the
        // OS, which it may only learn after this runs: both get ours, or a light Windows
        // shows egui's own white widgets until the theme is applied again
        ctx.all_styles_mut(|s| {
            s.visuals = v.clone();
            s.spacing.item_spacing = egui::vec2(self.gap, self.gap);
            s.spacing.button_padding = egui::vec2(8.0, 3.0);
            s.spacing.interact_size.y = 22.0;
            s.spacing.menu_margin = egui::Margin::same(4);
            // scroll bars sit flush on the region's edge (the divider), no gaps around them
            s.spacing.scroll = egui::style::ScrollStyle {
                bar_width: 8.0,
                handle_min_length: 24.0,
                bar_inner_margin: 0.0,
                bar_outer_margin: 0.0,
                content_margin: egui::Margin::ZERO,
                ..egui::style::ScrollStyle::solid()
            };
            s.animation_time = self.motion;
            s.text_styles = [
                (TextStyle::Heading, FontId::proportional(15.0)),
                (TextStyle::Body, FontId::proportional(13.0)),
                (TextStyle::Button, FontId::proportional(13.0)),
                (TextStyle::Small, FontId::proportional(11.0)),
                (TextStyle::Monospace, FontId::monospace(12.0)),
            ]
            .into();
        });
    }
}

/// Inter for UI text, JetBrains Mono for values and readouts (both SIL OFL), Phosphor for
/// icons (MIT). All three are pinned to the one weight we draw and subset to the glyphs we
/// use (~140 KB together); egui's own fallback fonts are left out. See assets/fonts.
pub fn install_fonts(ctx: &egui::Context) {
    let font = |bytes: &'static [u8]| Arc::new(FontData::from_static(bytes));
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("inter".into(), font(include_bytes!("../assets/fonts/Inter.ttf")));
    fonts.font_data.insert("jbmono".into(), font(include_bytes!("../assets/fonts/JetBrainsMono.ttf")));
    // Phosphor icons (MIT) sit in the Private Use Area. They go first: Inter has a few
    // private glyphs of its own there, and Phosphor has nothing else to shadow.
    fonts.font_data.insert("phosphor".into(), font(include_bytes!("../assets/fonts/Phosphor.ttf")));
    for (family, text) in [(FontFamily::Proportional, "inter"), (FontFamily::Monospace, "jbmono")] {
        let list = fonts.families.entry(family).or_default();
        list.insert(0, "phosphor".into());
        list.insert(1, text.into());
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_round_trip_through_files() {
        for theme in Theme::builtins() {
            let text = ron::ser::to_string_pretty(&theme, ron::ser::PrettyConfig::default()).unwrap();
            assert!(text.contains("accent: \"#"), "colors are written as hex");
            assert!(ron::from_str::<Theme>(&text).unwrap() == theme, "{}", theme.name);
        }
        // a file with only a few tokens takes the rest from Ravune Dark
        let partial: Theme = ron::from_str(r##"(name: "Mine", accent: "#00ff00")"##).unwrap();
        assert_eq!(partial.accent, rgb(0x00ff00));
        assert_eq!(partial.bg_base, Theme::dark().bg_base);
        assert!(ron::from_str::<Theme>(r#"(accent: "green")"#).is_err());
    }

    #[test]
    fn the_theme_holds_whichever_mode_the_os_is_in() {
        let ctx = egui::Context::default();
        Theme::dark().apply(&ctx);
        // the OS turns out to be light after the theme went on
        ctx.set_theme(egui::Theme::Light);
        assert_eq!(ctx.global_style().visuals.panel_fill, Theme::dark().bg_panel);
        assert_eq!(ctx.style_of(egui::Theme::Dark).visuals.panel_fill, Theme::dark().bg_panel);
    }
}
