//! Audition (CS-era) style palette, bundled Adobe open-source fonts, and
//! hand-drawn flat icons (no icon font needed).

use eframe::egui::{self, pos2, vec2, Color32, FontData, FontDefinitions, FontFamily, Pos2, Rect, Response, Rounding, Sense, Shape, Stroke, Ui};

// Workspace greys.
pub const BG_DEEP: Color32 = Color32::from_rgb(0x26, 0x26, 0x26); // gutters between panels
pub const BG_PANEL: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a); // panel body
pub const BG_HEADER: Color32 = Color32::from_rgb(0x2c, 0x2c, 0x2c); // panel tab strip
pub const BG_LIST: Color32 = Color32::from_rgb(0x33, 0x33, 0x33); // list / table wells
pub const BG_RAISED: Color32 = Color32::from_rgb(0x4a, 0x4a, 0x4a); // buttons
pub const BORDER: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
pub const TEXT: Color32 = Color32::from_rgb(0xd6, 0xd6, 0xd6);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
/// Audition's orange-yellow "hot text" for times and scrubbable values.
pub const HOT: Color32 = Color32::from_rgb(0xe8, 0xa1, 0x3a);
pub const ACCENT: Color32 = Color32::from_rgb(0x2d, 0x7f, 0xd9);

// Editor.
pub const LANE_BG: Color32 = Color32::from_rgb(0x0f, 0x13, 0x17);
pub const LANE_GRID: Color32 = Color32::from_rgb(0x1f, 0x2a, 0x2a);
pub const WAVE: Color32 = Color32::from_rgb(0x3f, 0xdc, 0x9b);
pub const WAVE_DIM: Color32 = Color32::from_rgb(0x2d, 0x6e, 0x55);
/// Waveform drawn inside a selection (dark on the light selection fill).
pub const WAVE_SEL: Color32 = Color32::from_rgb(0x0a, 0x0c, 0x0c);
/// Selection background: Audition inverts the selected range.
pub const SEL_FILL: Color32 = Color32::from_rgb(0x47, 0xd4, 0xa0);
pub const PLAYHEAD: Color32 = Color32::from_rgb(0xf2, 0xc2, 0x30);
pub const CURSOR: Color32 = Color32::from_rgb(0xf2, 0xc2, 0x30);
pub const MARKER: Color32 = Color32::from_rgb(0xe8, 0x7a, 0x30);
pub const RECORD: Color32 = Color32::from_rgb(0xd8, 0x32, 0x2c);
pub const TIME: Color32 = HOT;
pub const WARN: Color32 = Color32::from_rgb(0xf0, 0xb4, 0x3c);
pub const OVERVIEW_WAVE: Color32 = Color32::from_rgb(0xb8, 0xa0, 0x46);

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
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG_PANEL;
    v.window_fill = BG_PANEL;
    v.extreme_bg_color = BG_LIST;
    v.faint_bg_color = Color32::from_rgb(0x37, 0x37, 0x37);
    v.code_bg_color = BG_LIST;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_rounding = Rounding::same(3.0);
    v.menu_rounding = Rounding::same(2.0);
    v.selection.bg_fill = Color32::from_rgb(0x5c, 0x5c, 0x5c);
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.hyperlink_color = HOT;
    v.override_text_color = Some(TEXT);
    v.widgets.noninteractive.bg_fill = BG_PANEL;
    v.widgets.noninteractive.weak_bg_fill = BG_PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.bg_fill = BG_RAISED;
    v.widgets.inactive.weak_bg_fill = BG_RAISED;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x2a, 0x2a, 0x2a));
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x56, 0x56, 0x56);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x56, 0x56, 0x56);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x6a, 0x6a, 0x6a));
    v.widgets.active.bg_fill = Color32::from_rgb(0x62, 0x62, 0x62);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x62, 0x62, 0x62);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.rounding = Rounding::same(2.0);
    }
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
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

/// Audition-style scrubbable value: orange text, drag to change, click to type.
pub fn hot_drag<'a>(ui: &mut Ui, dv: egui::DragValue<'a>) -> Response {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        for st in [&mut w.inactive, &mut w.hovered, &mut w.active] {
            st.weak_bg_fill = Color32::TRANSPARENT;
            st.bg_fill = Color32::TRANSPARENT;
            st.bg_stroke = Stroke::NONE;
            st.fg_stroke = Stroke::new(1.0_f32, HOT);
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
        RECORD
    } else if active {
        HOT
    } else {
        Color32::from_rgb(0xd0, 0xd0, 0xd0)
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
