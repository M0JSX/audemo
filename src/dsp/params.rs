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

    /// Defaults with a preset's overrides applied.
    pub fn with_preset(defs: &[ParamDef], preset: &[(&'static str, Value)]) -> Self {
        let mut p = Params::defaults(defs);
        for (k, v) in preset {
            p.set(k, v.clone());
        }
        p
    }
}
