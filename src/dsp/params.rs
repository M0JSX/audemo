//! Parameter descriptions and values for effect dialogs.

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    F(f32),
    C(usize),
    B(bool),
    S(String),
}

#[derive(Clone, Debug)]
pub enum Kind {
    Float {
        min: f32,
        max: f32,
        default: f32,
        unit: &'static str,
        log: bool,
        decimals: usize,
    },
    Choice {
        options: &'static [&'static str],
        default: usize,
    },
    Toggle {
        default: bool,
    },
    Text {
        default: &'static str,
    },
    /// A curve drawn in the dialog: points "x:y,x:y,…" (stored as text),
    /// x from `x_min` to `x_max` (log axis when `log_x`), y in the range.
    Curve {
        x_min: f32,
        x_max: f32,
        log_x: bool,
        x_unit: &'static str,
        y_min: f32,
        y_max: f32,
        y_unit: &'static str,
        default: &'static str,
    },
}

#[derive(Clone, Debug)]
pub struct ParamDef {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    /// Visual grouping in the dialog ("" = ungrouped).
    pub group: &'static str,
}

impl ParamDef {
    pub fn log(mut self) -> Self {
        if let Kind::Float { log, .. } = &mut self.kind {
            *log = true;
        }
        self
    }
    pub fn decimals(mut self, d: usize) -> Self {
        if let Kind::Float { decimals, .. } = &mut self.kind {
            *decimals = d;
        }
        self
    }
    pub fn group(mut self, g: &'static str) -> Self {
        self.group = g;
        self
    }
    pub fn default_value(&self) -> Value {
        match &self.kind {
            Kind::Float { default, .. } => Value::F(*default),
            Kind::Choice { default, .. } => Value::C(*default),
            Kind::Toggle { default } => Value::B(*default),
            Kind::Text { default } => Value::S(default.to_string()),
            Kind::Curve { default, .. } => Value::S(default.to_string()),
        }
    }
}

pub fn float(
    key: &'static str,
    label: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
) -> ParamDef {
    let span = (max - min).abs();
    let decimals = if span <= 2.0 {
        2
    } else if span <= 200.0 {
        1
    } else {
        0
    };
    ParamDef {
        key,
        label,
        kind: Kind::Float { min, max, default, unit, log: false, decimals },
        group: "",
    }
}

pub fn choice(
    key: &'static str,
    label: &'static str,
    options: &'static [&'static str],
    default: usize,
) -> ParamDef {
    ParamDef { key, label, kind: Kind::Choice { options, default }, group: "" }
}

pub fn toggle(key: &'static str, label: &'static str, default: bool) -> ParamDef {
    ParamDef { key, label, kind: Kind::Toggle { default }, group: "" }
}

pub fn text(key: &'static str, label: &'static str, default: &'static str) -> ParamDef {
    ParamDef { key, label, kind: Kind::Text { default }, group: "" }
}

/// A drawn curve over time (x 0–100 % of the selection).
pub fn time_curve(key: &'static str, label: &'static str, y: (f32, f32, &'static str), default: &'static str) -> ParamDef {
    ParamDef { key, label, kind: Kind::Curve { x_min: 0.0, x_max: 100.0, log_x: false, x_unit: "%", y_min: y.0, y_max: y.1, y_unit: y.2, default }, group: "" }
}

/// A drawn curve over frequency (log axis, 20 Hz – 20 kHz).
pub fn freq_curve(key: &'static str, label: &'static str, y: (f32, f32, &'static str), default: &'static str) -> ParamDef {
    ParamDef { key, label, kind: Kind::Curve { x_min: 20.0, x_max: 20000.0, log_x: true, x_unit: "Hz", y_min: y.0, y_max: y.1, y_unit: y.2, default }, group: "" }
}

/// Points of a curve value, sorted by x.
pub fn parse_curve(s: &str) -> Vec<(f32, f32)> {
    let mut v: Vec<(f32, f32)> = s
        .split(',')
        .filter_map(|p| {
            let (x, y) = p.split_once(':')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .filter(|(x, y): &(f32, f32)| x.is_finite() && y.is_finite())
        .collect();
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    v
}

pub fn curve_text(pts: &[(f32, f32)]) -> String {
    pts.iter().map(|(x, y)| format!("{}:{}", round4(*x), round4(*y))).collect::<Vec<_>>().join(",")
}

fn round4(v: f32) -> f32 {
    (v * 10000.0).round() / 10000.0
}

/// The curve's value at `x`: straight lines between points, or a smooth
/// (Catmull-Rom) curve through them with `spline`. `log_x` interpolates
/// in log x (frequency curves). Flat beyond the end points.
pub fn eval_curve(pts: &[(f32, f32)], x: f32, spline: bool, log_x: bool) -> f32 {
    match pts.len() {
        0 => return 0.0,
        1 => return pts[0].1,
        _ => {}
    }
    let tx = |v: f32| if log_x { v.max(1e-6).ln() } else { v };
    let x = tx(x);
    let n = pts.len();
    if x <= tx(pts[0].0) {
        return pts[0].1;
    }
    if x >= tx(pts[n - 1].0) {
        return pts[n - 1].1;
    }
    let i = (0..n - 1).find(|&i| x <= tx(pts[i + 1].0)).unwrap_or(n - 2);
    let (x0, y0) = (tx(pts[i].0), pts[i].1);
    let (x1, y1) = (tx(pts[i + 1].0), pts[i + 1].1);
    let w = x1 - x0;
    if w <= 0.0 {
        return y1;
    }
    let t = (x - x0) / w;
    if !spline {
        return y0 + (y1 - y0) * t;
    }
    // Catmull-Rom tangents (scaled to this segment), clamped at the ends.
    let slope = |a: usize, b: usize| (pts[b].1 - pts[a].1) / (tx(pts[b].0) - tx(pts[a].0)).max(1e-9);
    let m0 = if i == 0 { slope(0, 1) } else { slope(i - 1, i + 1) } * w;
    let m1 = if i + 2 >= n { slope(n - 2, n - 1) } else { slope(i, i + 2) } * w;
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * m0 + (-2.0 * t3 + 3.0 * t2) * y1 + (t3 - t2) * m1
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Params {
    pub values: Vec<(&'static str, Value)>,
}

impl Params {
    pub fn defaults(defs: &[ParamDef]) -> Self {
        Params { values: defs.iter().map(|d| (d.key, d.default_value())).collect() }
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.values.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    pub fn f(&self, key: &str) -> f32 {
        match self.get(key) {
            Some(Value::F(v)) => *v,
            Some(Value::C(c)) => *c as f32,
            Some(Value::B(b)) => *b as u8 as f32,
            Some(Value::S(_)) | None => 0.0,
        }
    }

    pub fn c(&self, key: &str) -> usize {
        match self.get(key) {
            Some(Value::C(c)) => *c,
            Some(Value::F(v)) => v.max(0.0) as usize,
            _ => 0,
        }
    }

    pub fn b(&self, key: &str) -> bool {
        match self.get(key) {
            Some(Value::B(b)) => *b,
            Some(Value::F(v)) => *v != 0.0,
            _ => false,
        }
    }

    /// A curve parameter's points.
    pub fn curve(&self, key: &str) -> Vec<(f32, f32)> {
        parse_curve(&self.s(key))
    }

    pub fn s(&self, key: &str) -> String {
        match self.get(key) {
            Some(Value::S(s)) => s.clone(),
            _ => String::new(),
        }
    }

    pub fn set(&mut self, key: &'static str, v: Value) {
        if let Some(slot) = self.values.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = v;
        } else {
            self.values.push((key, v));
        }
    }

    /// `key=<type><value>;…` for session files (f float, c choice, b bool, s text).
    pub fn to_text(&self) -> String {
        self.values
            .iter()
            .map(|(k, v)| {
                let v = match v {
                    Value::F(x) => format!("f{x}"),
                    Value::C(c) => format!("c{c}"),
                    Value::B(b) => format!("b{}", *b as u8),
                    Value::S(s) => format!("s{}", s.replace([';', '=', '\t', '\n', '\r'], " ")),
                };
                format!("{k}={v}")
            })
            .collect::<Vec<_>>()
            .join(";")
    }

    /// Parse [`Params::to_text`] output against an effect's parameters
    /// (unknown keys are ignored, missing ones keep their defaults).
    pub fn from_text(defs: &[ParamDef], text: &str) -> Self {
        let mut p = Params::defaults(defs);
        for kv in text.split(';') {
            let Some((k, v)) = kv.split_once('=') else { continue };
            let Some(d) = defs.iter().find(|d| d.key == k.trim()) else { continue };
            let mut chars = v.chars();
            let t = chars.next();
            let rest = chars.as_str();
            let val = match t {
                Some('f') => rest.parse().ok().map(Value::F),
                Some('c') => rest.parse().ok().map(Value::C),
                Some('b') => Some(Value::B(rest == "1")),
                Some('s') => Some(Value::S(rest.to_string())),
                _ => None,
            };
            if let Some(val) = val {
                p.set(d.key, val);
            }
        }
        p
    }

    /// Defaults with a preset's overrides applied.
    pub fn with_preset(defs: &[ParamDef], preset: &[(&'static str, Value)]) -> Self {
        let mut p = Params::defaults(defs);
        for (k, v) in preset {
            p.set(k, v.clone());
        }
        p
    }
}
