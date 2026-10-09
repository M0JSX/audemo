//! The waveform editor: overview strip, time ruler, waveform lanes, spectral
//! display, selection, markers, fade handles and the clip gain HUD.

use eframe::egui::{self, pos2, vec2, Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui};

use crate::app::{Action, App, SpecTex, Tool};
use crate::dsp::spectrogram::{heat_colour, spectrogram_range};
use crate::dsp::util::format_time;
use crate::theme::*;

const OVERVIEW_H: f32 = 20.0;
const RULER_H: f32 = 24.0;
const LEFT_W: f32 = 24.0;
const RIGHT_W: f32 = 46.0;
const SPEC_FFT: usize = 1024;

const RULER_STEPS: [f64; 22] = [
    0.0005, 0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0,
    600.0, 1800.0,
];

#[derive(Clone, Copy)]
struct View {
    left: f32,
    width: f32,
    start: f64,
    end: f64,
}

impl View {
    fn x(&self, s: f64) -> f32 {
        self.left + ((s - self.start) / (self.end - self.start)) as f32 * self.width
    }
    fn s(&self, x: f32) -> f64 {
        self.start + ((x - self.left) / self.width) as f64 * (self.end - self.start)
    }
    fn spp(&self) -> f64 {
        (self.end - self.start) / self.width as f64
    }
}

fn ruler_label(t: f64, step: f64) -> String {
    let neg = t < 0.0;
    let t = t.abs();
    let m = (t / 60.0).floor() as u64;
    let s = t - m as f64 * 60.0;
    let body = if step < 0.01 {
        format!("{m}:{s:06.3}")
    } else if step < 1.0 {
        format!("{m}:{s:04.1}")
    } else {
        format!("{m}:{:02}", s.round() as u64)
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

impl App {
    pub fn editor_ui(&mut self, ui: &mut Ui) {
        let Some(di) = self.active.filter(|&i| i < self.docs.len()) else {
            self.welcome_ui(ui);
            return;
        };
        self.editor_tabs(ui);
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        if full.width() < 120.0 || full.height() < 80.0 {
            return;
        }
        let painter = ui.painter_at(full);
        let st = self.engine.status();
        let show_spec = self.show_spectral;
        let tool = self.tool;

        let overview = Rect::from_min_size(full.min, vec2(full.width(), OVERVIEW_H));
        let ruler = Rect::from_min_size(pos2(full.left(), overview.bottom()), vec2(full.width(), RULER_H));
        let body = Rect::from_min_max(pos2(full.left(), ruler.bottom() + 1.0), full.max);
        let lanes = Rect::from_min_max(pos2(body.left() + LEFT_W, body.top()), pos2(body.right() - RIGHT_W, body.bottom()));

        let doc = &mut self.docs[di];
        let len = doc.len();
        let n_ch = doc.n_ch().max(1);
        let sr = doc.sample_rate as f64;

        // ------------------------------------------------ overview strip
        painter.rect_filled(overview, 0.0, BG_DEEP);
        {
            let total = doc.max_span();
            let cols = overview.width().max(1.0) as usize;
            let mid = overview.center().y;
            let h = overview.height() * 0.45;
            let mut shapes = Vec::with_capacity(cols);
            for px in 0..cols {
                let a = (px as f64 / cols as f64 * total) as usize;
                let b = (((px + 1) as f64 / cols as f64 * total) as usize).max(a + 1);
                if a >= len {
                    break;
                }
                let mut amp = 0.0f32;
                for c in 0..doc.n_ch() {
                    let (lo, hi) = doc.peaks.min_max(&doc.audio, c, a, b);
                    amp = amp.max(hi.abs()).max(lo.abs());
                }
                let x = overview.left() + px as f32 + 0.5;
                shapes.push(Shape::line_segment([pos2(x, mid - amp * h), pos2(x, mid + amp * h + 1.0)], Stroke::new(1.0_f32, WAVE_DIM)));
            }
            painter.extend(shapes);
            let ox = |s: f64| overview.left() + (s / total) as f32 * overview.width();
            if let Some((a, b)) = doc.sel_range() {
                painter.rect_filled(Rect::from_x_y_ranges(ox(a as f64)..=ox(b as f64).max(ox(a as f64) + 1.0), overview.y_range()), 0.0, Color32::from_white_alpha(28));
            }
            let vr = Rect::from_x_y_ranges(ox(doc.view_start)..=ox(doc.view_end).max(ox(doc.view_start) + 3.0), overview.y_range());
            painter.rect_filled(vr, 0.0, Color32::from_rgba_unmultiplied(61, 155, 255, 30));
            painter.rect_stroke(vr.shrink(0.5), 0.0, Stroke::new(1.0_f32, ACCENT));
            if st.playing && st.tag == doc.id {
                let x = ox(st.pos);
                painter.line_segment([pos2(x, overview.top()), pos2(x, overview.bottom())], Stroke::new(1.0_f32, PLAYHEAD));
            }
            let resp = ui.interact(overview, ui.id().with(("overview", doc.id)), Sense::click_and_drag());
            if resp.is_pointer_button_down_on() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let centre = ((p.x - overview.left()) / overview.width()) as f64 * total;
                    let span = doc.view_end - doc.view_start;
                    doc.view_start = centre - span / 2.0;
                    doc.view_end = doc.view_start + span;
                    doc.clamp_view();
                }
            }
            if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
        }

        let v = View { left: lanes.left(), width: lanes.width(), start: doc.view_start, end: doc.view_end };

        // ------------------------------------------------ ruler
        painter.rect_filled(ruler, 0.0, BG_RAISED);
        painter.line_segment([pos2(ruler.left(), ruler.bottom()), pos2(ruler.right(), ruler.bottom())], Stroke::new(1.0_f32, BORDER));
        {
            let px_per_s = v.width as f64 / ((v.end - v.start) / sr);
            let step = *RULER_STEPS.iter().find(|&&s| s * px_per_s >= 84.0).unwrap_or(&3600.0);
            let first = (v.start / sr / step).floor() as i64;
            let last = (v.end / sr / step).ceil() as i64;
            let font = FontId::monospace(10.5);
            for i in first..=last {
                let t = i as f64 * step;
                let x = v.x(t * sr);
                if x >= lanes.left() - 0.5 && x <= lanes.right() + 0.5 {
                    painter.line_segment([pos2(x, ruler.bottom() - 9.0), pos2(x, ruler.bottom())], Stroke::new(1.0_f32, TEXT_DIM));
                    painter.text(pos2(x + 3.0, ruler.top() + 3.0), Align2::LEFT_TOP, ruler_label(t, step), font.clone(), TEXT_DIM);
                }
                for k in 1..5 {
                    let xm = v.x((t + step * k as f64 / 5.0) * sr);
                    if xm >= lanes.left() && xm <= lanes.right() {
                        painter.line_segment([pos2(xm, ruler.bottom() - 4.0), pos2(xm, ruler.bottom())], Stroke::new(1.0_f32, BORDER));
                    }
                }
            }
            let resp = ui.interact(ruler, ui.id().with(("ruler", doc.id)), Sense::click_and_drag());
            if resp.clicked() || resp.dragged() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let s = v.s(p.x.clamp(lanes.left(), lanes.right())).round().clamp(0.0, len as f64) as usize;
                    doc.cursor = s;
                    if resp.clicked() || resp.drag_stopped() {
                        self.actions.push(Action::SetCursor(s));
                    }
                }
            }
        }

        // ------------------------------------------------ lanes layout
        let (wave_area, spec_area) = if show_spec {
            let split = lanes.top() + lanes.height() * 0.42;
            (
                Rect::from_min_max(lanes.min, pos2(lanes.right(), split - 2.0)),
                Some(Rect::from_min_max(pos2(lanes.left(), split + 2.0), lanes.max)),
            )
        } else {
            (lanes, None)
        };
        let lane_rect = |area: Rect, c: usize| {
            let h = area.height() / n_ch as f32;
            Rect::from_min_size(pos2(area.left(), area.top() + h * c as f32), vec2(area.width(), h - if c + 1 < n_ch { 2.0 } else { 0.0 }))
        };
        painter.rect_filled(Rect::from_min_max(body.min, body.max), 0.0, BG_DEEP);
        let sel = doc.sel_range();

        for c in 0..n_ch {
            let r = lane_rect(wave_area, c);
            let active = doc.active_ch.get(c).copied().unwrap_or(true);
            painter.rect_filled(r, 0.0, LANE_BG);
            // Selection behind the waveform.
            if let Some((a, b)) = sel {
                let x0 = v.x(a as f64).max(r.left());
                let x1 = v.x(b as f64).min(r.right()).max(x0 + 1.0);
                if x1 > r.left() && x0 < r.right() {
                    painter.rect_filled(Rect::from_x_y_ranges(x0..=x1, r.y_range()), 0.0, if active { SEL_FILL } else { Color32::from_white_alpha(10) });
                }
            }
            draw_grid(&painter, r, doc.amp_zoom);
            draw_wave(&painter, r, &doc.audio[c], &doc.peaks, c, &v, sel, active, doc.amp_zoom);
            // Beyond end of file.
            let xe = v.x(len as f64);
            if xe < r.right() {
                painter.rect_filled(Rect::from_x_y_ranges(xe.max(r.left())..=r.right(), r.y_range()), 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 90));
            }
            draw_amp_ruler(&painter, r, lanes.right(), doc.amp_zoom);
            // Channel toggle.
            let label = match (n_ch, c) {
                (1, _) => "M",
                (_, 0) => "L",
                (_, 1) => "R",
                _ => "·",
            };
            let tr = Rect::from_min_size(pos2(body.left() + 3.0, r.top() + 4.0), vec2(LEFT_W - 6.0, 18.0));
            let tresp = ui.interact(tr, ui.id().with(("chan", doc.id, c)), Sense::click());
            painter.rect_filled(tr, 3.0, if active { Color32::from_rgb(0x1d, 0x5a, 0x40) } else { BG_RAISED });
            painter.text(tr.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.0), if active { WAVE_SEL } else { TEXT_DIM });
            let tresp = tresp.on_hover_text("Enable / disable this channel for editing");
            if tresp.clicked() && c < doc.active_ch.len() {
                doc.active_ch[c] = !doc.active_ch[c];
                if doc.active_ch.iter().all(|a| !a) {
                    doc.active_ch[c] = true;
                }
            }
        }

        // ------------------------------------------------ spectral display
        if let Some(area) = spec_area {
            if doc.spec.len() < n_ch {
                doc.spec.resize_with(n_ch, || None);
            }
            let nyq = sr / 2.0;
            for c in 0..n_ch {
                let r = lane_rect(area, c);
                let cols = (r.width() as usize).clamp(16, 2400);
                let key = (doc.version, v.start as i64, v.end as i64, cols);
                let stale = doc.spec[c].as_ref().map(|t| t.key != key).unwrap_or(true);
                if stale {
                    let bins = SPEC_FFT / 2;
                    let data = spectrogram_range(&doc.audio[c], v.start, v.end, cols, SPEC_FFT);
                    let mut rgba = vec![0u8; cols * bins * 4];
                    for x in 0..cols {
                        for b in 0..bins {
                            let col = heat_colour(data[x * bins + b], -115.0);
                            let row = bins - 1 - b;
                            let i = (row * cols + x) * 4;
                            rgba[i] = col[0];
                            rgba[i + 1] = col[1];
                            rgba[i + 2] = col[2];
                            rgba[i + 3] = 255;
                        }
                    }
                    let img = egui::ColorImage::from_rgba_unmultiplied([cols, bins], &rgba);
                    let tex = ui.ctx().load_texture(format!("spec-{}-{}", doc.id, c), img, egui::TextureOptions::LINEAR);
                    doc.spec[c] = Some(SpecTex { key, tex });
                }
                if let Some(t) = &doc.spec[c] {
                    painter.image(t.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                if let Some((a, b)) = sel {
                    let x0 = v.x(a as f64).max(r.left());
                    let x1 = v.x(b as f64).min(r.right()).max(x0 + 1.0);
                    if x1 > r.left() && x0 < r.right() {
                        painter.rect_filled(Rect::from_x_y_ranges(x0..=x1, r.y_range()), 0.0, Color32::from_white_alpha(34));
                    }
                }
                // Frequency ruler (kHz).
                let step_hz = [500.0, 1000.0, 2000.0, 5000.0, 10000.0]
                    .into_iter()
                    .find(|s| r.height() as f64 * s / nyq >= 22.0)
                    .unwrap_or(10000.0);
                let mut f = step_hz;
                while f < nyq {
                    let y = r.bottom() - (f / nyq) as f32 * r.height();
                    painter.line_segment([pos2(lanes.right(), y), pos2(lanes.right() + 5.0, y)], Stroke::new(1.0_f32, TEXT_DIM));
                    let label = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
                    painter.text(pos2(lanes.right() + 8.0, y), Align2::LEFT_CENTER, label, FontId::monospace(9.5), TEXT_DIM);
                    f += step_hz;
                }
                painter.text(pos2(lanes.right() + 8.0, r.top() + 6.0), Align2::LEFT_CENTER, "Hz", FontId::monospace(9.5), TEXT_DIM);
            }
        }

        // ------------------------------------------------ live recording
        if let (Some(t), Some((in_rate, _))) = (&self.rec_target, self.engine.recording_format()) {
            if t.doc_id == doc.id {
                let a = t.range.0 as f64;
                let head = a + doc.pending as f64;
                let bs = crate::engine::REC_BLOCK as f64 * sr / in_rate as f64;
                let x0 = v.x(a).max(lanes.left());
                let x1 = v.x(head).min(lanes.right());
                for c in 0..n_ch {
                    let r = lane_rect(wave_area, c);
                    if x1 > x0 {
                        painter.rect_filled(Rect::from_x_y_ranges(x0..=x1, r.y_range()), 0.0, LANE_BG);
                    }
                    draw_grid(&painter, Rect::from_x_y_ranges(x0..=x1.max(x0), r.y_range()), doc.amp_zoom);
                }
                let amp_zoom = doc.amp_zoom;
                self.engine.with_rec_blocks(|blocks| {
                    let mut shapes = Vec::new();
                    let cols = (x1 - x0).max(0.0) as usize;
                    for c in 0..n_ch {
                        let r = lane_rect(wave_area, c);
                        let mid = r.center().y;
                        let amp = (r.height() * 0.5 - 1.0) * amp_zoom;
                        let bc = c.min(1);
                        for i in 0..cols {
                            let x = x0 + i as f32;
                            let s0 = (v.s(x) - a) / bs;
                            let s1 = (v.s(x + 1.0) - a) / bs;
                            let b0 = s0.floor().max(0.0) as usize;
                            let b1 = (s1.ceil() as usize).max(b0 + 1).min(blocks.len());
                            if b0 >= b1 {
                                continue;
                            }
                            let (lo, hi) = blocks[b0..b1].iter().fold((f32::MAX, f32::MIN), |(lo, hi), b| (lo.min(b[bc].0), hi.max(b[bc].1)));
                            let y0 = (mid - hi * amp).clamp(r.top(), r.bottom());
                            let y1 = (mid - lo * amp).clamp(r.top(), r.bottom()).max(y0 + 1.0);
                            shapes.push(Shape::line_segment([pos2(x + 0.5, y0), pos2(x + 0.5, y1)], Stroke::new(1.0_f32, WAVE)));
                        }
                    }
                    painter.extend(shapes);
                });
                let hx = v.x(head);
                if hx >= lanes.left() && hx <= lanes.right() {
                    painter.line_segment([pos2(hx, ruler.top()), pos2(hx, lanes.bottom())], Stroke::new(1.5_f32, RECORD));
                    painter.add(Shape::convex_polygon(vec![pos2(hx - 6.0, ruler.top()), pos2(hx + 6.0, ruler.top()), pos2(hx, ruler.top() + 9.0)], RECORD, Stroke::NONE));
                }
            }
        }

        // ------------------------------------------------ markers, cursor, playhead
        for (mi, m) in doc.markers.iter().enumerate() {
            let x = v.x(m.pos as f64);
            if x < lanes.left() || x > lanes.right() {
                continue;
            }
            let pts = [pos2(x, lanes.top()), pos2(x, lanes.bottom())];
            painter.extend(Shape::dashed_line(&pts, Stroke::new(1.0_f32, MARKER), 4.0, 3.0));
            let flag = Rect::from_min_size(pos2(x, ruler.bottom() - 11.0), vec2(9.0, 10.0));
            painter.add(Shape::convex_polygon(vec![flag.left_top(), flag.right_top(), pos2(flag.right(), flag.bottom() - 3.0), flag.left_bottom()], MARKER, Stroke::NONE));
            let fr = ui.interact(flag.expand(2.0), ui.id().with(("marker", doc.id, mi)), Sense::click());
            if fr.clicked() {
                self.actions.push(Action::SetCursor(m.pos));
            }
            fr.on_hover_text(format!("{}  {}", m.name, format_time(m.pos as f64, doc.sample_rate)));
        }
        let cx = v.x(doc.cursor as f64);
        if cx >= lanes.left() && cx <= lanes.right() {
            painter.line_segment([pos2(cx, lanes.top()), pos2(cx, lanes.bottom())], Stroke::new(1.0_f32, CURSOR));
            painter.add(Shape::convex_polygon(vec![pos2(cx - 5.0, ruler.top()), pos2(cx + 5.0, ruler.top()), pos2(cx, ruler.top() + 7.0)], CURSOR, Stroke::NONE));
        }
        if st.playing && st.tag == doc.id {
            let px = v.x(st.pos);
            if px >= lanes.left() && px <= lanes.right() {
                painter.line_segment([pos2(px, ruler.top()), pos2(px, lanes.bottom())], Stroke::new(1.5_f32, PLAYHEAD));
                painter.add(Shape::convex_polygon(vec![pos2(px - 6.0, ruler.top()), pos2(px + 6.0, ruler.top()), pos2(px, ruler.top() + 9.0)], PLAYHEAD, Stroke::NONE));
            }
        }

        // ------------------------------------------------ lane interaction
        let resp = ui.interact(lanes, ui.id().with(("lanes", doc.id)), Sense::click_and_drag());
        let mods = ui.input(|i| i.modifiers);
        let middle = ui.input(|i| i.pointer.middle_down());
        let to_sample = |x: f32| v.s(x).round().clamp(0.0, len as f64) as usize;
        if resp.hovered() {
            ui.ctx().set_cursor_icon(if tool == Tool::Hand || self.hand_anchor.is_some() { CursorIcon::Grab } else { CursorIcon::Text });
        }
        if resp.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(lanes.center());
            if tool == Tool::Hand || middle {
                self.hand_anchor = Some((origin.x, doc.view_start, doc.view_end));
            } else {
                let s0 = to_sample(origin.x);
                let anchor = if mods.shift {
                    match doc.sel_range() {
                        Some((a, b)) => {
                            if s0.abs_diff(a) > s0.abs_diff(b) {
                                a
                            } else {
                                b
                            }
                        }
                        None => doc.cursor,
                    }
                } else {
                    s0
                };
                self.drag_anchor = Some(anchor);
            }
        }
        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some((x0, vs, ve)) = self.hand_anchor {
                    let spp = (ve - vs) / v.width as f64;
                    let dx = (p.x - x0) as f64 * spp;
                    doc.view_start = vs - dx;
                    doc.view_end = ve - dx;
                    doc.clamp_view();
                    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                } else if let Some(anchor) = self.drag_anchor {
                    // Auto-scroll when dragging past the edges.
                    let spp = v.spp();
                    if p.x > lanes.right() {
                        let d = ((p.x - lanes.right()) as f64 * spp * 0.4).max(1.0);
                        doc.view_start += d;
                        doc.view_end += d;
                        doc.clamp_view();
                    } else if p.x < lanes.left() {
                        let d = ((lanes.left() - p.x) as f64 * spp * 0.4).max(1.0);
                        doc.view_start -= d;
                        doc.view_end -= d;
                        doc.clamp_view();
                    }
                    let s = to_sample(p.x);
                    doc.sel = if s != anchor { Some((anchor.min(s), anchor.max(s))) } else { None };
                    doc.cursor = anchor.min(s);
                }
            }
        }
        if resp.drag_stopped() {
            self.drag_anchor = None;
            self.hand_anchor = None;
        }
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let s = to_sample(p.x);
                if mods.shift {
                    let (a, b) = match doc.sel_range() {
                        Some((a, b)) => {
                            if s < a {
                                (s, b)
                            } else {
                                (a, s.max(a))
                            }
                        }
                        None => (doc.cursor.min(s), doc.cursor.max(s)),
                    };
                    doc.sel = if b > a { Some((a, b)) } else { None };
                } else if tool == Tool::Selection {
                    doc.sel = None;
                    self.actions.push(Action::SetCursor(s));
                }
            }
        }
        if resp.double_clicked() {
            let a = v.start.max(0.0) as usize;
            let b = (v.end as usize).min(len);
            if b > a {
                doc.sel = Some((a, b));
            }
        }
        if resp.hovered() {
            let (scroll, hover) = ui.input(|i| (i.raw_scroll_delta, i.pointer.hover_pos()));
            if scroll != egui::Vec2::ZERO {
                if mods.alt {
                    doc.amp_zoom = (doc.amp_zoom * (1.0 + scroll.y * 0.004)).clamp(1.0, 64.0);
                } else if mods.shift || scroll.x.abs() > scroll.y.abs() {
                    let d = if scroll.x != 0.0 { scroll.x } else { scroll.y };
                    let shift = -(d as f64) * v.spp() * 1.5;
                    doc.view_start += shift;
                    doc.view_end += shift;
                    doc.clamp_view();
                } else {
                    let f = 0.85f64.powf(scroll.y as f64 / 40.0);
                    let anchor = hover.map(|h| v.s(h.x)).unwrap_or((v.start + v.end) / 2.0);
                    doc.zoom(f, anchor);
                }
            }
        }

        // ------------------------------------------------ fade handles
        let top_lane = lane_rect(wave_area, 0);
        let wave_rects: Vec<Rect> = (0..n_ch).map(|c| lane_rect(wave_area, c)).collect();
        for fade_in in [true, false] {
            let current = match self.fade_drag {
                Some((fi, l)) if fi == fade_in => l,
                _ => 0,
            };
            let edge = if fade_in { 0.0 } else { len as f64 };
            let ex = v.x(edge);
            if len < 4 || ex < lanes.left() - 1.0 || ex > lanes.right() + 1.0 {
                continue;
            }
            let hx = if fade_in { v.x(current as f64) } else { v.x((len - current.min(len)) as f64) };
            let hx = hx.clamp(lanes.left() + 6.0, lanes.right() - 6.0);
            let hr = Rect::from_center_size(pos2(hx, top_lane.top() + 9.0), vec2(11.0, 11.0));
            let hresp = ui.interact(hr.expand(3.0), ui.id().with(("fade", doc.id, fade_in)), Sense::drag());
            let hot = hresp.hovered() || hresp.dragged();
            painter.rect_filled(hr, 2.0, if hot { ACCENT } else { Color32::from_rgb(0x9a, 0xa3, 0xb0) });
            painter.rect_stroke(hr, 2.0, Stroke::new(1.0_f32, Color32::from_rgb(0x20, 0x24, 0x2a)));
            if hot {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            let hresp = hresp.on_hover_text(if fade_in { "Drag to fade in" } else { "Drag to fade out" });
            if hresp.dragged() {
                if let Some(p) = hresp.interact_pointer_pos() {
                    let s = to_sample(p.x);
                    let l = if fade_in { s } else { len - s.min(len) };
                    self.fade_drag = Some((fade_in, l));
                }
            }
            if hresp.drag_stopped() {
                if let Some((fi, l)) = self.fade_drag.take() {
                    if l > 1 {
                        self.actions.push(Action::ApplyFade { fade_in: fi, len: l });
                    }
                }
            }
            if let Some((fi, l)) = self.fade_drag {
                if fi == fade_in && l > 1 {
                    let (a, b) = if fade_in { (0.0, l as f64) } else { (len as f64 - l as f64, len as f64) };
                    for r in &wave_rects {
                        let pts: Vec<Pos2> = (0..=48)
                            .map(|k| {
                                let t = k as f32 / 48.0;
                                let g = 0.5 - 0.5 * (std::f32::consts::PI * t).cos();
                                let g = if fade_in { g } else { 1.0 - g };
                                pos2(v.x(a + (b - a) * t as f64), r.bottom() - g * r.height())
                            })
                            .collect();
                        painter.add(Shape::line(pts, Stroke::new(1.5_f32, ACCENT)));
                    }
                    painter.text(
                        pos2(hx, top_lane.top() + 20.0),
                        if fade_in { Align2::LEFT_TOP } else { Align2::RIGHT_TOP },
                        format!(" {} ", format_time(l as f64, doc.sample_rate)),
                        FontId::monospace(10.0),
                        Color32::WHITE,
                    );
                }
            }
        }

        // ------------------------------------------------ clip gain HUD
        let hud = Rect::from_center_size(pos2(top_lane.center().x, top_lane.top() + 16.0), vec2(118.0, 22.0));
        if top_lane.height() > 60.0 {
            painter.rect_filled(hud, 11.0, Color32::from_rgba_unmultiplied(20, 24, 30, 210));
            painter.rect_stroke(hud, 11.0, Stroke::new(1.0_f32, BORDER));
            painter.circle_stroke(pos2(hud.left() + 13.0, hud.center().y), 6.0, Stroke::new(1.5_f32, TEXT_DIM));
            let angle = -std::f32::consts::FRAC_PI_2 + self.hud_gain / 40.0 * 2.4;
            let c = pos2(hud.left() + 13.0, hud.center().y);
            painter.line_segment([c, c + vec2(angle.cos(), angle.sin()) * 6.0], Stroke::new(1.5_f32, ACCENT));
            let dv_rect = Rect::from_min_max(pos2(hud.left() + 24.0, hud.top() + 2.0), pos2(hud.right() - 6.0, hud.bottom() - 2.0));
            let r = ui.put(
                dv_rect,
                egui::DragValue::new(&mut self.hud_gain).speed(0.1).range(-40.0..=40.0).fixed_decimals(1).suffix(" dB"),
            );
            let r = r.on_hover_text("Drag to change the gain of the selection (or whole file)");
            if (r.drag_stopped() || r.lost_focus()) && self.hud_gain.abs() > 0.05 {
                self.actions.push(Action::ApplyGain(self.hud_gain));
                self.hud_gain = 0.0;
            }
        }

        // Channel separator line between waveform and spectral views.
        if let Some(area) = spec_area {
            painter.line_segment([pos2(body.left(), area.top() - 2.0), pos2(body.right(), area.top() - 2.0)], Stroke::new(1.0_f32, BORDER));
        }
    }

    fn editor_tabs(&mut self, ui: &mut Ui) {
        let mut switch = None;
        let mut close = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (i, d) in self.docs.iter().enumerate() {
                let active = Some(i) == self.active;
                let text = egui::RichText::new(format!("Editor: {}", d.display_name())).size(12.0);
                let text = if active { text.color(Color32::WHITE) } else { text.color(TEXT_DIM) };
                if ui.selectable_label(active, text).clicked() {
                    switch = Some(i);
                }
                if ui.small_button("×").on_hover_text("Close file").clicked() {
                    close = Some(i);
                }
                ui.add_space(8.0);
            }
        });
        if let Some(i) = switch {
            self.active = Some(i);
        }
        if let Some(i) = close {
            self.actions.push(Action::Close(i));
        }
    }

    fn welcome_ui(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, 0.0, BG_DEEP);
        ui.allocate_ui_at_rect(rect.shrink(40.0), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((rect.height() * 0.22).max(10.0));
                ui.label(egui::RichText::new("Audemo").size(34.0).color(Color32::WHITE).strong());
                ui.label(egui::RichText::new("Waveform audio editor").size(14.0).color(TEXT_DIM));
                ui.add_space(22.0);
                ui.horizontal(|ui| {
                    let w = 3.0 * 120.0 + 2.0 * 8.0;
                    ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                    if ui.add_sized([120.0, 30.0], egui::Button::new("Open File…")).clicked() {
                        self.actions.push(Action::Open);
                    }
                    if ui.add_sized([120.0, 30.0], egui::Button::new("New File…")).clicked() {
                        self.actions.push(Action::New);
                    }
                    if ui.add_sized([120.0, 30.0], egui::Button::new("Record")).clicked() {
                        self.actions.push(Action::Record);
                    }
                });
                ui.add_space(18.0);
                ui.label(egui::RichText::new("Drop audio files anywhere in this window.").color(TEXT_DIM));
                ui.label(egui::RichText::new("WAV · AIFF · FLAC · MP3 · OGG · M4A/AAC · ALAC").color(TEXT_DIM).size(11.0));
            });
        });
    }
}

fn draw_grid(p: &egui::Painter, r: Rect, amp_zoom: f32) {
    let mid = r.center().y;
    let half = r.height() * 0.5 - 1.0;
    p.line_segment([pos2(r.left(), mid), pos2(r.right(), mid)], Stroke::new(1.0_f32, LANE_GRID));
    for lin in [0.5f32, 0.25] {
        let dy = lin * half * amp_zoom;
        if dy < half {
            for y in [mid - dy, mid + dy] {
                p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], Stroke::new(1.0_f32, Color32::from_rgb(0x13, 0x1b, 0x22)));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_wave(
    p: &egui::Painter,
    r: Rect,
    ch: &[f32],
    peaks: &crate::dsp::peaks::PeakCache,
    c: usize,
    v: &View,
    sel: Option<(usize, usize)>,
    active: bool,
    amp_zoom: f32,
) {
    let mid = r.center().y;
    let amp = (r.height() * 0.5 - 1.0) * amp_zoom;
    let (col, col_sel) = if active { (WAVE, WAVE_SEL) } else { (WAVE_DIM, WAVE_DIM) };
    let len = ch.len();
    if len == 0 {
        return;
    }
    let y = |val: f32| (mid - val * amp).clamp(r.top(), r.bottom());
    let spp = v.spp();
    // `peaks.min_max` wants the full channel list; build a one-channel view.
    let single = PeakView { ch, peaks, c };
    if spp >= 1.0 {
        let cols = r.width().ceil() as usize;
        let mut shapes = Vec::with_capacity(cols);
        for i in 0..cols {
            let x = r.left() + i as f32;
            let s0 = v.s(x);
            let s1 = s0 + spp;
            if s1 <= 0.0 || s0 >= len as f64 {
                continue;
            }
            let a = s0.max(0.0) as usize;
            let b = (s1.ceil() as usize).min(len).max(a + 1);
            let (lo, hi) = single.min_max(a, b);
            let in_sel = sel.map(|(sa, sb)| a >= sa && a < sb).unwrap_or(false);
            let y0 = y(hi);
            let y1 = y(lo).max(y0 + 1.0);
            shapes.push(Shape::line_segment([pos2(x + 0.5, y0), pos2(x + 0.5, y1)], Stroke::new(1.0_f32, if in_sel { col_sel } else { col })));
        }
        p.extend(shapes);
    } else {
        let a = v.start.floor().max(0.0) as usize;
        let b = ((v.end.ceil() as usize) + 1).min(len);
        if b > a {
            let pts: Vec<Pos2> = (a..b).map(|i| pos2(v.x(i as f64), y(ch[i]))).collect();
            if spp < 0.12 {
                for pt in &pts {
                    p.line_segment([pos2(pt.x, mid), *pt], Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(47, 212, 140, 60)));
                    p.circle_filled(*pt, 2.4, col);
                }
            }
            p.add(Shape::line(pts, Stroke::new(1.3_f32, col)));
        }
    }
}

struct PeakView<'a> {
    ch: &'a [f32],
    peaks: &'a crate::dsp::peaks::PeakCache,
    c: usize,
}

impl PeakView<'_> {
    fn min_max(&self, a: usize, b: usize) -> (f32, f32) {
        let span = b - a;
        if let Some(level) = self.peaks.levels.iter().rev().find(|l| l.block * 2 <= span) {
            if let Some(d) = level.data.get(self.c) {
                let i0 = a / level.block;
                let i1 = ((b + level.block - 1) / level.block).min(d.len());
                if i1 > i0 {
                    return d[i0..i1].iter().fold((f32::MAX, f32::MIN), |(lo, hi), &(x, y)| (lo.min(x), hi.max(y)));
                }
            }
        }
        self.ch[a..b].iter().fold((f32::MAX, f32::MIN), |(lo, hi), &s| (lo.min(s), hi.max(s)))
    }
}

fn draw_amp_ruler(p: &egui::Painter, r: Rect, x: f32, amp_zoom: f32) {
    let mid = r.center().y;
    let half = r.height() * 0.5 - 1.0;
    let font = FontId::monospace(9.5);
    let mut last_y = f32::MIN;
    for db in [0.0f32, -3.0, -6.0, -9.0, -12.0, -18.0, -24.0, -36.0, -48.0] {
        let lin = 10f32.powf(db / 20.0);
        let dy = lin * half * amp_zoom;
        if dy > half + 0.5 {
            continue;
        }
        let y = mid - dy;
        if (y - last_y).abs() < 12.0 || (mid - y) < 10.0 {
            continue;
        }
        last_y = y;
        p.line_segment([pos2(x, y), pos2(x + 4.0, y)], Stroke::new(1.0_f32, TEXT_DIM));
        p.text(pos2(x + 7.0, y), Align2::LEFT_CENTER, format!("{db:.0}"), font.clone(), TEXT_DIM);
        let y2 = mid + dy;
        p.line_segment([pos2(x, y2), pos2(x + 4.0, y2)], Stroke::new(1.0_f32, BORDER));
    }
    p.text(pos2(x + 7.0, mid), Align2::LEFT_CENTER, "-∞", font.clone(), TEXT_DIM);
    p.text(pos2(x + 7.0, r.top() + 6.0), Align2::LEFT_CENTER, "dB", font, TEXT_DIM);
}
