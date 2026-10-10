//! Audition (CS-era) style palette, bundled Adobe open-source fonts, and
//! hand-drawn flat icons (no icon font needed).

use eframe::egui::{self, pos2, vec2, Color32, FontData, FontDefinitions, FontFamily, Pos2, Rect, Response, Rounding, Sense, Shape, Stroke, Ui};

// Palette: each colour lives in an atomic so Preferences > Appearance can
// change it at run time. The functions keep their constant-like names.
macro_rules! palette {
    ($($(#[$m:meta])* $name:ident = $rgb:expr;)*) => {
        mod cells {
            use std::sync::atomic::AtomicU32;
            $( #[allow(non_upper_case_globals)] pub static $name: AtomicU32 = AtomicU32::new($rgb); )*
        }
        $( $(#[$m])* #[allow(non_snake_case)] #[inline] pub fn $name() -> Color32 { rgb(cells::$name.load(std::sync::atomic::Ordering::Relaxed)) } )*
        /// Every palette entry at its built-in value.
        pub const DEFAULT_PALETTE: &[(&str, u32)] = &[$((stringify!($name), $rgb)),*];
        fn set(name: &str, v: u32) {
            match name { $(stringify!($name) => cells::$name.store(v, std::sync::atomic::Ordering::Relaxed),)* _ => {} }
        }
    };
}

fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

palette! {
    /// gutters between panels
    BG_DEEP = 0x262626;
    /// panel body
    BG_PANEL = 0x3a3a3a;
    /// panel tab strip
    BG_HEADER = 0x2c2c2c;
    /// list / table wells
    BG_LIST = 0x333333;
    /// buttons
    BG_RAISED = 0x4a4a4a;
    BORDER = 0x222222;
    TEXT = 0xd6d6d6;
    TEXT_DIM = 0x9a9a9a;
    /// Audition's orange-yellow "hot text" for times and scrubbable values.
    HOT = 0xe8a13a;
    ACCENT = 0x2d7fd9;
    LANE_BG = 0x0f1317;
    LANE_GRID = 0x1f2a2a;
    WAVE = 0x3fdc9b;
    WAVE_DIM = 0x2d6e55;
    /// Waveform drawn inside a selection (dark on the light selection fill).
    WAVE_SEL = 0x0a0c0c;
    /// Selection background: Audition inverts the selected range.
    SEL_FILL = 0x47d4a0;
    PLAYHEAD = 0xf2c230;
    CURSOR = 0xf2c230;
    MARKER = 0xe87a30;
    RECORD = 0xd8322c;
    WARN = 0xf0b43c;
    OVERVIEW_WAVE = 0xb8a046;
}

#[allow(non_snake_case)]
#[inline]
pub fn TIME() -> Color32 {
    HOT()
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("source_sans".into(), FontData::from_static(include_bytes!("../assets/fonts/SourceSans3-Regular.ttf")));
    fonts.font_data.insert("source_sans_semibold".into(), FontData::from_static(include_bytes!("../assets/fonts/SourceSans3-Semibold.ttf")));
    fonts.font_data.insert("source_code".into(), FontData::from_static(include_bytes!("../assets/fonts/SourceCodePro-Semibold.ttf")));
    // Prepend, keeping egui's bundled fonts as fallbacks for symbols.
    if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
        f.insert(0, "source_sans".into());
    }
    if let Some(f) = fonts.families.get_mut(&FontFamily::Monospace) {
        f.insert(0, "source_code".into());
    }
    let mut bold = vec!["source_sans_semibold".to_string()];
    bold.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(FontFamily::Name("bold".into()), bold);
    ctx.set_fonts(fonts);
}

pub fn bold(size: f32) -> egui::FontId {
    egui::FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    // Audemo is always dark-themed in egui's terms. egui otherwise follows
    // the OS theme and swaps in its stock light or dark style, leaving our
    // panels with the wrong text colours (an unreadable menu bar on Windows).
    ctx.set_theme(egui::ThemePreference::Dark);
    apply_visuals(ctx, 0);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(6.0, 3.0);
        s.spacing.button_padding = vec2(7.0, 2.0);
        s.spacing.slider_width = 190.0;
        s.spacing.interact_size.y = 19.0;
        use egui::{FontId, TextStyle};
        s.text_styles.insert(TextStyle::Body, FontId::proportional(13.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.0));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.0));
    });
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
}

/// Lighten (positive) or darken a grey by `off` levels.
fn shade(v: u32, off: i32) -> u32 {
    let ch = |x: u32| ((x as i32 + off).clamp(0x08, 0xf0)) as u32;
    ch(v >> 16 & 0xff) << 16 | ch(v >> 8 & 0xff) << 8 | ch(v & 0xff)
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = ((a >> s & 0xff) as f32, (b >> s & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// Grey offset for an Appearance brightness (30 = Audemo's default).
fn brightness_offset(b: f32) -> i32 {
    if b >= 30.0 {
        ((b - 30.0) / 70.0 * 150.0) as i32
    } else {
        ((b - 30.0) / 30.0 * 40.0) as i32
    }
}

static GRADIENTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Preferences > Appearance: "Use Gradients".
pub fn gradients() -> bool {
    GRADIENTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Apply Preferences > Appearance (and the tooltip setting) to the palette
/// and egui's style.
pub fn set_look(ctx: &egui::Context, p: &crate::prefs::Prefs) {
    let off = brightness_offset(p.brightness);
    for (name, base) in DEFAULT_PALETTE {
        if name.starts_with("BG_") || *name == "BORDER" {
            set(name, shade(*base, off));
        }
    }
    // Light interfaces get dark text.
    let light = 0x3a + off > 0x90;
    set("TEXT", if light { 0x1c1c1c } else { 0xd6d6d6 });
    set("TEXT_DIM", if light { 0x4e4e4e } else { 0x9a9a9a });
    let c = p.colors;
    set("WAVE", c[0]);
    // The dimmed waveform follows a custom waveform colour.
    let wave_default = crate::prefs::DEFAULT_COLORS[0];
    set("WAVE_DIM", if c[0] == wave_default { 0x2d6e55 } else { mix(c[0], 0x000000, 0.5) });
    set("SEL_FILL", c[1]);
    set("PLAYHEAD", c[2]);
    set("CURSOR", c[2]);
    set("MARKER", c[3]);
    set("HOT", c[4]);
    set("ACCENT", c[5]);
    GRADIENTS.store(p.gradients, std::sync::atomic::Ordering::Relaxed);
    apply_visuals(ctx, off);
    ctx.set_zoom_factor(p.ui_scale);
    let delay = if p.show_tooltips { 0.5 } else { f32::INFINITY };
    ctx.all_styles_mut(|s| s.interaction.tooltip_delay = delay);
}

fn apply_visuals(ctx: &egui::Context, off: i32) {
    let g = |v: u32| rgb(shade(v, off));
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG_PANEL();
    v.window_fill = BG_PANEL();
    v.extreme_bg_color = BG_LIST();
    v.faint_bg_color = g(0x373737);
    v.code_bg_color = BG_LIST();
    v.window_stroke = Stroke::new(1.0_f32, BORDER());
    v.window_rounding = Rounding::same(3.0);
    v.menu_rounding = Rounding::same(2.0);
    v.selection.bg_fill = g(0x5c5c5c);
    v.selection.stroke = Stroke::new(1.0_f32, if off > 60 { TEXT() } else { Color32::WHITE });
    v.hyperlink_color = HOT();
    v.override_text_color = Some(TEXT());
    v.widgets.noninteractive.bg_fill = BG_PANEL();
    v.widgets.noninteractive.weak_bg_fill = BG_PANEL();
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER());
    v.widgets.inactive.bg_fill = BG_RAISED();
    v.widgets.inactive.weak_bg_fill = BG_RAISED();
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, g(0x2a2a2a));
    v.widgets.hovered.bg_fill = g(0x565656);
    v.widgets.hovered.weak_bg_fill = g(0x565656);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, g(0x6a6a6a));
    v.widgets.active.bg_fill = g(0x626262);
    v.widgets.active.weak_bg_fill = g(0x626262);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.rounding = Rounding::same(2.0);
    }
    ctx.set_visuals_of(egui::Theme::Dark, v.clone());
    ctx.set_visuals_of(egui::Theme::Light, v);
}

/// Waveform lane background: flat, or a soft vertical gradient when
/// "Use Gradients" is on.
pub fn lane_fill(p: &egui::Painter, rect: Rect) {
    if !gradients() {
        p.rect_filled(rect, 0.0, LANE_BG());
        return;
    }
    let edge = LANE_BG();
    let mid = rgb(mix(cells::LANE_BG.load(std::sync::atomic::Ordering::Relaxed), 0x2a3640, 0.6));
    let mut mesh = egui::Mesh::default();
    let ys = [rect.top(), rect.center().y, rect.bottom()];
    let cs = [edge, mid, edge];
    for (y, c) in ys.iter().zip(cs) {
        mesh.colored_vertex(pos2(rect.left(), *y), c);
        mesh.colored_vertex(pos2(rect.right(), *y), c);
    }
    for k in 0..2u32 {
        let i = k * 2;
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 3, i + 2);
    }
    p.add(Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_and_colours() {
        assert_eq!(brightness_offset(30.0), 0);
        assert!(brightness_offset(100.0) > 140 && brightness_offset(0.0) < -30);
        assert_eq!(shade(0x3a3a3a, 0), 0x3a3a3a);
        assert_eq!(shade(0x101010, -100), 0x080808);
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
    }
}

/// Audition-style scrubbable value: orange text, drag to change, click to type.
pub fn hot_drag<'a>(ui: &mut Ui, dv: egui::DragValue<'a>) -> Response {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        for st in [&mut w.inactive, &mut w.hovered, &mut w.active] {
            st.weak_bg_fill = Color32::TRANSPARENT;
            st.bg_fill = Color32::TRANSPARENT;
            st.bg_stroke = Stroke::NONE;
            st.fg_stroke = Stroke::new(1.0_f32, HOT());
        }
        w.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xff, 0xc4, 0x6a));
        ui.add(dv)
    })
    .inner
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Record,
    ToStart,
    ToEnd,
    Rewind,
    FastForward,
    Loop,
    ZoomIn,
    ZoomOut,
    ZoomFull,
    ZoomSel,
    AmpIn,
    AmpOut,
    Spectral,
    Selection,
    Hand,
    Move,
    Razor,
    Slip,
    Marquee,
    Lasso,
    Brush,
    Healing,
    Waveform,
    Multitrack,
    Power,
    FolderOpen,
    NewFile,
    CloseFile,
    Trash,
    Menu,
}

/// Flat toolbar button: icon only, background on hover/active.
pub fn icon_button(ui: &mut Ui, icon: Icon, active: bool, tip: &str) -> Response {
    icon_button_sized(ui, icon, active, tip, vec2(24.0, 22.0), true)
}

pub fn icon_button_sized(ui: &mut Ui, icon: Icon, active: bool, tip: &str, size: egui::Vec2, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let painter = ui.painter_at(rect);
    let bg = if active {
        Some(Color32::from_rgb(0x26, 0x26, 0x26))
    } else if enabled && resp.is_pointer_button_down_on() {
        Some(Color32::from_rgb(0x2a, 0x2a, 0x2a))
    } else if enabled && resp.hovered() {
        Some(Color32::from_rgb(0x50, 0x50, 0x50))
    } else {
        None
    };
    if let Some(bg) = bg {
        painter.rect_filled(rect, 2.0, bg);
        if active {
            painter.rect_stroke(rect, 2.0, Stroke::new(1.0_f32, Color32::from_rgb(0x1a, 0x1a, 0x1a)));
        }
    }
    let fg = if !enabled {
        Color32::from_rgb(0x6a, 0x6a, 0x6a)
    } else if icon == Icon::Record {
        RECORD()
    } else if active {
        HOT()
    } else {
        // Light icons on dark panels; dark ones on light panels.
        let t = TEXT();
        if t.r() < 0x80 {
            t
        } else {
            Color32::from_rgb(0xd0, 0xd0, 0xd0)
        }
    };
    let pad = (rect.height() * 0.25).max(4.0);
    let inner = Rect::from_center_size(rect.center(), vec2(rect.height() - pad * 2.0, rect.height() - pad * 2.0));
    draw_icon(&painter, inner, icon, fg);
    resp.on_hover_text(tip)
}

fn tri(p: &egui::Painter, a: Pos2, b: Pos2, c: Pos2, col: Color32) {
    p.add(Shape::convex_polygon(vec![a, b, c], col, Stroke::NONE));
}

pub fn draw_icon(p: &egui::Painter, r: Rect, icon: Icon, col: Color32) {
    let s = Stroke::new(1.6_f32, col);
    let (l, t, rr, b) = (r.left(), r.top(), r.right(), r.bottom());
    let c = r.center();
    match icon {
        Icon::Play => tri(p, pos2(l + 2.0, t), pos2(rr, c.y), pos2(l + 2.0, b), col),
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(pos2(l + 2.0, t), pos2(c.x - 1.5, b)), 1.0, col);
            p.rect_filled(Rect::from_min_max(pos2(c.x + 1.5, t), pos2(rr - 2.0, b)), 1.0, col);
        }
        Icon::Stop => {
            p.rect_filled(r.shrink(1.0), 1.0, col);
        }
        Icon::Record => {
            p.circle_filled(c, r.height() * 0.48, col);
        }
        Icon::ToStart => {
            p.rect_filled(Rect::from_min_max(pos2(l, t), pos2(l + 2.0, b)), 0.0, col);
            tri(p, pos2(rr, t), pos2(l + 2.5, c.y), pos2(rr, b), col);
        }
        Icon::ToEnd => {
            p.rect_filled(Rect::from_min_max(pos2(rr - 2.0, t), pos2(rr, b)), 0.0, col);
            tri(p, pos2(l, t), pos2(rr - 2.5, c.y), pos2(l, b), col);
        }
        Icon::Loop => {
            let rad = r.height() * 0.42;
            let pts: Vec<Pos2> = (0..=26)
                .map(|i| {
                    let a = 0.6 + i as f32 / 26.0 * 5.2;
                    pos2(c.x + rad * a.cos() * 1.15, c.y + rad * a.sin())
                })
                .collect();
            let end = *pts.last().unwrap();
            p.add(Shape::line(pts, s));
            tri(p, pos2(end.x - 3.5, end.y - 3.0), pos2(end.x + 3.5, end.y - 1.0), pos2(end.x - 0.5, end.y + 3.5), col);
        }
        Icon::ZoomIn | Icon::ZoomOut | Icon::ZoomFull | Icon::ZoomSel => {
            let lens = pos2(l + r.width() * 0.42, t + r.height() * 0.42);
            let rad = r.width() * 0.36;
            p.circle_stroke(lens, rad, s);
            p.line_segment([pos2(lens.x + rad * 0.7, lens.y + rad * 0.7), pos2(rr, b)], Stroke::new(2.2_f32, col));
            match icon {
                Icon::ZoomIn => {
                    p.line_segment([pos2(lens.x - rad * 0.5, lens.y), pos2(lens.x + rad * 0.5, lens.y)], s);
                    p.line_segment([pos2(lens.x, lens.y - rad * 0.5), pos2(lens.x, lens.y + rad * 0.5)], s);
                }
                Icon::ZoomOut => {
                    p.line_segment([pos2(lens.x - rad * 0.5, lens.y), pos2(lens.x + rad * 0.5, lens.y)], s);
                }
                Icon::ZoomFull => {
                    p.rect_stroke(Rect::from_center_size(lens, vec2(rad, rad * 0.7)), 0.0, Stroke::new(1.0_f32, col));
                }
                _ => {
                    p.line_segment([pos2(lens.x - rad * 0.45, lens.y - rad * 0.5), pos2(lens.x - rad * 0.45, lens.y + rad * 0.5)], s);
                    p.line_segment([pos2(lens.x + rad * 0.45, lens.y - rad * 0.5), pos2(lens.x + rad * 0.45, lens.y + rad * 0.5)], s);
                }
            }
        }
        Icon::AmpIn | Icon::AmpOut => {
            p.line_segment([pos2(c.x, t), pos2(c.x, b)], s);
            let (y1, y2, dir) = if icon == Icon::AmpIn { (t, b, 1.0) } else { (c.y - 1.0, c.y + 1.0, -1.0) };
            tri(p, pos2(c.x - 4.0, y1 + 4.0 * dir), pos2(c.x + 4.0, y1 + 4.0 * dir), pos2(c.x, y1), col);
            tri(p, pos2(c.x - 4.0, y2 - 4.0 * dir), pos2(c.x + 4.0, y2 - 4.0 * dir), pos2(c.x, y2), col);
            p.line_segment([pos2(l, c.y), pos2(c.x - 4.0, c.y)], Stroke::new(1.0_f32, col));
            p.line_segment([pos2(c.x + 4.0, c.y), pos2(rr, c.y)], Stroke::new(1.0_f32, col));
        }
        Icon::Spectral => {
            let w = r.width() / 5.0;
            let heights = [0.5, 0.9, 0.65, 1.0, 0.4];
            let cols = [
                Color32::from_rgb(70, 30, 150),
                Color32::from_rgb(160, 30, 140),
                Color32::from_rgb(225, 60, 60),
                Color32::from_rgb(250, 160, 40),
                Color32::from_rgb(250, 240, 190),
            ];
            for i in 0..5 {
                let x = l + i as f32 * w;
                let h = r.height() * heights[i];
                p.rect_filled(Rect::from_min_max(pos2(x, b - h), pos2(x + w - 1.0, b)), 0.0, cols[i]);
            }
        }
        Icon::Selection => {
            p.line_segment([pos2(c.x, t), pos2(c.x, b)], s);
            p.line_segment([pos2(c.x - 3.5, t), pos2(c.x + 3.5, t)], s);
            p.line_segment([pos2(c.x - 3.5, b), pos2(c.x + 3.5, b)], s);
        }
        Icon::Hand => {
            for i in 0..4 {
                let x = l + 2.0 + i as f32 * 2.8;
                p.line_segment([pos2(x, t + 1.0 + (i % 2) as f32), pos2(x, c.y + 2.0)], s);
            }
            p.rect_filled(Rect::from_min_max(pos2(l + 1.0, c.y), pos2(rr - 2.0, b)), 3.0, col);
        }
        Icon::Rewind => {
            tri(p, pos2(c.x, t), pos2(l, c.y), pos2(c.x, b), col);
            tri(p, pos2(rr, t), pos2(c.x, c.y), pos2(rr, b), col);
        }
        Icon::FastForward => {
            tri(p, pos2(l, t), pos2(c.x, c.y), pos2(l, b), col);
            tri(p, pos2(c.x, t), pos2(rr, c.y), pos2(c.x, b), col);
        }
        Icon::Move => {
            tri(p, pos2(l + 1.0, t), pos2(l + 1.0, b - 2.0), pos2(rr - 1.0, c.y + 2.0), col);
            p.line_segment([pos2(c.x, c.y + 1.0), pos2(rr - 1.0, b)], Stroke::new(2.0_f32, col));
        }
        Icon::Razor => {
            p.add(Shape::convex_polygon(vec![pos2(l + 2.0, b), pos2(rr - 4.0, t), pos2(rr, t + 4.0), pos2(l + 6.0, b)], col, Stroke::NONE));
        }
        Icon::Slip => {
            p.line_segment([pos2(l, c.y), pos2(rr, c.y)], s);
            tri(p, pos2(l, c.y), pos2(l + 4.0, c.y - 3.5), pos2(l + 4.0, c.y + 3.5), col);
            tri(p, pos2(rr, c.y), pos2(rr - 4.0, c.y - 3.5), pos2(rr - 4.0, c.y + 3.5), col);
            p.line_segment([pos2(c.x, t), pos2(c.x, b)], Stroke::new(1.0_f32, col));
        }
        Icon::Marquee => {
            let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
            p.extend(Shape::dashed_line(&pts, Stroke::new(1.2_f32, col), 2.5, 2.0));
        }
        Icon::Lasso => {
            let pts: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let a = i as f32 / 24.0 * std::f32::consts::TAU;
                    pos2(c.x + r.width() * 0.45 * a.cos(), c.y - 1.5 + r.height() * 0.32 * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, s));
            p.line_segment([pos2(c.x - 1.0, c.y + r.height() * 0.3), pos2(c.x - 3.0, b)], s);
        }
        Icon::Brush => {
            p.line_segment([pos2(rr, t), pos2(c.x, c.y + 1.0)], Stroke::new(2.0_f32, col));
            p.circle_filled(pos2(c.x - 1.5, c.y + 3.0), 3.2, col);
        }
        Icon::Healing => {
            let rot = |x: f32, y: f32| {
                let a = -std::f32::consts::FRAC_PI_4;
                pos2(c.x + x * a.cos() - y * a.sin(), c.y + x * a.sin() + y * a.cos())
            };
            let w = r.width() * 0.55;
            let h = r.height() * 0.2;
            p.add(Shape::convex_polygon(vec![rot(-w, -h), rot(w, -h), rot(w, h), rot(-w, h)], col, Stroke::NONE));
        }
        Icon::Waveform | Icon::Multitrack => {
            let rows = if icon == Icon::Waveform { 1 } else { 3 };
            for row in 0..rows {
                let y0 = t + (row as f32 + 0.5) * r.height() / rows as f32;
                let amp = r.height() / rows as f32 * 0.45;
                for i in 0..7 {
                    let x = l + i as f32 * r.width() / 6.0;
                    let a = amp * [0.3, 0.9, 0.5, 1.0, 0.6, 0.8, 0.3][i];
                    p.line_segment([pos2(x, y0 - a), pos2(x, y0 + a)], Stroke::new(1.3_f32, col));
                }
            }
        }
        Icon::Power => {
            let rad = r.height() * 0.42;
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + 0.6 + i as f32 / 20.0 * (std::f32::consts::TAU - 1.2);
                    pos2(c.x + rad * a.cos(), c.y + rad * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, s));
            p.line_segment([pos2(c.x, t), pos2(c.x, c.y)], s);
        }
        Icon::FolderOpen => {
            p.add(Shape::convex_polygon(vec![pos2(l, t + 2.0), pos2(l + r.width() * 0.4, t + 2.0), pos2(l + r.width() * 0.5, t + 4.0), pos2(rr, t + 4.0), pos2(rr, b), pos2(l, b)], col, Stroke::NONE));
        }
        Icon::NewFile => {
            p.rect_stroke(Rect::from_min_max(pos2(l + 2.0, t), pos2(rr - 2.0, b)), 0.0, Stroke::new(1.2_f32, col));
            p.line_segment([pos2(c.x - 3.0, c.y), pos2(c.x + 3.0, c.y)], s);
            p.line_segment([pos2(c.x, c.y - 3.0), pos2(c.x, c.y + 3.0)], s);
        }
        Icon::CloseFile => {
            p.line_segment([pos2(l + 2.0, t + 2.0), pos2(rr - 2.0, b - 2.0)], Stroke::new(1.8_f32, col));
            p.line_segment([pos2(rr - 2.0, t + 2.0), pos2(l + 2.0, b - 2.0)], Stroke::new(1.8_f32, col));
        }
        Icon::Trash => {
            p.rect_filled(Rect::from_min_max(pos2(l + 2.0, t + 3.0), pos2(rr - 2.0, b)), 1.0, col);
            p.line_segment([pos2(l, t + 1.5), pos2(rr, t + 1.5)], s);
        }
        Icon::Menu => {
            for k in 0..3 {
                let y = t + 2.0 + k as f32 * (r.height() - 4.0) / 2.0;
                p.line_segment([pos2(l, y), pos2(rr, y)], Stroke::new(1.2_f32, col));
            }
        }
    }
}
