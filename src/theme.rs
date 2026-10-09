//! Dark studio palette plus hand-drawn transport icons (no icon font needed).

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Response, Rounding, Sense, Shape, Stroke, Ui};

pub const BG_DEEP: Color32 = Color32::from_rgb(0x15, 0x17, 0x1c);
pub const BG_PANEL: Color32 = Color32::from_rgb(0x1f, 0x22, 0x28);
pub const BG_RAISED: Color32 = Color32::from_rgb(0x2a, 0x2e, 0x36);
pub const BORDER: Color32 = Color32::from_rgb(0x34, 0x39, 0x42);
pub const TEXT: Color32 = Color32::from_rgb(0xc9, 0xce, 0xd6);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x82, 0x89, 0x94);
pub const ACCENT: Color32 = Color32::from_rgb(0x3d, 0x9b, 0xff);

pub const LANE_BG: Color32 = Color32::from_rgb(0x0b, 0x10, 0x15);
pub const LANE_GRID: Color32 = Color32::from_rgb(0x1a, 0x24, 0x2c);
pub const WAVE: Color32 = Color32::from_rgb(0x2f, 0xd4, 0x8c);
pub const WAVE_DIM: Color32 = Color32::from_rgb(0x2a, 0x55, 0x45);
pub const WAVE_SEL: Color32 = Color32::from_rgb(0x8c, 0xf5, 0xc4);
pub const SEL_FILL: Color32 = Color32::from_rgba_premultiplied(0x34, 0x3c, 0x48, 0x70);
pub const PLAYHEAD: Color32 = Color32::from_rgb(0xff, 0x5a, 0x5a);
pub const CURSOR: Color32 = Color32::from_rgb(0xf2, 0xd3, 0x4b);
pub const MARKER: Color32 = Color32::from_rgb(0xe8, 0x9b, 0x3a);
pub const RECORD: Color32 = Color32::from_rgb(0xe8, 0x3e, 0x3e);
pub const TIME: Color32 = Color32::from_rgb(0x5c, 0xe6, 0xa8);
pub const WARN: Color32 = Color32::from_rgb(0xf0, 0xb4, 0x3c);

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG_PANEL;
    v.window_fill = BG_PANEL;
    v.extreme_bg_color = BG_DEEP;
    v.faint_bg_color = Color32::from_rgb(0x24, 0x28, 0x2f);
    v.code_bg_color = BG_DEEP;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_rounding = Rounding::same(4.0);
    v.menu_rounding = Rounding::same(3.0);
    v.selection.bg_fill = Color32::from_rgb(0x1f, 0x5f, 0xa8);
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.hyperlink_color = ACCENT;
    v.override_text_color = Some(TEXT);

    v.widgets.noninteractive.bg_fill = BG_PANEL;
    v.widgets.noninteractive.weak_bg_fill = BG_PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.bg_fill = BG_RAISED;
    v.widgets.inactive.weak_bg_fill = BG_RAISED;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x3a, 0x40, 0x4a));
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x36, 0x3c, 0x46);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x36, 0x3c, 0x46);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x55, 0x5e, 0x6c));
    v.widgets.active.bg_fill = Color32::from_rgb(0x25, 0x6a, 0xb8);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x25, 0x6a, 0xb8);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.rounding = Rounding::same(3.0);
    }
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(6.0, 4.0);
        s.spacing.button_padding = vec2(8.0, 3.0);
        s.spacing.slider_width = 210.0;
        s.spacing.interact_size.y = 20.0;
    });
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Record,
    ToStart,
    ToEnd,
    PrevMarker,
    NextMarker,
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
}

pub fn icon_button(ui: &mut Ui, icon: Icon, active: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 24.0), Sense::click());
    let painter = ui.painter_at(rect);
    let bg = if active {
        Color32::from_rgb(0x25, 0x5a, 0x9a)
    } else if resp.is_pointer_button_down_on() {
        Color32::from_rgb(0x40, 0x47, 0x52)
    } else if resp.hovered() {
        Color32::from_rgb(0x36, 0x3c, 0x46)
    } else {
        BG_RAISED
    };
    painter.rect_filled(rect, 3.0, bg);
    let fg = if icon == Icon::Record { RECORD } else if active { Color32::WHITE } else { TEXT };
    draw_icon(&painter, rect.shrink(6.0), icon, fg);
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
        Icon::PrevMarker => {
            tri(p, pos2(c.x, t), pos2(l, c.y), pos2(c.x, b), col);
            tri(p, pos2(rr, t), pos2(c.x, c.y), pos2(rr, b), col);
        }
        Icon::NextMarker => {
            tri(p, pos2(l, t), pos2(c.x, c.y), pos2(l, b), col);
            tri(p, pos2(c.x, t), pos2(rr, c.y), pos2(c.x, b), col);
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
    }
}
