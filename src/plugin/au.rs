//! Audio Unit (AUv2 API) hosting on macOS through AudioToolbox: listing
//! effects, processing, parameters, saved state and the plug-in's own view.

#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_void};
use std::ptr::{null, null_mut};

use super::view::native as ns;
use super::vst3::{ParamInfo, PARAM_READ_ONLY};

type OSStatus = i32;
type AudioUnit = *mut c_void;
type AudioComponent = *mut c_void;
type CFRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Desc {
    pub kind: u32,
    pub sub: u32,
    pub manu: u32,
    pub flags: u32,
    pub mask: u32,
}

#[repr(C)]
struct Asbd {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels: u32,
    bits: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioBuffer {
    channels: u32,
    size: u32,
    data: *mut c_void,
}

/// AudioBufferList with room for two buffers.
#[repr(C)]
struct BufferList {
    n: u32,
    buffers: [AudioBuffer; 2],
}

#[repr(C)]
#[derive(Default)]
struct Smpte {
    subframes: i16,
    divisor: i16,
    counter: u32,
    kind: u32,
    flags: u32,
    hours: i16,
    minutes: i16,
    seconds: i16,
    frames: i16,
}

#[repr(C)]
#[derive(Default)]
struct TimeStamp {
    sample_time: f64,
    host_time: u64,
    rate_scalar: f64,
    word_clock: u64,
    smpte: Smpte,
    flags: u32,
    reserved: u32,
}

type RenderProc = unsafe extern "C" fn(*mut c_void, *mut u32, *const TimeStamp, u32, u32, *mut BufferList) -> OSStatus;

#[repr(C)]
struct RenderCallback {
    proc_: RenderProc,
    refcon: *mut c_void,
}

#[repr(C)]
struct ParamInfoRaw {
    name: [c_char; 52],
    unit_name: CFRef,
    clump: u32,
    cf_name: CFRef,
    unit: u32,
    min: f32,
    max: f32,
    default: f32,
    flags: u32,
}

#[repr(C)]
struct StringFromValue {
    param: u32,
    value: *const f32,
    out: CFRef,
}

#[repr(C)]
struct AuParameter {
    unit: AudioUnit,
    id: u32,
    scope: u32,
    element: u32,
}

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioComponentFindNext(c: AudioComponent, d: *const Desc) -> AudioComponent;
    fn AudioComponentCopyName(c: AudioComponent, name: *mut CFRef) -> OSStatus;
    fn AudioComponentGetDescription(c: AudioComponent, d: *mut Desc) -> OSStatus;
    fn AudioComponentGetVersion(c: AudioComponent, v: *mut u32) -> OSStatus;
    fn AudioComponentInstanceNew(c: AudioComponent, out: *mut AudioUnit) -> OSStatus;
    fn AudioComponentInstanceDispose(u: AudioUnit) -> OSStatus;
    fn AudioUnitInitialize(u: AudioUnit) -> OSStatus;
    fn AudioUnitUninitialize(u: AudioUnit) -> OSStatus;
    fn AudioUnitGetPropertyInfo(u: AudioUnit, id: u32, scope: u32, el: u32, size: *mut u32, writable: *mut u8) -> OSStatus;
    fn AudioUnitGetProperty(u: AudioUnit, id: u32, scope: u32, el: u32, data: *mut c_void, size: *mut u32) -> OSStatus;
    fn AudioUnitSetProperty(u: AudioUnit, id: u32, scope: u32, el: u32, data: *const c_void, size: u32) -> OSStatus;
    fn AudioUnitGetParameter(u: AudioUnit, id: u32, scope: u32, el: u32, v: *mut f32) -> OSStatus;
    fn AudioUnitRender(u: AudioUnit, flags: *mut u32, ts: *const TimeStamp, bus: u32, frames: u32, data: *mut BufferList) -> OSStatus;
    fn AudioUnitReset(u: AudioUnit, scope: u32, el: u32) -> OSStatus;
    fn AUParameterSet(listener: *mut c_void, sender: *mut c_void, p: *const AuParameter, v: f32, offset: u32) -> OSStatus;
    fn AUParameterListenerNotify(listener: *mut c_void, sender: *mut c_void, p: *const AuParameter) -> OSStatus;
}

// AUGenericView, for units without a view of their own.
#[link(name = "CoreAudioKit", kind = "framework")]
extern "C" {}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(o: CFRef);
    fn CFStringGetLength(s: CFRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(len: isize, enc: u32) -> isize;
    fn CFStringGetCString(s: CFRef, buf: *mut c_char, size: isize, enc: u32) -> u8;
    fn CFPropertyListCreateData(alloc: CFRef, plist: CFRef, format: isize, options: usize, err: *mut CFRef) -> CFRef;
    fn CFPropertyListCreateWithData(alloc: CFRef, data: CFRef, options: usize, format: *mut isize, err: *mut CFRef) -> CFRef;
    fn CFDataCreate(alloc: CFRef, bytes: *const u8, len: isize) -> CFRef;
    fn CFDataGetLength(d: CFRef) -> isize;
    fn CFDataGetBytePtr(d: CFRef) -> *const u8;
    fn CFArrayGetCount(a: CFRef) -> isize;
    fn CFArrayGetValueAtIndex(a: CFRef, i: isize) -> CFRef;
}

const UTF8: u32 = 0x0800_0100;
const GLOBAL: u32 = 0;
const INPUT: u32 = 1;
const OUTPUT: u32 = 2;

const P_CLASS_INFO: u32 = 0;
const P_PARAMETER_LIST: u32 = 3;
const P_PARAMETER_INFO: u32 = 4;
const P_STREAM_FORMAT: u32 = 8;
const P_ELEMENT_COUNT: u32 = 11;
const P_LATENCY: u32 = 12;
const P_MAX_FRAMES: u32 = 14;
const P_VALUE_STRINGS: u32 = 16;
const P_RENDER_CALLBACK: u32 = 23;
const P_COCOA_UI: u32 = 31;
const P_STRING_FROM_VALUE: u32 = 33;
const P_OFFLINE_RENDER: u32 = 37;

const FLAG_VALUES_HAVE_STRINGS: u32 = 1 << 21;
const FLAG_HAS_CF_NAME: u32 = 1 << 27;
const FLAG_CF_NAME_RELEASE: u32 = 1 << 4;
const FLAG_WRITABLE: u32 = 1 << 31;

const UNIT_INDEXED: u32 = 1;
const UNIT_BOOLEAN: u32 = 2;
const UNIT_CUSTOM: u32 = 26;

pub const TYPE_EFFECT: u32 = u32::from_be_bytes(*b"aufx");
pub const TYPE_MUSIC_EFFECT: u32 = u32::from_be_bytes(*b"aumf");

unsafe fn cf_string(s: CFRef) -> String {
    if s.is_null() {
        return String::new();
    }
    let len = CFStringGetLength(s);
    let cap = CFStringGetMaximumSizeForEncoding(len, UTF8) + 1;
    let mut buf = vec![0 as c_char; cap.max(1) as usize];
    if CFStringGetCString(s, buf.as_mut_ptr(), cap, UTF8) == 0 {
        return String::new();
    }
    std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().to_string()
}

/// "aufx" etc. as 24 hex digits: type, subtype, manufacturer.
pub fn desc_to_cid(d: &Desc) -> String {
    format!("{:08X}{:08X}{:08X}", d.kind, d.sub, d.manu)
}

pub fn cid_to_desc(cid: &str) -> Option<Desc> {
    if cid.len() != 24 {
        return None;
    }
    let p = |i: usize| u32::from_str_radix(cid.get(i * 8..i * 8 + 8)?, 16).ok();
    Some(Desc { kind: p(0)?, sub: p(1)?, manu: p(2)?, flags: 0, mask: 0 })
}

pub struct Listed {
    pub cid: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
}

/// Every Audio Unit effect installed (no plug-in code is loaded).
pub fn list() -> Vec<Listed> {
    let mut out = Vec::new();
    for kind in [TYPE_EFFECT, TYPE_MUSIC_EFFECT] {
        let want = Desc { kind, ..Default::default() };
        let mut c: AudioComponent = null_mut();
        loop {
            c = unsafe { AudioComponentFindNext(c, &want) };
            if c.is_null() {
                break;
            }
            let mut d = Desc::default();
            let mut name: CFRef = null();
            let mut ver = 0u32;
            unsafe {
                if AudioComponentGetDescription(c, &mut d) != 0 {
                    continue;
                }
                AudioComponentCopyName(c, &mut name);
                AudioComponentGetVersion(c, &mut ver);
            }
            let full = unsafe { cf_string(name) };
            if !name.is_null() {
                unsafe { CFRelease(name) };
            }
            // Names are "Manufacturer: Plug-in".
            let (vendor, plug) = match full.split_once(": ") {
                Some((v, n)) => (v.trim().to_string(), n.trim().to_string()),
                None => (String::new(), full.clone()),
            };
            out.push(Listed { cid: desc_to_cid(&d), name: plug, vendor, version: format!("{}.{}.{}", ver >> 16, ver >> 8 & 0xFF, ver & 0xFF) });
        }
    }
    out
}

/// Input audio for the render callback (fixed address while the unit lives).
struct InputState {
    bufs: Vec<Vec<f32>>,
    frames: usize,
    channels: usize,
}

unsafe extern "C" fn render_input(refcon: *mut c_void, _flags: *mut u32, _ts: *const TimeStamp, _bus: u32, frames: u32, data: *mut BufferList) -> OSStatus {
    let st = &mut *(refcon as *mut InputState);
    let list = &mut *data;
    let n = (frames as usize).min(st.frames);
    let count = (list.n as usize).min(8);
    // AudioBufferList is variable-length; walk it by pointer.
    let first = list.buffers.as_mut_ptr();
    for i in 0..count {
        let b = &mut *first.add(i);
        let k = i.min(st.channels.saturating_sub(1)).min(st.bufs.len() - 1);
        let src = &mut st.bufs[k];
        if b.data.is_null() {
            b.data = src.as_mut_ptr() as *mut c_void;
        } else if b.data as *const f32 != src.as_ptr() {
            std::ptr::copy_nonoverlapping(src.as_ptr(), b.data as *mut f32, n);
        }
        b.size = (n * 4) as u32;
    }
    0
}

struct Param {
    info: ParamInfo,
    min: f32,
    max: f32,
    unit: u32,
    strings: bool,
}

pub struct Instance {
    pub name: String,
    unit: AudioUnit,
    input: Box<InputState>,
    out_bufs: Vec<Vec<f32>>,
    channels: usize,
    max_block: usize,
    pos: f64,
    pub latency: usize,
    params: Vec<Param>,
    /// Last values seen, to notice edits made in the unit's own view.
    seen: Vec<f32>,
}

unsafe impl Send for Instance {}

impl Instance {
    pub fn new(cid: &str, name: &str, sample_rate: f64, channels: usize, max_block: usize, offline: bool) -> Result<Instance, String> {
        let desc = cid_to_desc(cid).ok_or("bad Audio Unit id")?;
        let comp = unsafe { AudioComponentFindNext(null_mut(), &desc) };
        if comp.is_null() {
            return Err(format!("{name}: the Audio Unit isn't installed any more."));
        }
        let mut unit: AudioUnit = null_mut();
        if unsafe { AudioComponentInstanceNew(comp, &mut unit) } != 0 || unit.is_null() {
            return Err(format!("{name}: the Audio Unit couldn't be opened."));
        }
        let max_block = max_block.max(32);
        let mut inst = Instance {
            name: name.to_string(),
            unit,
            input: Box::new(InputState { bufs: vec![vec![0.0; max_block]; 2], frames: 0, channels: 2 }),
            out_bufs: vec![vec![0.0; max_block]; 2],
            channels: 2,
            max_block,
            pos: 0.0,
            latency: 0,
            params: Vec::new(),
            seen: Vec::new(),
        };
        unsafe {
            let n_in = inst.element_count(INPUT);
            // Stereo if the unit takes it, otherwise mono.
            let mut ok = false;
            for ch in [channels.clamp(1, 2), 3 - channels.clamp(1, 2)] {
                let f = Asbd {
                    sample_rate,
                    format_id: u32::from_be_bytes(*b"lpcm"),
                    format_flags: 1 | 8 | 32, // float, packed, non-interleaved
                    bytes_per_packet: 4,
                    frames_per_packet: 1,
                    bytes_per_frame: 4,
                    channels: ch as u32,
                    bits: 32,
                    reserved: 0,
                };
                let size = std::mem::size_of::<Asbd>() as u32;
                let out_ok = AudioUnitSetProperty(unit, P_STREAM_FORMAT, OUTPUT, 0, &f as *const Asbd as *const c_void, size) == 0;
                let in_ok = n_in == 0 || AudioUnitSetProperty(unit, P_STREAM_FORMAT, INPUT, 0, &f as *const Asbd as *const c_void, size) == 0;
                if out_ok && in_ok {
                    inst.channels = ch;
                    ok = true;
                    break;
                }
            }
            if !ok {
                return Err(format!("{name}: doesn't accept mono or stereo 32-bit audio."));
            }
            inst.input.channels = inst.channels;
            let mf = max_block as u32;
            AudioUnitSetProperty(unit, P_MAX_FRAMES, GLOBAL, 0, &mf as *const u32 as *const c_void, 4);
            let off: u32 = offline as u32;
            AudioUnitSetProperty(unit, P_OFFLINE_RENDER, GLOBAL, 0, &off as *const u32 as *const c_void, 4);
            if n_in > 0 {
                let cb = RenderCallback { proc_: render_input, refcon: &mut *inst.input as *mut InputState as *mut c_void };
                if AudioUnitSetProperty(unit, P_RENDER_CALLBACK, INPUT, 0, &cb as *const RenderCallback as *const c_void, std::mem::size_of::<RenderCallback>() as u32) != 0 {
                    return Err(format!("{name}: couldn't connect its input."));
                }
            }
            if AudioUnitInitialize(unit) != 0 {
                return Err(format!("{name}: the Audio Unit failed to initialise."));
            }
        }
        inst.read_latency();
        inst.params = inst.read_params();
        inst.seen = inst.params.iter().map(|p| inst.raw(p.info.id)).collect();
        Ok(inst)
    }

    unsafe fn element_count(&self, scope: u32) -> u32 {
        let mut n = 0u32;
        let mut size = 4u32;
        if AudioUnitGetProperty(self.unit, P_ELEMENT_COUNT, scope, 0, &mut n as *mut u32 as *mut c_void, &mut size) != 0 {
            return 1;
        }
        n
    }

    fn read_latency(&mut self) {
        let mut secs = 0f64;
        let mut size = 8u32;
        unsafe { AudioUnitGetProperty(self.unit, P_LATENCY, GLOBAL, 0, &mut secs as *mut f64 as *mut c_void, &mut size) };
        // Stored as seconds; the instance only knows its rate via the format.
        let mut f: Asbd = unsafe { std::mem::zeroed() };
        let mut fs = std::mem::size_of::<Asbd>() as u32;
        unsafe { AudioUnitGetProperty(self.unit, P_STREAM_FORMAT, OUTPUT, 0, &mut f as *mut Asbd as *mut c_void, &mut fs) };
        self.latency = (secs.max(0.0) * f.sample_rate).round() as usize;
    }

    fn read_params(&self) -> Vec<Param> {
        let mut out = Vec::new();
        unsafe {
            let mut size = 0u32;
            if AudioUnitGetPropertyInfo(self.unit, P_PARAMETER_LIST, GLOBAL, 0, &mut size, null_mut()) != 0 || size == 0 {
                return out;
            }
            let mut ids = vec![0u32; size as usize / 4];
            if AudioUnitGetProperty(self.unit, P_PARAMETER_LIST, GLOBAL, 0, ids.as_mut_ptr() as *mut c_void, &mut size) != 0 {
                return out;
            }
            ids.truncate(size as usize / 4);
            for id in ids {
                let mut pi: ParamInfoRaw = std::mem::zeroed();
                let mut s = std::mem::size_of::<ParamInfoRaw>() as u32;
                if AudioUnitGetProperty(self.unit, P_PARAMETER_INFO, GLOBAL, id, &mut pi as *mut ParamInfoRaw as *mut c_void, &mut s) != 0 {
                    continue;
                }
                let title = if pi.flags & FLAG_HAS_CF_NAME != 0 && !pi.cf_name.is_null() {
                    let t = cf_string(pi.cf_name);
                    if pi.flags & FLAG_CF_NAME_RELEASE != 0 {
                        CFRelease(pi.cf_name);
                    }
                    t
                } else {
                    let bytes: Vec<u8> = pi.name.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
                    String::from_utf8_lossy(&bytes).to_string()
                };
                let units = match pi.unit {
                    UNIT_CUSTOM => cf_string(pi.unit_name),
                    u => unit_label(u).to_string(),
                };
                let range = (pi.max - pi.min).max(1e-9);
                let steps = if pi.unit == UNIT_INDEXED || pi.unit == UNIT_BOOLEAN { (pi.max - pi.min).round().max(1.0) as i32 } else { 0 };
                let flags = if pi.flags & FLAG_WRITABLE == 0 { PARAM_READ_ONLY } else { 0 };
                out.push(Param {
                    info: ParamInfo { id, title, units, steps, default: ((pi.default - pi.min) / range).clamp(0.0, 1.0) as f64, flags },
                    min: pi.min,
                    max: pi.max,
                    unit: pi.unit,
                    strings: pi.flags & FLAG_VALUES_HAVE_STRINGS != 0,
                });
            }
        }
        out
    }

    pub fn params(&self) -> Vec<ParamInfo> {
        self.params.iter().map(|p| p.info.clone()).collect()
    }

    fn param(&self, id: u32) -> Option<&Param> {
        self.params.iter().find(|p| p.info.id == id)
    }

    fn raw(&self, id: u32) -> f32 {
        let mut v = 0f32;
        unsafe { AudioUnitGetParameter(self.unit, id, GLOBAL, 0, &mut v) };
        v
    }

    pub fn get_param(&self, id: u32) -> f64 {
        let Some(p) = self.param(id) else { return 0.0 };
        ((self.raw(id) - p.min) / (p.max - p.min).max(1e-9)).clamp(0.0, 1.0) as f64
    }

    pub fn param_text(&self, id: u32, v: f64) -> String {
        let Some(p) = self.param(id) else { return format!("{v:.3}") };
        let x = p.min + (p.max - p.min) * v as f32;
        unsafe {
            if p.strings {
                let mut sv = StringFromValue { param: id, value: &x, out: null() };
                let mut size = std::mem::size_of::<StringFromValue>() as u32;
                if AudioUnitGetProperty(self.unit, P_STRING_FROM_VALUE, GLOBAL, 0, &mut sv as *mut StringFromValue as *mut c_void, &mut size) == 0 && !sv.out.is_null() {
                    let t = cf_string(sv.out);
                    CFRelease(sv.out);
                    return t;
                }
            }
            if p.unit == UNIT_INDEXED {
                let mut arr: CFRef = null();
                let mut size = std::mem::size_of::<CFRef>() as u32;
                if AudioUnitGetProperty(self.unit, P_VALUE_STRINGS, GLOBAL, id, &mut arr as *mut CFRef as *mut c_void, &mut size) == 0 && !arr.is_null() {
                    let i = (x - p.min).round() as isize;
                    let t = if i >= 0 && i < CFArrayGetCount(arr) { cf_string(CFArrayGetValueAtIndex(arr, i)) } else { String::new() };
                    CFRelease(arr);
                    if !t.is_empty() {
                        return t;
                    }
                }
            }
        }
        match p.unit {
            UNIT_BOOLEAN => (if x >= 0.5 { "On" } else { "Off" }).to_string(),
            UNIT_INDEXED => format!("{}", x.round()),
            _ if (p.max - p.min) >= 100.0 => format!("{x:.1}"),
            _ => format!("{x:.2}"),
        }
    }

    pub fn set_param(&mut self, id: u32, v: f64) {
        let Some(p) = self.param(id) else { return };
        let x = p.min + (p.max - p.min) * v.clamp(0.0, 1.0) as f32;
        let ap = AuParameter { unit: self.unit, id, scope: GLOBAL, element: 0 };
        // AUParameterSet also tells the unit's view about the change.
        unsafe { AUParameterSet(null_mut(), null_mut(), &ap, x, 0) };
        if let Some(i) = self.params.iter().position(|q| q.info.id == id) {
            self.seen[i] = self.raw(id);
        }
    }

    /// Parameters changed in the unit's own view since the last call.
    pub fn take_touched(&mut self) -> Option<Vec<(u32, f64)>> {
        let mut changed = Vec::new();
        for i in 0..self.params.len() {
            let id = self.params[i].info.id;
            let v = self.raw(id);
            if v != self.seen[i] {
                self.seen[i] = v;
                changed.push((id, self.get_param(id)));
            }
        }
        (!changed.is_empty()).then_some(changed)
    }

    pub fn reset(&mut self) {
        unsafe { AudioUnitReset(self.unit, GLOBAL, 0) };
        self.pos = 0.0;
    }

    /// Process `l`/`r` in place. Allocation-free.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let total = l.len().min(r.len());
        let mut off = 0;
        while off < total {
            let n = (total - off).min(self.max_block);
            self.block(&mut l[off..off + n], &mut r[off..off + n]);
            off += n;
        }
    }

    fn block(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        if self.channels == 1 {
            for i in 0..n {
                self.input.bufs[0][i] = 0.5 * (l[i] + r[i]);
            }
        } else {
            self.input.bufs[0][..n].copy_from_slice(l);
            self.input.bufs[1][..n].copy_from_slice(r);
        }
        self.input.frames = n;
        let mut list = BufferList { n: self.channels as u32, buffers: [AudioBuffer { channels: 1, size: 0, data: null_mut() }; 2] };
        for c in 0..self.channels {
            list.buffers[c] = AudioBuffer { channels: 1, size: (n * 4) as u32, data: self.out_bufs[c].as_mut_ptr() as *mut c_void };
        }
        let ts = TimeStamp { sample_time: self.pos, flags: 1, ..Default::default() };
        let mut flags = 0u32;
        let ok = unsafe { AudioUnitRender(self.unit, &mut flags, &ts, 0, n as u32, &mut list) } == 0;
        self.pos += n as f64;
        if !ok {
            return;
        }
        // The unit may have pointed the buffers at its own memory.
        unsafe {
            let a = std::slice::from_raw_parts(list.buffers[0].data as *const f32, n);
            l.copy_from_slice(a);
            if self.channels == 2 {
                r.copy_from_slice(std::slice::from_raw_parts(list.buffers[1].data as *const f32, n));
            } else {
                r.copy_from_slice(a);
            }
        }
    }

    /// The unit's state (its ClassInfo property list, in binary form).
    pub fn get_state(&self) -> Vec<u8> {
        unsafe {
            let mut plist: CFRef = null();
            let mut size = std::mem::size_of::<CFRef>() as u32;
            if AudioUnitGetProperty(self.unit, P_CLASS_INFO, GLOBAL, 0, &mut plist as *mut CFRef as *mut c_void, &mut size) != 0 || plist.is_null() {
                return Vec::new();
            }
            let data = CFPropertyListCreateData(null(), plist, 200, 0, null_mut());
            CFRelease(plist);
            if data.is_null() {
                return Vec::new();
            }
            let v = std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data) as usize).to_vec();
            CFRelease(data);
            v
        }
    }

    pub fn set_state(&mut self, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }
        unsafe {
            let data = CFDataCreate(null(), bytes.as_ptr(), bytes.len() as isize);
            if data.is_null() {
                return false;
            }
            let mut fmt = 0isize;
            let plist = CFPropertyListCreateWithData(null(), data, 0, &mut fmt, null_mut());
            CFRelease(data);
            if plist.is_null() {
                return false;
            }
            let ok = AudioUnitSetProperty(self.unit, P_CLASS_INFO, GLOBAL, 0, &plist as *const CFRef as *const c_void, std::mem::size_of::<CFRef>() as u32) == 0;
            CFRelease(plist);
            // Let the unit's view refresh every parameter.
            let any = AuParameter { unit: self.unit, id: 0xFFFF_FFFF, scope: GLOBAL, element: 0 };
            AUParameterListenerNotify(null_mut(), null_mut(), &any);
            self.seen = self.params.iter().map(|p| self.raw(p.info.id)).collect();
            self.read_latency();
            ok
        }
    }

    /// The unit's own view (or Apple's generic one) in a window.
    pub fn open_window(&self, title: &str) -> Result<AuWindow, String> {
        unsafe {
            let view = self.cocoa_view().or_else(|| self.generic_view()).ok_or("This Audio Unit has no view to show.")?;
            let mut win = ns::create(title, 400, 300, true)?;
            ns::set_content(&mut win, view);
            // The window retains the view now.
            ns::send0(view, b"release\0");
            ns::show(&mut win);
            Ok(AuWindow { win })
        }
    }

    /// A view from the unit's Cocoa UI bundle (+1 retained).
    unsafe fn cocoa_view(&self) -> Option<ns::Id> {
        let mut size = 0u32;
        if AudioUnitGetPropertyInfo(self.unit, P_COCOA_UI, GLOBAL, 0, &mut size, null_mut()) != 0 || (size as usize) < 2 * std::mem::size_of::<CFRef>() {
            return None;
        }
        let mut buf = vec![null::<c_void>(); size as usize / std::mem::size_of::<CFRef>()];
        if AudioUnitGetProperty(self.unit, P_COCOA_UI, GLOBAL, 0, buf.as_mut_ptr() as *mut c_void, &mut size) != 0 {
            return None;
        }
        // { CFURLRef bundle; CFStringRef classes[] }: we own every reference.
        let (url, class_name) = (buf[0], buf[1]);
        let mut view = None;
        if !url.is_null() && !class_name.is_null() {
            let bundle = ns::send_id(ns::class(b"NSBundle\0"), b"bundleWithURL:\0", url as ns::Id);
            if !bundle.is_null() {
                let cls = ns::send_id(bundle, b"classNamed:\0", class_name as ns::Id);
                if !cls.is_null() {
                    let factory = ns::send0(ns::send0(cls, b"alloc\0"), b"init\0");
                    if !factory.is_null() {
                        let f: unsafe extern "C" fn(ns::Id, ns::Sel, AudioUnit, ns::NsSize) -> ns::Id = std::mem::transmute(ns::objc_msgSend as unsafe extern "C" fn());
                        let v = f(factory, ns::sel(b"uiViewForAudioUnit:withSize:\0"), self.unit, ns::NsSize { w: 0.0, h: 0.0 });
                        if !v.is_null() {
                            ns::send0(v, b"retain\0");
                            view = Some(v);
                        }
                        ns::send0(factory, b"release\0");
                    }
                }
            }
        }
        for r in buf.iter().take(size as usize / std::mem::size_of::<CFRef>()) {
            if !r.is_null() {
                CFRelease(*r);
            }
        }
        view
    }

    /// Apple's generic parameter view (+1 retained).
    unsafe fn generic_view(&self) -> Option<ns::Id> {
        let cls = ns::class(b"AUGenericView\0");
        if cls.is_null() {
            return None;
        }
        let f: unsafe extern "C" fn(ns::Id, ns::Sel, AudioUnit) -> ns::Id = std::mem::transmute(ns::objc_msgSend as unsafe extern "C" fn());
        let v = f(ns::send0(cls, b"alloc\0"), ns::sel(b"initWithAudioUnit:\0"), self.unit);
        (!v.is_null()).then_some(v)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            AudioUnitUninitialize(self.unit);
            AudioComponentInstanceDispose(self.unit);
        }
    }
}

pub struct AuWindow {
    win: Box<ns::Win>,
}

impl super::view::Window for AuWindow {
    fn closed(&self) -> bool {
        ns::closed(&self.win)
    }
}

impl Drop for AuWindow {
    fn drop(&mut self) {
        ns::destroy(&mut self.win);
    }
}

fn unit_label(u: u32) -> &'static str {
    // AudioUnitParameterUnit values.
    match u {
        3 => "%",
        4 => "s",
        5 => "samples",
        6 | 15 => "°",
        8 => "Hz",
        9 | 20 => "cents",
        10 => "semitones",
        13 => "dB",
        21 => "octaves",
        22 => "BPM",
        23 => "beats",
        24 => "ms",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        let d = Desc { kind: TYPE_EFFECT, sub: u32::from_be_bytes(*b"dely"), manu: u32::from_be_bytes(*b"appl"), flags: 0, mask: 0 };
        assert_eq!(cid_to_desc(&desc_to_cid(&d)), Some(d));
        assert_eq!(cid_to_desc("nope"), None);
    }
}
