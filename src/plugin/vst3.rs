//! VST3 hosting: the binary interface (written from the MIT-licensed VST3
//! SDK headers), the host-side objects plug-ins talk to, module loading and
//! a plug-in instance that processes audio.

// A binding of the VST3 interfaces: not every constant and method is used.
#![allow(non_snake_case, dead_code, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_void, CStr};
use std::path::{Path, PathBuf};
use std::ptr::{self, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

// ------------------------------------------------------------------ basic types

pub type TResult = i32;
pub type Tuid = [u8; 16];
pub type ParamId = u32;

#[cfg(windows)]
mod codes {
    pub const NO_INTERFACE: i32 = 0x8000_4002u32 as i32;
    pub const INVALID_ARG: i32 = 0x8007_0057u32 as i32;
    pub const NOT_IMPLEMENTED: i32 = 0x8000_4001u32 as i32;
}
#[cfg(not(windows))]
mod codes {
    pub const NO_INTERFACE: i32 = -1;
    pub const INVALID_ARG: i32 = 2;
    pub const NOT_IMPLEMENTED: i32 = 3;
}
pub use codes::*;
pub const OK: TResult = 0;
pub const FALSE: TResult = 1;

/// INLINE_UID: COM byte order on Windows, big-endian elsewhere.
pub const fn uid(l1: u32, l2: u32, l3: u32, l4: u32) -> Tuid {
    let a = l1.to_be_bytes();
    let b = l2.to_be_bytes();
    let c = l3.to_be_bytes();
    let d = l4.to_be_bytes();
    if cfg!(windows) {
        [a[3], a[2], a[1], a[0], b[1], b[0], b[3], b[2], c[0], c[1], c[2], c[3], d[0], d[1], d[2], d[3]]
    } else {
        [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3], c[0], c[1], c[2], c[3], d[0], d[1], d[2], d[3]]
    }
}

pub const IID_FUNKNOWN: Tuid = uid(0x00000000, 0x00000000, 0xC0000000, 0x00000046);
pub const IID_PLUGIN_BASE: Tuid = uid(0x22888DDB, 0x156E45AE, 0x8358B348, 0x08190625);
pub const IID_PLUGIN_FACTORY2: Tuid = uid(0x0007B650, 0xF24B4C0B, 0xA464EDB9, 0xF00B2ABB);
pub const IID_COMPONENT: Tuid = uid(0xE831FF31, 0xF2D54301, 0x928EBBEE, 0x25697802);
pub const IID_AUDIO_PROCESSOR: Tuid = uid(0x42043F99, 0xB7DA453C, 0xA569E79D, 0x9AAEC33D);
pub const IID_EDIT_CONTROLLER: Tuid = uid(0xDCD7BBE3, 0x7742448D, 0xA874AACC, 0x979C759E);
pub const IID_CONNECTION_POINT: Tuid = uid(0x70A4156F, 0x6E6E4026, 0x989148BF, 0xAA60D8D1);
pub const IID_HOST_APPLICATION: Tuid = uid(0x58E595CC, 0xDB2D4969, 0x8B6AAF8C, 0x36A664E5);
pub const IID_COMPONENT_HANDLER: Tuid = uid(0x93A0BEA3, 0x0BD045DB, 0x8E890B0C, 0xC1E46AC6);
pub const IID_BSTREAM: Tuid = uid(0xC3BF6EA2, 0x30994752, 0x9B6BF990, 0x1EE33E9B);
pub const IID_PARAMETER_CHANGES: Tuid = uid(0xA4779663, 0x0BB64A56, 0xB44384A8, 0x466FEB9D);
pub const IID_PARAM_VALUE_QUEUE: Tuid = uid(0x01263A18, 0xED074F6F, 0x98C9D356, 0x4686F9BA);
pub const IID_MESSAGE: Tuid = uid(0x936F033B, 0xC6C047DB, 0xBB0882F8, 0x13C1E613);
pub const IID_ATTRIBUTE_LIST: Tuid = uid(0x1E5F0AEB, 0xCC7F4533, 0xA2544011, 0x38AD5EE4);

pub const MEDIA_AUDIO: i32 = 0;
pub const MEDIA_EVENT: i32 = 1;
pub const DIR_IN: i32 = 0;
pub const DIR_OUT: i32 = 1;
pub const MODE_REALTIME: i32 = 0;
pub const MODE_OFFLINE: i32 = 2;
pub const SAMPLE_32: i32 = 0;
pub const SPEAKER_MONO: u64 = 1 << 19;
pub const SPEAKER_STEREO: u64 = 0b11;

pub const PARAM_CAN_AUTOMATE: i32 = 1 << 0;
pub const PARAM_READ_ONLY: i32 = 1 << 1;
pub const PARAM_LIST: i32 = 1 << 3;
pub const PARAM_HIDDEN: i32 = 1 << 4;
pub const PARAM_BYPASS: i32 = 1 << 16;

pub const RESTART_LATENCY: i32 = 1 << 3;

// ------------------------------------------------------------------ structs

#[repr(C)]
pub struct PClassInfo2 {
    pub cid: Tuid,
    pub cardinality: i32,
    pub category: [c_char; 32],
    pub name: [c_char; 64],
    pub class_flags: u32,
    pub sub_categories: [c_char; 128],
    pub vendor: [c_char; 64],
    pub version: [c_char; 64],
    pub sdk_version: [c_char; 64],
}

#[repr(C)]
pub struct PClassInfo {
    pub cid: Tuid,
    pub cardinality: i32,
    pub category: [c_char; 32],
    pub name: [c_char; 64],
}

#[repr(C)]
pub struct PFactoryInfo {
    pub vendor: [c_char; 64],
    pub url: [c_char; 256],
    pub email: [c_char; 128],
    pub flags: i32,
}

#[repr(C)]
pub struct BusInfo {
    pub media_type: i32,
    pub direction: i32,
    pub channel_count: i32,
    pub name: [u16; 128],
    pub bus_type: i32,
    pub flags: u32,
}

#[repr(C)]
pub struct ParameterInfo {
    pub id: ParamId,
    pub title: [u16; 128],
    pub short_title: [u16; 128],
    pub units: [u16; 128],
    pub step_count: i32,
    pub default_normalized: f64,
    pub unit_id: i32,
    pub flags: i32,
}

#[repr(C)]
pub struct ProcessSetup {
    pub process_mode: i32,
    pub sample_size: i32,
    pub max_block: i32,
    pub sample_rate: f64,
}

#[repr(C)]
pub struct AudioBusBuffers {
    pub num_channels: i32,
    pub silence_flags: u64,
    pub buffers: *mut *mut f32,
}

#[repr(C)]
pub struct ProcessContext {
    pub state: u32,
    pub sample_rate: f64,
    pub project_time_samples: i64,
    pub system_time: i64,
    pub continous_time_samples: i64,
    pub project_time_music: f64,
    pub bar_position_music: f64,
    pub cycle_start_music: f64,
    pub cycle_end_music: f64,
    pub tempo: f64,
    pub time_sig_numerator: i32,
    pub time_sig_denominator: i32,
    pub chord: [u8; 4],
    pub smpte_offset_subframes: i32,
    pub frame_rate: [u32; 2],
    pub samples_to_next_clock: i32,
}

#[repr(C)]
pub struct ProcessData {
    pub process_mode: i32,
    pub sample_size: i32,
    pub num_samples: i32,
    pub num_inputs: i32,
    pub num_outputs: i32,
    pub inputs: *mut AudioBusBuffers,
    pub outputs: *mut AudioBusBuffers,
    pub input_params: *mut c_void,
    pub output_params: *mut c_void,
    pub input_events: *mut c_void,
    pub output_events: *mut c_void,
    pub context: *mut ProcessContext,
}

// ------------------------------------------------------------------ vtables

#[repr(C)]
pub struct FUnknownVtbl {
    pub query_interface: unsafe extern "system" fn(*mut c_void, *const Tuid, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
pub struct PluginFactoryVtbl {
    pub unk: FUnknownVtbl,
    pub get_factory_info: unsafe extern "system" fn(*mut c_void, *mut PFactoryInfo) -> TResult,
    pub count_classes: unsafe extern "system" fn(*mut c_void) -> i32,
    pub get_class_info: unsafe extern "system" fn(*mut c_void, i32, *mut PClassInfo) -> TResult,
    pub create_instance: unsafe extern "system" fn(*mut c_void, *const c_char, *const c_char, *mut *mut c_void) -> TResult,
    pub get_class_info2: unsafe extern "system" fn(*mut c_void, i32, *mut PClassInfo2) -> TResult,
}

#[repr(C)]
pub struct ComponentVtbl {
    pub unk: FUnknownVtbl,
    pub initialize: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    pub terminate: unsafe extern "system" fn(*mut c_void) -> TResult,
    pub get_controller_class_id: unsafe extern "system" fn(*mut c_void, *mut Tuid) -> TResult,
    pub set_io_mode: unsafe extern "system" fn(*mut c_void, i32) -> TResult,
    pub get_bus_count: unsafe extern "system" fn(*mut c_void, i32, i32) -> i32,
    pub get_bus_info: unsafe extern "system" fn(*mut c_void, i32, i32, i32, *mut BusInfo) -> TResult,
    pub get_routing_info: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> TResult,
    pub activate_bus: unsafe extern "system" fn(*mut c_void, i32, i32, i32, u8) -> TResult,
    pub set_active: unsafe extern "system" fn(*mut c_void, u8) -> TResult,
    pub set_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    pub get_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
}

#[repr(C)]
pub struct AudioProcessorVtbl {
    pub unk: FUnknownVtbl,
    pub set_bus_arrangements: unsafe extern "system" fn(*mut c_void, *mut u64, i32, *mut u64, i32) -> TResult,
    pub get_bus_arrangement: unsafe extern "system" fn(*mut c_void, i32, i32, *mut u64) -> TResult,
    pub can_process_sample_size: unsafe extern "system" fn(*mut c_void, i32) -> TResult,
    pub get_latency_samples: unsafe extern "system" fn(*mut c_void) -> u32,
    pub setup_processing: unsafe extern "system" fn(*mut c_void, *mut ProcessSetup) -> TResult,
    pub set_processing: unsafe extern "system" fn(*mut c_void, u8) -> TResult,
    pub process: unsafe extern "system" fn(*mut c_void, *mut ProcessData) -> TResult,
    pub get_tail_samples: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
pub struct EditControllerVtbl {
    pub unk: FUnknownVtbl,
    pub initialize: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    pub terminate: unsafe extern "system" fn(*mut c_void) -> TResult,
    pub set_component_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    pub set_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    pub get_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    pub get_parameter_count: unsafe extern "system" fn(*mut c_void) -> i32,
    pub get_parameter_info: unsafe extern "system" fn(*mut c_void, i32, *mut ParameterInfo) -> TResult,
    pub get_param_string_by_value: unsafe extern "system" fn(*mut c_void, ParamId, f64, *mut u16) -> TResult,
    pub get_param_value_by_string: unsafe extern "system" fn(*mut c_void, ParamId, *const u16, *mut f64) -> TResult,
    pub normalized_param_to_plain: unsafe extern "system" fn(*mut c_void, ParamId, f64) -> f64,
    pub plain_param_to_normalized: unsafe extern "system" fn(*mut c_void, ParamId, f64) -> f64,
    pub get_param_normalized: unsafe extern "system" fn(*mut c_void, ParamId) -> f64,
    pub set_param_normalized: unsafe extern "system" fn(*mut c_void, ParamId, f64) -> TResult,
    pub set_component_handler: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    pub create_view: unsafe extern "system" fn(*mut c_void, *const c_char) -> *mut c_void,
}

#[repr(C)]
pub struct ConnectionPointVtbl {
    pub unk: FUnknownVtbl,
    pub connect: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    pub disconnect: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    pub notify: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
}

/// A counted reference to a plug-in object, released on drop.
pub struct ComPtr<V> {
    pub ptr: *mut c_void,
    _v: std::marker::PhantomData<V>,
}

impl<V> ComPtr<V> {
    /// Take ownership of a reference.
    pub unsafe fn from_raw(ptr: *mut c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(ComPtr { ptr, _v: std::marker::PhantomData })
        }
    }
    pub fn vtbl(&self) -> &V {
        unsafe { &**(self.ptr as *mut *const V) }
    }
    fn unk(&self) -> &FUnknownVtbl {
        unsafe { &**(self.ptr as *mut *const FUnknownVtbl) }
    }
    pub fn query<W>(&self, iid: &Tuid) -> Option<ComPtr<W>> {
        let mut out = null_mut();
        let r = unsafe { (self.unk().query_interface)(self.ptr, iid, &mut out) };
        if r == OK {
            unsafe { ComPtr::from_raw(out) }
        } else {
            None
        }
    }
}

impl<V> Drop for ComPtr<V> {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { (self.unk().release)(self.ptr) };
        }
    }
}

pub fn cstr(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).to_string()
}

pub fn u16str(chars: &[u16]) -> String {
    let n = chars.iter().position(|c| *c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..n])
}

pub fn tuid_hex(t: &Tuid) -> String {
    t.iter().map(|b| format!("{b:02X}")).collect()
}

pub fn tuid_from_hex(s: &str) -> Option<Tuid> {
    if s.len() != 32 {
        return None;
    }
    let mut t = [0u8; 16];
    for (i, b) in t.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(t)
}

// ------------------------------------------------------------------ host objects
//
// Each host object is a #[repr(C)] struct whose first field points at a
// static vtable, so a pointer to the struct is a valid interface pointer.

unsafe fn iid_eq(a: *const Tuid, b: &Tuid) -> bool {
    !a.is_null() && &*a == b
}

macro_rules! com_object {
    ($ty:ident, $vt:ident, [$($iid:expr),*]) => {
        unsafe extern "system" fn qi(this: *mut c_void, iid: *const Tuid, obj: *mut *mut c_void) -> TResult {
            if iid_eq(iid, &IID_FUNKNOWN) $(|| iid_eq(iid, &$iid))* {
                add_ref(this);
                *obj = this;
                OK
            } else {
                *obj = null_mut();
                NO_INTERFACE
            }
        }
        unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
            (*(this as *mut $ty)).refs.fetch_add(1, Ordering::AcqRel) + 1
        }
        unsafe extern "system" fn release(this: *mut c_void) -> u32 {
            let n = (*(this as *mut $ty)).refs.fetch_sub(1, Ordering::AcqRel) - 1;
            if n == 0 {
                drop(Box::from_raw(this as *mut $ty));
            }
            n
        }
        const UNK: FUnknownVtbl = FUnknownVtbl { query_interface: qi, add_ref, release };
    };
}

// ---- IBStream over memory

pub mod stream {
    use super::*;
    #[repr(C)]
    struct Vtbl {
        unk: FUnknownVtbl,
        read: unsafe extern "system" fn(*mut c_void, *mut c_void, i32, *mut i32) -> TResult,
        write: unsafe extern "system" fn(*mut c_void, *const c_void, i32, *mut i32) -> TResult,
        seek: unsafe extern "system" fn(*mut c_void, i64, i32, *mut i64) -> TResult,
        tell: unsafe extern "system" fn(*mut c_void, *mut i64) -> TResult,
    }
    #[repr(C)]
    pub struct MemStream {
        vtbl: *const Vtbl,
        refs: AtomicU32,
        pub data: Vec<u8>,
        pos: usize,
    }
    com_object!(MemStream, Vtbl, [IID_BSTREAM]);
    unsafe extern "system" fn read(this: *mut c_void, buf: *mut c_void, n: i32, got: *mut i32) -> TResult {
        let s = &mut *(this as *mut MemStream);
        let n = (n.max(0) as usize).min(s.data.len().saturating_sub(s.pos));
        if n > 0 {
            ptr::copy_nonoverlapping(s.data.as_ptr().add(s.pos), buf as *mut u8, n);
        }
        s.pos += n;
        if !got.is_null() {
            *got = n as i32;
        }
        OK
    }
    unsafe extern "system" fn write(this: *mut c_void, buf: *const c_void, n: i32, put: *mut i32) -> TResult {
        let s = &mut *(this as *mut MemStream);
        let n = n.max(0) as usize;
        let src = std::slice::from_raw_parts(buf as *const u8, n);
        if s.pos > s.data.len() {
            s.data.resize(s.pos, 0);
        }
        let end = s.pos + n;
        if end > s.data.len() {
            s.data.resize(end, 0);
        }
        s.data[s.pos..end].copy_from_slice(src);
        s.pos = end;
        if !put.is_null() {
            *put = n as i32;
        }
        OK
    }
    unsafe extern "system" fn seek(this: *mut c_void, pos: i64, mode: i32, result: *mut i64) -> TResult {
        let s = &mut *(this as *mut MemStream);
        let base = match mode {
            0 => 0i64,
            1 => s.pos as i64,
            2 => s.data.len() as i64,
            _ => return INVALID_ARG,
        };
        let p = base + pos;
        if p < 0 {
            return INVALID_ARG;
        }
        s.pos = p as usize;
        if !result.is_null() {
            *result = p;
        }
        OK
    }
    unsafe extern "system" fn tell(this: *mut c_void, pos: *mut i64) -> TResult {
        if !pos.is_null() {
            *pos = (*(this as *mut MemStream)).pos as i64;
        }
        OK
    }
    static VTBL: Vtbl = Vtbl { unk: UNK, read, write, seek, tell };

    /// A new stream holding `data`, with one reference owned by the caller.
    pub fn new(data: Vec<u8>) -> *mut MemStream {
        Box::into_raw(Box::new(MemStream { vtbl: &VTBL, refs: AtomicU32::new(1), data, pos: 0 }))
    }
    pub unsafe fn take(s: *mut MemStream) -> Vec<u8> {
        let d = std::mem::take(&mut (*s).data);
        release(s as *mut c_void);
        d
    }
    pub unsafe fn free(s: *mut MemStream) {
        release(s as *mut c_void);
    }
}

// ---- IAttributeList and IMessage (plug-in component ↔ controller messages)

pub mod message {
    use super::*;
    #[derive(Clone)]
    enum Attr {
        Int(i64),
        Float(f64),
        Str(Vec<u16>),
        Bin(Vec<u8>),
    }
    #[repr(C)]
    struct ListVtbl {
        unk: FUnknownVtbl,
        set_int: unsafe extern "system" fn(*mut c_void, *const c_char, i64) -> TResult,
        get_int: unsafe extern "system" fn(*mut c_void, *const c_char, *mut i64) -> TResult,
        set_float: unsafe extern "system" fn(*mut c_void, *const c_char, f64) -> TResult,
        get_float: unsafe extern "system" fn(*mut c_void, *const c_char, *mut f64) -> TResult,
        set_string: unsafe extern "system" fn(*mut c_void, *const c_char, *const u16) -> TResult,
        get_string: unsafe extern "system" fn(*mut c_void, *const c_char, *mut u16, u32) -> TResult,
        set_binary: unsafe extern "system" fn(*mut c_void, *const c_char, *const c_void, u32) -> TResult,
        get_binary: unsafe extern "system" fn(*mut c_void, *const c_char, *mut *const c_void, *mut u32) -> TResult,
    }
    #[repr(C)]
    pub struct AttrList {
        vtbl: *const ListVtbl,
        refs: AtomicU32,
        items: Vec<(String, Attr)>,
    }
    mod list_com {
        use super::*;
        com_object!(AttrList, ListVtbl, [IID_ATTRIBUTE_LIST]);
        pub(super) const U: FUnknownVtbl = UNK;
        pub(super) unsafe fn rel(p: *mut c_void) {
            release(p);
        }
    }
    unsafe fn key(id: *const c_char) -> Option<String> {
        if id.is_null() {
            None
        } else {
            Some(CStr::from_ptr(id).to_string_lossy().to_string())
        }
    }
    unsafe fn set(this: *mut c_void, id: *const c_char, v: Attr) -> TResult {
        let Some(k) = key(id) else { return INVALID_ARG };
        let l = &mut *(this as *mut AttrList);
        l.items.retain(|(n, _)| *n != k);
        l.items.push((k, v));
        OK
    }
    unsafe fn get(this: *mut c_void, id: *const c_char) -> Option<&'static Attr> {
        let k = key(id)?;
        let l = &*(this as *mut AttrList);
        l.items.iter().find(|(n, _)| *n == k).map(|(_, v)| &*(v as *const Attr))
    }
    unsafe extern "system" fn set_int(t: *mut c_void, id: *const c_char, v: i64) -> TResult {
        set(t, id, Attr::Int(v))
    }
    unsafe extern "system" fn get_int(t: *mut c_void, id: *const c_char, v: *mut i64) -> TResult {
        match get(t, id) {
            Some(Attr::Int(x)) => {
                *v = *x;
                OK
            }
            _ => FALSE,
        }
    }
    unsafe extern "system" fn set_float(t: *mut c_void, id: *const c_char, v: f64) -> TResult {
        set(t, id, Attr::Float(v))
    }
    unsafe extern "system" fn get_float(t: *mut c_void, id: *const c_char, v: *mut f64) -> TResult {
        match get(t, id) {
            Some(Attr::Float(x)) => {
                *v = *x;
                OK
            }
            _ => FALSE,
        }
    }
    unsafe extern "system" fn set_string(t: *mut c_void, id: *const c_char, s: *const u16) -> TResult {
        if s.is_null() {
            return INVALID_ARG;
        }
        let mut n = 0;
        while *s.add(n) != 0 {
            n += 1;
        }
        let mut v = std::slice::from_raw_parts(s, n).to_vec();
        v.push(0);
        set(t, id, Attr::Str(v))
    }
    unsafe extern "system" fn get_string(t: *mut c_void, id: *const c_char, out: *mut u16, bytes: u32) -> TResult {
        match get(t, id) {
            Some(Attr::Str(x)) if !out.is_null() && bytes >= 2 => {
                let n = x.len().min(bytes as usize / 2);
                ptr::copy_nonoverlapping(x.as_ptr(), out, n);
                *out.add(n - 1) = 0;
                OK
            }
            _ => FALSE,
        }
    }
    unsafe extern "system" fn set_binary(t: *mut c_void, id: *const c_char, d: *const c_void, n: u32) -> TResult {
        let v = if d.is_null() { Vec::new() } else { std::slice::from_raw_parts(d as *const u8, n as usize).to_vec() };
        set(t, id, Attr::Bin(v))
    }
    unsafe extern "system" fn get_binary(t: *mut c_void, id: *const c_char, d: *mut *const c_void, n: *mut u32) -> TResult {
        match get(t, id) {
            Some(Attr::Bin(x)) => {
                *d = x.as_ptr() as *const c_void;
                *n = x.len() as u32;
                OK
            }
            _ => FALSE,
        }
    }
    static LIST_VTBL: ListVtbl = ListVtbl { unk: list_com::U, set_int, get_int, set_float, get_float, set_string, get_string, set_binary, get_binary };

    #[repr(C)]
    struct MsgVtbl {
        unk: FUnknownVtbl,
        get_message_id: unsafe extern "system" fn(*mut c_void) -> *const c_char,
        set_message_id: unsafe extern "system" fn(*mut c_void, *const c_char),
        get_attributes: unsafe extern "system" fn(*mut c_void) -> *mut c_void,
    }
    #[repr(C)]
    pub struct Message {
        vtbl: *const MsgVtbl,
        refs: AtomicU32,
        id: std::ffi::CString,
        attrs: *mut AttrList,
    }
    impl Drop for Message {
        fn drop(&mut self) {
            unsafe { list_com::rel(self.attrs as *mut c_void) };
        }
    }
    mod msg_com {
        use super::*;
        com_object!(Message, MsgVtbl, [IID_MESSAGE]);
        pub(super) const U: FUnknownVtbl = UNK;
    }
    unsafe extern "system" fn get_message_id(t: *mut c_void) -> *const c_char {
        (*(t as *mut Message)).id.as_ptr()
    }
    unsafe extern "system" fn set_message_id(t: *mut c_void, id: *const c_char) {
        if !id.is_null() {
            (*(t as *mut Message)).id = CStr::from_ptr(id).to_owned();
        }
    }
    unsafe extern "system" fn get_attributes(t: *mut c_void) -> *mut c_void {
        // Not add-ref'd: the message owns its list (as in the SDK's HostMessage).
        (*(t as *mut Message)).attrs as *mut c_void
    }
    static MSG_VTBL: MsgVtbl = MsgVtbl { unk: msg_com::U, get_message_id, set_message_id, get_attributes };

    pub fn new_attr_list() -> *mut c_void {
        Box::into_raw(Box::new(AttrList { vtbl: &LIST_VTBL, refs: AtomicU32::new(1), items: Vec::new() })) as *mut c_void
    }
    pub fn new_message() -> *mut c_void {
        let attrs = new_attr_list() as *mut AttrList;
        Box::into_raw(Box::new(Message { vtbl: &MSG_VTBL, refs: AtomicU32::new(1), id: std::ffi::CString::default(), attrs })) as *mut c_void
    }
}

// ---- IHostApplication

pub mod host {
    use super::*;
    #[repr(C)]
    struct Vtbl {
        unk: FUnknownVtbl,
        get_name: unsafe extern "system" fn(*mut c_void, *mut u16) -> TResult,
        create_instance: unsafe extern "system" fn(*mut c_void, *const Tuid, *const Tuid, *mut *mut c_void) -> TResult,
    }
    #[repr(C)]
    pub struct HostApp {
        vtbl: *const Vtbl,
        refs: AtomicU32,
    }
    com_object!(HostApp, Vtbl, [IID_HOST_APPLICATION]);
    unsafe extern "system" fn get_name(_: *mut c_void, out: *mut u16) -> TResult {
        if out.is_null() {
            return INVALID_ARG;
        }
        for (i, c) in "Audemo".encode_utf16().chain(Some(0)).enumerate() {
            *out.add(i) = c;
        }
        OK
    }
    unsafe extern "system" fn create_instance(_: *mut c_void, cid: *const Tuid, iid: *const Tuid, obj: *mut *mut c_void) -> TResult {
        if obj.is_null() {
            return INVALID_ARG;
        }
        if iid_eq(cid, &IID_MESSAGE) && iid_eq(iid, &IID_MESSAGE) {
            *obj = message::new_message();
            OK
        } else if iid_eq(cid, &IID_ATTRIBUTE_LIST) && iid_eq(iid, &IID_ATTRIBUTE_LIST) {
            *obj = message::new_attr_list();
            OK
        } else {
            *obj = null_mut();
            FALSE
        }
    }
    static VTBL: Vtbl = Vtbl { unk: UNK, get_name, create_instance };

    /// The process-wide host context (never freed).
    pub fn context() -> *mut c_void {
        static HOST: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *HOST.get_or_init(|| Box::into_raw(Box::new(HostApp { vtbl: &VTBL, refs: AtomicU32::new(1) })) as usize) as *mut c_void
    }
}

// ---- IComponentHandler: edits made in the plug-in's own editor

/// Parameter edits coming from the plug-in's controller, waiting to be sent
/// to its processor.
#[derive(Default)]
pub struct EditQueue {
    pub edits: Vec<(ParamId, f64)>,
    pub restart: i32,
    pub touched: bool,
}

pub mod handler {
    use super::*;
    #[repr(C)]
    struct Vtbl {
        unk: FUnknownVtbl,
        begin_edit: unsafe extern "system" fn(*mut c_void, ParamId) -> TResult,
        perform_edit: unsafe extern "system" fn(*mut c_void, ParamId, f64) -> TResult,
        end_edit: unsafe extern "system" fn(*mut c_void, ParamId) -> TResult,
        restart_component: unsafe extern "system" fn(*mut c_void, i32) -> TResult,
    }
    #[repr(C)]
    pub struct Handler {
        vtbl: *const Vtbl,
        refs: AtomicU32,
        pub queue: Arc<Mutex<EditQueue>>,
    }
    com_object!(Handler, Vtbl, [IID_COMPONENT_HANDLER]);
    unsafe extern "system" fn begin_edit(_: *mut c_void, _: ParamId) -> TResult {
        OK
    }
    unsafe extern "system" fn perform_edit(t: *mut c_void, id: ParamId, v: f64) -> TResult {
        if let Ok(mut q) = (*(t as *mut Handler)).queue.lock() {
            q.edits.retain(|e| e.0 != id);
            q.edits.push((id, v));
            q.touched = true;
        }
        OK
    }
    unsafe extern "system" fn end_edit(_: *mut c_void, _: ParamId) -> TResult {
        OK
    }
    unsafe extern "system" fn restart_component(t: *mut c_void, flags: i32) -> TResult {
        if let Ok(mut q) = (*(t as *mut Handler)).queue.lock() {
            q.restart |= flags;
            q.touched = true;
        }
        OK
    }
    static VTBL: Vtbl = Vtbl { unk: UNK, begin_edit, perform_edit, end_edit, restart_component };
    pub fn new(queue: Arc<Mutex<EditQueue>>) -> *mut c_void {
        Box::into_raw(Box::new(Handler { vtbl: &VTBL, refs: AtomicU32::new(1), queue })) as *mut c_void
    }
    pub unsafe fn free(p: *mut c_void) {
        release(p);
    }
}

// ---- IParameterChanges / IParamValueQueue (preallocated: no allocation
// on the audio thread)

pub mod changes {
    use super::*;
    pub const MAX_PARAMS: usize = 256;
    const MAX_POINTS: usize = 16;

    #[repr(C)]
    struct QueueVtbl {
        unk: FUnknownVtbl,
        get_parameter_id: unsafe extern "system" fn(*mut c_void) -> ParamId,
        get_point_count: unsafe extern "system" fn(*mut c_void) -> i32,
        get_point: unsafe extern "system" fn(*mut c_void, i32, *mut i32, *mut f64) -> TResult,
        add_point: unsafe extern "system" fn(*mut c_void, i32, f64, *mut i32) -> TResult,
    }
    #[repr(C)]
    pub struct Queue {
        vtbl: *const QueueVtbl,
        refs: AtomicU32,
        id: ParamId,
        n: usize,
        points: [(i32, f64); MAX_POINTS],
    }
    // Queues live inside Changes and are never freed on their own.
    unsafe extern "system" fn q_qi(this: *mut c_void, iid: *const Tuid, obj: *mut *mut c_void) -> TResult {
        if iid_eq(iid, &IID_FUNKNOWN) || iid_eq(iid, &IID_PARAM_VALUE_QUEUE) {
            *obj = this;
            OK
        } else {
            *obj = null_mut();
            NO_INTERFACE
        }
    }
    unsafe extern "system" fn q_ref(_: *mut c_void) -> u32 {
        1
    }
    unsafe extern "system" fn get_parameter_id(t: *mut c_void) -> ParamId {
        (*(t as *mut Queue)).id
    }
    unsafe extern "system" fn get_point_count(t: *mut c_void) -> i32 {
        (*(t as *mut Queue)).n as i32
    }
    unsafe extern "system" fn get_point(t: *mut c_void, i: i32, off: *mut i32, v: *mut f64) -> TResult {
        let q = &*(t as *mut Queue);
        if i < 0 || i as usize >= q.n {
            return INVALID_ARG;
        }
        let (o, x) = q.points[i as usize];
        *off = o;
        *v = x;
        OK
    }
    unsafe extern "system" fn add_point(t: *mut c_void, off: i32, v: f64, idx: *mut i32) -> TResult {
        let q = &mut *(t as *mut Queue);
        if q.n >= MAX_POINTS {
            return FALSE;
        }
        q.points[q.n] = (off, v);
        if !idx.is_null() {
            *idx = q.n as i32;
        }
        q.n += 1;
        OK
    }
    static QUEUE_VTBL: QueueVtbl = QueueVtbl {
        unk: FUnknownVtbl { query_interface: q_qi, add_ref: q_ref, release: q_ref },
        get_parameter_id,
        get_point_count,
        get_point,
        add_point,
    };

    #[repr(C)]
    struct Vtbl {
        unk: FUnknownVtbl,
        get_parameter_count: unsafe extern "system" fn(*mut c_void) -> i32,
        get_parameter_data: unsafe extern "system" fn(*mut c_void, i32) -> *mut c_void,
        add_parameter_data: unsafe extern "system" fn(*mut c_void, *const ParamId, *mut i32) -> *mut c_void,
    }
    #[repr(C)]
    pub struct Changes {
        vtbl: *const Vtbl,
        refs: AtomicU32,
        n: usize,
        queues: Vec<Queue>,
    }
    unsafe extern "system" fn c_qi(this: *mut c_void, iid: *const Tuid, obj: *mut *mut c_void) -> TResult {
        if iid_eq(iid, &IID_FUNKNOWN) || iid_eq(iid, &IID_PARAMETER_CHANGES) {
            *obj = this;
            OK
        } else {
            *obj = null_mut();
            NO_INTERFACE
        }
    }
    unsafe extern "system" fn get_parameter_count(t: *mut c_void) -> i32 {
        (*(t as *mut Changes)).n as i32
    }
    unsafe extern "system" fn get_parameter_data(t: *mut c_void, i: i32) -> *mut c_void {
        let c = &mut *(t as *mut Changes);
        if i < 0 || i as usize >= c.n {
            return null_mut();
        }
        &mut c.queues[i as usize] as *mut Queue as *mut c_void
    }
    unsafe extern "system" fn add_parameter_data(t: *mut c_void, id: *const ParamId, idx: *mut i32) -> *mut c_void {
        let c = &mut *(t as *mut Changes);
        if id.is_null() {
            return null_mut();
        }
        if let Some(i) = c.queues[..c.n].iter().position(|q| q.id == *id) {
            if !idx.is_null() {
                *idx = i as i32;
            }
            return &mut c.queues[i] as *mut Queue as *mut c_void;
        }
        if c.n >= c.queues.len() {
            return null_mut();
        }
        let i = c.n;
        c.n += 1;
        c.queues[i].id = *id;
        c.queues[i].n = 0;
        if !idx.is_null() {
            *idx = i as i32;
        }
        &mut c.queues[i] as *mut Queue as *mut c_void
    }
    static VTBL: Vtbl = Vtbl {
        unk: FUnknownVtbl { query_interface: c_qi, add_ref: q_ref, release: q_ref },
        get_parameter_count,
        get_parameter_data,
        add_parameter_data,
    };

    impl Changes {
        pub fn new() -> Box<Changes> {
            let queues = (0..MAX_PARAMS)
                .map(|_| Queue { vtbl: &QUEUE_VTBL, refs: AtomicU32::new(1), id: 0, n: 0, points: [(0, 0.0); MAX_POINTS] })
                .collect();
            Box::new(Changes { vtbl: &VTBL, refs: AtomicU32::new(1), n: 0, queues })
        }
        pub fn clear(&mut self) {
            self.n = 0;
        }
        pub fn is_empty(&self) -> bool {
            self.n == 0
        }
        /// Set a parameter's value at the start of the next block.
        pub fn set(&mut self, id: ParamId, v: f64) {
            let p = self as *mut Changes as *mut c_void;
            let mut idx = 0;
            let q = unsafe { add_parameter_data(p, &id, &mut idx) };
            if !q.is_null() {
                let q = unsafe { &mut *(q as *mut Queue) };
                q.n = 1;
                q.points[0] = (0, v);
            }
        }
        /// (id, last value) of each parameter the plug-in reported.
        pub fn values(&self) -> impl Iterator<Item = (ParamId, f64)> + '_ {
            self.queues[..self.n].iter().filter(|q| q.n > 0).map(|q| (q.id, q.points[q.n - 1].1))
        }
        pub fn as_ptr(&mut self) -> *mut c_void {
            self as *mut Changes as *mut c_void
        }
    }
}

// ------------------------------------------------------------------ module loading

type GetFactoryFn = unsafe extern "system" fn() -> *mut c_void;

#[cfg(unix)]
mod dl {
    use std::ffi::{c_char, c_int, c_void};
    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    extern "C" {
        pub fn dlopen(file: *const c_char, mode: c_int) -> *mut c_void;
        pub fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        pub fn dlclose(handle: *mut c_void) -> c_int;
        pub fn dlerror() -> *const c_char;
    }
    pub const RTLD_NOW: c_int = 2;
    #[cfg(target_os = "linux")]
    pub const RTLD_LOCAL: c_int = 0;
    #[cfg(target_os = "macos")]
    pub const RTLD_LOCAL: c_int = 4;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub const RTLD_LOCAL: c_int = 0;
}

#[cfg(windows)]
mod dl {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        pub fn LoadLibraryW(name: *const u16) -> *mut c_void;
        pub fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
        pub fn FreeLibrary(module: *mut c_void) -> i32;
        pub fn GetLastError() -> u32;
    }
}

#[cfg(target_os = "macos")]
mod cf {
    use std::ffi::{c_char, c_void};
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFURLCreateFromFileSystemRepresentation(alloc: *const c_void, buf: *const u8, len: isize, dir: u8) -> *mut c_void;
        pub fn CFBundleCreate(alloc: *const c_void, url: *mut c_void) -> *mut c_void;
        pub fn CFBundleLoadExecutable(bundle: *mut c_void) -> u8;
        pub fn CFBundleGetFunctionPointerForName(bundle: *mut c_void, name: *mut c_void) -> *mut c_void;
        pub fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, enc: u32) -> *mut c_void;
        pub fn CFRelease(obj: *const c_void);
    }
    pub const UTF8: u32 = 0x0800_0100;
}

/// A loaded plug-in binary (shared between its instances).
pub struct Module {
    pub path: PathBuf,
    handle: *mut c_void,
    pub factory: ComPtr<PluginFactoryVtbl>,
    exit: Option<unsafe extern "C" fn() -> bool>,
}

unsafe impl Send for Module {}
unsafe impl Sync for Module {}

impl Drop for Module {
    fn drop(&mut self) {
        unsafe {
            // Release the factory before the module's exit function runs.
            let f = std::mem::replace(&mut self.factory.ptr, null_mut());
            if !f.is_null() {
                let unk = &**(f as *mut *const FUnknownVtbl);
                (unk.release)(f);
            }
            if let Some(exit) = self.exit {
                exit();
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            if !self.handle.is_null() {
                dl::dlclose(self.handle);
            }
            #[cfg(windows)]
            if !self.handle.is_null() {
                dl::FreeLibrary(self.handle);
            }
            #[cfg(target_os = "macos")]
            if !self.handle.is_null() {
                cf::CFRelease(self.handle);
            }
        }
    }
}

/// The binary inside a `.vst3` bundle for this platform.
pub fn binary_path(bundle: &Path) -> PathBuf {
    if bundle.is_file() {
        return bundle.to_path_buf();
    }
    let stem = bundle.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
    if cfg!(windows) {
        let a = if cfg!(target_arch = "aarch64") { "arm64-win" } else { "x86_64-win" };
        bundle.join("Contents").join(a).join(format!("{stem}.vst3"))
    } else if cfg!(target_os = "macos") {
        bundle.join("Contents").join("MacOS").join(stem)
    } else {
        bundle.join("Contents").join(format!("{arch}-linux")).join(format!("{stem}.so"))
    }
}

impl Module {
    pub fn load(bundle: &Path) -> Result<Arc<Module>, String> {
        unsafe { Self::load_inner(bundle) }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    unsafe fn load_inner(bundle: &Path) -> Result<Arc<Module>, String> {
        use std::os::unix::ffi::OsStrExt;
        let bin = binary_path(bundle);
        let c = std::ffi::CString::new(bin.as_os_str().as_bytes()).map_err(|_| "bad path")?;
        let h = dl::dlopen(c.as_ptr(), dl::RTLD_NOW | dl::RTLD_LOCAL);
        if h.is_null() {
            let e = dl::dlerror();
            let msg = if e.is_null() { "unknown error".into() } else { CStr::from_ptr(e).to_string_lossy().to_string() };
            return Err(format!("Couldn't load {}: {msg}", bin.display()));
        }
        let sym = |n: &[u8]| dl::dlsym(h, n.as_ptr() as *const c_char);
        let entry = sym(b"ModuleEntry\0");
        if !entry.is_null() {
            let f: unsafe extern "C" fn(*mut c_void) -> bool = std::mem::transmute(entry);
            if !f(h) {
                dl::dlclose(h);
                return Err("The plug-in refused to start (ModuleEntry).".into());
            }
        }
        let exit = sym(b"ModuleExit\0");
        let exit: Option<unsafe extern "C" fn() -> bool> = if exit.is_null() { None } else { Some(std::mem::transmute(exit)) };
        Self::finish(bundle, h, sym(b"GetPluginFactory\0"), exit)
    }

    #[cfg(windows)]
    unsafe fn load_inner(bundle: &Path) -> Result<Arc<Module>, String> {
        use std::os::windows::ffi::OsStrExt;
        let bin = binary_path(bundle);
        let wide: Vec<u16> = bin.as_os_str().encode_wide().chain(Some(0)).collect();
        let h = dl::LoadLibraryW(wide.as_ptr());
        if h.is_null() {
            return Err(format!("Couldn't load {} (error {})", bin.display(), dl::GetLastError()));
        }
        let sym = |n: &[u8]| dl::GetProcAddress(h, n.as_ptr());
        let init = sym(b"InitDll\0");
        if !init.is_null() {
            let f: unsafe extern "C" fn() -> bool = std::mem::transmute(init);
            if !f() {
                dl::FreeLibrary(h);
                return Err("The plug-in refused to start (InitDll).".into());
            }
        }
        let exit = sym(b"ExitDll\0");
        let exit: Option<unsafe extern "C" fn() -> bool> = if exit.is_null() { None } else { Some(std::mem::transmute(exit)) };
        Self::finish(bundle, h, sym(b"GetPluginFactory\0"), exit)
    }

    #[cfg(target_os = "macos")]
    unsafe fn load_inner(bundle: &Path) -> Result<Arc<Module>, String> {
        use std::os::unix::ffi::OsStrExt;
        let bytes = bundle.as_os_str().as_bytes();
        let url = cf::CFURLCreateFromFileSystemRepresentation(ptr::null(), bytes.as_ptr(), bytes.len() as isize, 1);
        if url.is_null() {
            return Err("Bad plug-in path.".into());
        }
        let b = cf::CFBundleCreate(ptr::null(), url);
        cf::CFRelease(url);
        if b.is_null() {
            return Err(format!("{} isn't a bundle.", bundle.display()));
        }
        if cf::CFBundleLoadExecutable(b) == 0 {
            cf::CFRelease(b);
            return Err(format!("Couldn't load {} (wrong architecture?)", bundle.display()));
        }
        let sym = |n: &[u8]| {
            let s = cf::CFStringCreateWithCString(ptr::null(), n.as_ptr() as *const c_char, cf::UTF8);
            let p = cf::CFBundleGetFunctionPointerForName(b, s);
            cf::CFRelease(s);
            p
        };
        let entry = sym(b"bundleEntry\0");
        if !entry.is_null() {
            let f: unsafe extern "C" fn(*mut c_void) -> bool = std::mem::transmute(entry);
            if !f(b) {
                cf::CFRelease(b);
                return Err("The plug-in refused to start (bundleEntry).".into());
            }
        }
        let exit = sym(b"bundleExit\0");
        let exit: Option<unsafe extern "C" fn() -> bool> = if exit.is_null() { None } else { Some(std::mem::transmute(exit)) };
        Self::finish(bundle, b, sym(b"GetPluginFactory\0"), exit)
    }

    unsafe fn finish(bundle: &Path, handle: *mut c_void, get: *mut c_void, exit: Option<unsafe extern "C" fn() -> bool>) -> Result<Arc<Module>, String> {
        if get.is_null() {
            if let Some(e) = exit {
                e();
            }
            return Err("Not a VST3 plug-in (no GetPluginFactory).".into());
        }
        let get: GetFactoryFn = std::mem::transmute(get);
        let Some(factory) = ComPtr::from_raw(get()) else {
            if let Some(e) = exit {
                e();
            }
            return Err("The plug-in returned no factory.".into());
        };
        Ok(Arc::new(Module { path: bundle.to_path_buf(), handle, factory, exit }))
    }

    /// Effect classes in this module: (class id, name, vendor, sub-categories, version).
    pub fn classes(&self) -> Vec<ClassInfo> {
        let f = &self.factory;
        let n = unsafe { (f.vtbl().count_classes)(f.ptr) };
        let f2: Option<ComPtr<PluginFactoryVtbl>> = f.query(&IID_PLUGIN_FACTORY2);
        let mut fi: PFactoryInfo = unsafe { std::mem::zeroed() };
        let vendor = if unsafe { (f.vtbl().get_factory_info)(f.ptr, &mut fi) } == OK { cstr(&fi.vendor) } else { String::new() };
        let mut out = Vec::new();
        for i in 0..n.max(0) {
            if let Some(f2) = &f2 {
                let mut c: PClassInfo2 = unsafe { std::mem::zeroed() };
                if unsafe { (f2.vtbl().get_class_info2)(f2.ptr, i, &mut c) } == OK {
                    let v = cstr(&c.vendor);
                    out.push(ClassInfo {
                        cid: c.cid,
                        category: cstr(&c.category),
                        name: cstr(&c.name),
                        vendor: if v.is_empty() { vendor.clone() } else { v },
                        sub_categories: cstr(&c.sub_categories),
                        version: cstr(&c.version),
                    });
                    continue;
                }
            }
            let mut c: PClassInfo = unsafe { std::mem::zeroed() };
            if unsafe { (f.vtbl().get_class_info)(f.ptr, i, &mut c) } == OK {
                out.push(ClassInfo { cid: c.cid, category: cstr(&c.category), name: cstr(&c.name), vendor: vendor.clone(), sub_categories: String::new(), version: String::new() });
            }
        }
        out
    }

    fn create<V>(&self, cid: &Tuid, iid: &Tuid) -> Option<ComPtr<V>> {
        let mut out = null_mut();
        let r = unsafe { (self.factory.vtbl().create_instance)(self.factory.ptr, cid.as_ptr() as *const c_char, iid.as_ptr() as *const c_char, &mut out) };
        if r == OK {
            unsafe { ComPtr::from_raw(out) }
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClassInfo {
    pub cid: Tuid,
    pub category: String,
    pub name: String,
    pub vendor: String,
    pub sub_categories: String,
    pub version: String,
}

impl ClassInfo {
    /// An audio effect (not an instrument or a controller class).
    pub fn is_effect(&self) -> bool {
        self.category == "Audio Module Class" && !self.sub_categories.split('|').any(|s| s == "Instrument")
    }
}

// ------------------------------------------------------------------ instances

#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub id: ParamId,
    pub title: String,
    pub units: String,
    pub steps: i32,
    pub default: f64,
    pub flags: i32,
}

/// A running plug-in: component, processor and (if any) edit controller.
pub struct Instance {
    pub name: String,
    pub cid: Tuid,
    component: ComPtr<ComponentVtbl>,
    processor: ComPtr<AudioProcessorVtbl>,
    controller: Option<ComPtr<EditControllerVtbl>>,
    /// Separate controller object (connected through IConnectionPoint).
    split: bool,
    handler: *mut c_void,
    pub edits: Arc<Mutex<EditQueue>>,
    pub channels: usize,
    pub sample_rate: f64,
    pub max_block: usize,
    active: bool,
    in_changes: Box<changes::Changes>,
    out_changes: Box<changes::Changes>,
    context: Box<ProcessContext>,
    pos: i64,
    in_bufs: Vec<Vec<f32>>,
    out_bufs: Vec<Vec<f32>>,
    in_ptrs: Vec<*mut f32>,
    out_ptrs: Vec<*mut f32>,
    /// Channels on the plug-in's main input and output buses.
    in_ch: usize,
    out_ch: usize,
    mode: i32,
    pub latency: usize,
    n_inputs: i32,
    /// Declared last so the binary stays loaded until everything above is released.
    pub module: Arc<Module>,
}

unsafe impl Send for Instance {}

impl Instance {
    /// Create, initialise and activate class `cid` from `module` for
    /// `channels` (1 or 2) channels of audio.
    pub fn new(module: Arc<Module>, cid: &Tuid, sample_rate: f64, channels: usize, max_block: usize, offline: bool) -> Result<Instance, String> {
        let name = module.classes().into_iter().find(|c| &c.cid == cid).map(|c| c.name).unwrap_or_else(|| "Plug-in".into());
        let component: ComPtr<ComponentVtbl> = module.create(cid, &IID_COMPONENT).ok_or_else(|| format!("{name}: couldn't create the plug-in."))?;
        let host = host::context();
        if unsafe { (component.vtbl().initialize)(component.ptr, host) } != OK {
            return Err(format!("{name}: the plug-in failed to initialise."));
        }
        let processor: ComPtr<AudioProcessorVtbl> = match component.query(&IID_AUDIO_PROCESSOR) {
            Some(p) => p,
            None => {
                unsafe { (component.vtbl().terminate)(component.ptr) };
                return Err(format!("{name}: not an audio effect."));
            }
        };
        // Edit controller: the component itself, or a separate class.
        let mut split = false;
        let mut controller: Option<ComPtr<EditControllerVtbl>> = component.query(&IID_EDIT_CONTROLLER);
        if controller.is_none() {
            let mut ccid: Tuid = [0; 16];
            if unsafe { (component.vtbl().get_controller_class_id)(component.ptr, &mut ccid) } == OK && ccid != [0; 16] {
                if let Some(c) = module.create::<EditControllerVtbl>(&ccid, &IID_EDIT_CONTROLLER) {
                    if unsafe { (c.vtbl().initialize)(c.ptr, host) } == OK {
                        controller = Some(c);
                        split = true;
                    }
                }
            }
        }
        let edits = Arc::new(Mutex::new(EditQueue::default()));
        let handler = handler::new(edits.clone());
        let mut inst = Instance {
            module,
            name,
            cid: *cid,
            component,
            processor,
            controller,
            split,
            handler,
            edits,
            channels: channels.clamp(1, 2),
            sample_rate,
            max_block: max_block.max(32),
            active: false,
            in_changes: changes::Changes::new(),
            out_changes: changes::Changes::new(),
            context: Box::new(unsafe { std::mem::zeroed() }),
            pos: 0,
            in_bufs: Vec::new(),
            out_bufs: Vec::new(),
            in_ptrs: Vec::new(),
            out_ptrs: Vec::new(),
            in_ch: 0,
            out_ch: 0,
            mode: MODE_REALTIME,
            latency: 0,
            n_inputs: 0,
        };
        if inst.split {
            inst.connect(true);
        }
        if let Some(c) = &inst.controller {
            unsafe { (c.vtbl().set_component_handler)(c.ptr, inst.handler) };
        }
        inst.sync_controller();
        inst.configure(offline)?;
        Ok(inst)
    }

    fn connect(&self, on: bool) {
        let Some(ctrl) = &self.controller else { return };
        let a: Option<ComPtr<ConnectionPointVtbl>> = self.component.query(&IID_CONNECTION_POINT);
        let b: Option<ComPtr<ConnectionPointVtbl>> = ctrl.query(&IID_CONNECTION_POINT);
        if let (Some(a), Some(b)) = (a, b) {
            unsafe {
                if on {
                    (a.vtbl().connect)(a.ptr, b.ptr);
                    (b.vtbl().connect)(b.ptr, a.ptr);
                } else {
                    (a.vtbl().disconnect)(a.ptr, b.ptr);
                    (b.vtbl().disconnect)(b.ptr, a.ptr);
                }
            }
        }
    }

    /// Give a separate controller the component's current state.
    fn sync_controller(&self) {
        if !self.split {
            return;
        }
        let Some(ctrl) = &self.controller else { return };
        let s = stream::new(Vec::new());
        unsafe {
            if (self.component.vtbl().get_state)(self.component.ptr, s as *mut c_void) == OK {
                let data = stream::take(s);
                let s2 = stream::new(data);
                (ctrl.vtbl().set_component_state)(ctrl.ptr, s2 as *mut c_void);
                stream::free(s2);
            } else {
                stream::free(s);
            }
        }
    }

    fn configure(&mut self, offline: bool) -> Result<(), String> {
        let c = &self.component;
        let p = &self.processor;
        unsafe {
            let n_in = (c.vtbl().get_bus_count)(c.ptr, MEDIA_AUDIO, DIR_IN);
            let n_out = (c.vtbl().get_bus_count)(c.ptr, MEDIA_AUDIO, DIR_OUT);
            self.n_inputs = n_in;
            if n_out < 1 {
                return Err(format!("{}: has no audio output.", self.name));
            }
            // Only the main buses are used.
            for i in 0..n_in {
                (c.vtbl().activate_bus)(c.ptr, MEDIA_AUDIO, DIR_IN, i, (i == 0) as u8);
            }
            for i in 0..n_out {
                (c.vtbl().activate_bus)(c.ptr, MEDIA_AUDIO, DIR_OUT, i, (i == 0) as u8);
            }
            for dir in [DIR_IN, DIR_OUT] {
                for i in 0..(c.vtbl().get_bus_count)(c.ptr, MEDIA_EVENT, dir) {
                    (c.vtbl().activate_bus)(c.ptr, MEDIA_EVENT, dir, i, 0);
                }
            }
            let want = if self.channels == 1 { SPEAKER_MONO } else { SPEAKER_STEREO };
            let mut ins = vec![want; n_in.max(0) as usize];
            let mut outs = vec![want; n_out as usize];
            // Secondary buses keep whatever the plug-in wants.
            for (i, a) in ins.iter_mut().enumerate().skip(1) {
                (p.vtbl().get_bus_arrangement)(p.ptr, DIR_IN, i as i32, a);
            }
            for (i, a) in outs.iter_mut().enumerate().skip(1) {
                (p.vtbl().get_bus_arrangement)(p.ptr, DIR_OUT, i as i32, a);
            }
            // If the plug-in refuses, it keeps its own layout and the host
            // maps channels to it.
            (p.vtbl().set_bus_arrangements)(p.ptr, ins.as_mut_ptr(), n_in, outs.as_mut_ptr(), n_out);
            let count = |dir: i32, fallback: usize| {
                let mut a = 0u64;
                if (p.vtbl().get_bus_arrangement)(p.ptr, dir, 0, &mut a) == OK && a != 0 {
                    (a.count_ones() as usize).min(32)
                } else {
                    fallback
                }
            };
            self.in_ch = if n_in > 0 { count(DIR_IN, self.channels) } else { 0 };
            self.out_ch = count(DIR_OUT, self.channels);
            if (p.vtbl().can_process_sample_size)(p.ptr, SAMPLE_32) != OK {
                return Err(format!("{}: doesn't support 32-bit float audio.", self.name));
            }
            let mut setup = ProcessSetup {
                process_mode: if offline { MODE_OFFLINE } else { MODE_REALTIME },
                sample_size: SAMPLE_32,
                max_block: self.max_block as i32,
                sample_rate: self.sample_rate,
            };
            if (p.vtbl().setup_processing)(p.ptr, &mut setup) != OK {
                return Err(format!("{}: couldn't set up processing at {} Hz.", self.name, self.sample_rate));
            }
            if (c.vtbl().set_active)(c.ptr, 1) != OK {
                return Err(format!("{}: couldn't be activated.", self.name));
            }
            (p.vtbl().set_processing)(p.ptr, 1);
            self.active = true;
            self.latency = (p.vtbl().get_latency_samples)(p.ptr) as usize;
        }
        self.mode = if offline { MODE_OFFLINE } else { MODE_REALTIME };
        // Buffers for every channel the buses have (at least two each).
        self.in_bufs = vec![vec![0.0; self.max_block]; self.in_ch.max(2)];
        self.out_bufs = vec![vec![0.0; self.max_block]; self.out_ch.max(2)];
        self.in_ptrs = self.in_bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        self.out_ptrs = self.out_bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        let ctx = &mut self.context;
        ctx.sample_rate = self.sample_rate;
        ctx.tempo = 120.0;
        ctx.time_sig_numerator = 4;
        ctx.time_sig_denominator = 4;
        ctx.state = 1 << 10 | 1 << 13; // tempo valid, time signature valid
        Ok(())
    }

    fn deactivate(&mut self) {
        if self.active {
            unsafe {
                (self.processor.vtbl().set_processing)(self.processor.ptr, 0);
                (self.component.vtbl().set_active)(self.component.ptr, 0);
            }
            self.active = false;
        }
    }

    /// Restart processing (after a latency or bus change, or a sample-rate change).
    pub fn reconfigure(&mut self, sample_rate: f64, offline: bool) -> Result<(), String> {
        self.deactivate();
        self.sample_rate = sample_rate;
        self.configure(offline)
    }

    /// Clear the plug-in's internal state (tails, delay lines).
    pub fn reset(&mut self) {
        if self.active {
            unsafe {
                (self.processor.vtbl().set_processing)(self.processor.ptr, 0);
                (self.component.vtbl().set_active)(self.component.ptr, 0);
                (self.component.vtbl().set_active)(self.component.ptr, 1);
                (self.processor.vtbl().set_processing)(self.processor.ptr, 1);
            }
        }
        self.pos = 0;
    }

    pub fn params(&self) -> Vec<ParamInfo> {
        let Some(c) = &self.controller else { return Vec::new() };
        let n = unsafe { (c.vtbl().get_parameter_count)(c.ptr) };
        let mut out = Vec::with_capacity(n.max(0) as usize);
        for i in 0..n.max(0) {
            let mut pi: ParameterInfo = unsafe { std::mem::zeroed() };
            if unsafe { (c.vtbl().get_parameter_info)(c.ptr, i, &mut pi) } == OK {
                out.push(ParamInfo { id: pi.id, title: u16str(&pi.title), units: u16str(&pi.units), steps: pi.step_count, default: pi.default_normalized, flags: pi.flags });
            }
        }
        out
    }

    pub fn refresh_latency(&mut self) {
        self.latency = unsafe { (self.processor.vtbl().get_latency_samples)(self.processor.ptr) } as usize;
    }

    pub fn get_param(&self, id: ParamId) -> f64 {
        self.controller.as_ref().map(|c| unsafe { (c.vtbl().get_param_normalized)(c.ptr, id) }).unwrap_or(0.0)
    }

    pub fn param_text(&self, id: ParamId, v: f64) -> String {
        let Some(c) = &self.controller else { return format!("{v:.3}") };
        let mut buf = [0u16; 128];
        if unsafe { (c.vtbl().get_param_string_by_value)(c.ptr, id, v, buf.as_mut_ptr()) } == OK {
            u16str(&buf)
        } else {
            format!("{v:.3}")
        }
    }

    /// Change a parameter from the host: the controller shows it and the
    /// processor gets it with the next block.
    pub fn set_param(&mut self, id: ParamId, v: f64) {
        let v = v.clamp(0.0, 1.0);
        if let Some(c) = &self.controller {
            unsafe { (c.vtbl().set_param_normalized)(c.ptr, id, v) };
        }
        self.in_changes.set(id, v);
    }

    /// Move parameter edits made in the plug-in's editor to the processor.
    /// Returns the restart flags the plug-in asked for.
    pub fn take_edits(&mut self) -> i32 {
        let Ok(mut q) = self.edits.try_lock() else { return 0 };
        for (id, v) in q.edits.drain(..) {
            self.in_changes.set(id, v);
        }
        q.touched = false;
        std::mem::take(&mut q.restart)
    }

    /// Let the processor take pending parameter changes without audio.
    pub fn flush_params(&mut self) {
        self.take_edits();
        if self.in_changes.is_empty() {
            return;
        }
        let mut data = ProcessData {
            process_mode: self.mode,
            sample_size: SAMPLE_32,
            num_samples: 0,
            num_inputs: 0,
            num_outputs: 0,
            inputs: null_mut(),
            outputs: null_mut(),
            input_params: self.in_changes.as_ptr(),
            output_params: null_mut(),
            input_events: null_mut(),
            output_events: null_mut(),
            context: null_mut(),
        };
        unsafe { (self.processor.vtbl().process)(self.processor.ptr, &mut data) };
        self.in_changes.clear();
    }

    /// Process `l`/`r` in place (`r` is ignored for a mono instance).
    /// Allocation-free; safe on the audio thread.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.take_edits();
        let total = l.len().min(r.len());
        let mut off = 0;
        while off < total {
            let n = (total - off).min(self.max_block);
            self.process_block(&mut l[off..off + n], &mut r[off..off + n]);
            off += n;
        }
    }

    fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        // Inputs: L/R on the first two channels (the mid signal for a mono
        // bus), silence on any others.
        for (c, b) in self.in_bufs.iter_mut().enumerate() {
            let b = &mut b[..n];
            match (self.in_ch, c) {
                (1, 0) => {
                    for i in 0..n {
                        b[i] = 0.5 * (l[i] + r[i]);
                    }
                }
                (_, 0) => b.copy_from_slice(l),
                (_, 1) => b.copy_from_slice(r),
                _ => b.fill(0.0),
            }
        }
        let mut input = AudioBusBuffers { num_channels: self.in_ch as i32, silence_flags: 0, buffers: self.in_ptrs.as_mut_ptr() };
        let mut output = AudioBusBuffers { num_channels: self.out_ch as i32, silence_flags: 0, buffers: self.out_ptrs.as_mut_ptr() };
        self.context.project_time_samples = self.pos;
        self.context.continous_time_samples = self.pos;
        self.out_changes.clear();
        let has_in = self.n_inputs > 0 && self.in_ch > 0;
        let mut data = ProcessData {
            process_mode: self.mode,
            sample_size: SAMPLE_32,
            num_samples: n as i32,
            num_inputs: has_in as i32,
            num_outputs: 1,
            inputs: if has_in { &mut input } else { null_mut() },
            outputs: &mut output,
            input_params: self.in_changes.as_ptr(),
            output_params: self.out_changes.as_ptr(),
            input_events: null_mut(),
            output_events: null_mut(),
            context: &mut *self.context,
        };
        let ok = unsafe { (self.processor.vtbl().process)(self.processor.ptr, &mut data) } == OK;
        self.in_changes.clear();
        self.pos += n as i64;
        if !ok {
            return;
        }
        if self.out_ch == 1 {
            l.copy_from_slice(&self.out_bufs[0][..n]);
            r.copy_from_slice(&self.out_bufs[0][..n]);
        } else {
            l.copy_from_slice(&self.out_bufs[0][..n]);
            r.copy_from_slice(&self.out_bufs[1][..n]);
        }
    }

    /// Parameter values the processor reported in the last block.
    pub fn output_params(&self) -> Vec<(ParamId, f64)> {
        self.out_changes.values().collect()
    }

    /// Plug-in state: component and controller chunks.
    pub fn get_state(&mut self) -> (Vec<u8>, Vec<u8>) {
        self.flush_params();
        let mut comp = Vec::new();
        let mut ctrl = Vec::new();
        unsafe {
            let s = stream::new(Vec::new());
            if (self.component.vtbl().get_state)(self.component.ptr, s as *mut c_void) == OK {
                comp = stream::take(s);
            } else {
                stream::free(s);
            }
            if self.split {
                if let Some(c) = &self.controller {
                    let s = stream::new(Vec::new());
                    if (c.vtbl().get_state)(c.ptr, s as *mut c_void) == OK {
                        ctrl = stream::take(s);
                    } else {
                        stream::free(s);
                    }
                }
            }
        }
        (comp, ctrl)
    }

    pub fn set_state(&mut self, comp: &[u8], ctrl: &[u8]) -> bool {
        let mut ok = true;
        unsafe {
            if !comp.is_empty() {
                let s = stream::new(comp.to_vec());
                ok &= (self.component.vtbl().set_state)(self.component.ptr, s as *mut c_void) == OK;
                stream::free(s);
                if self.split {
                    if let Some(c) = &self.controller {
                        let s = stream::new(comp.to_vec());
                        (c.vtbl().set_component_state)(c.ptr, s as *mut c_void);
                        stream::free(s);
                    }
                }
            }
            if self.split && !ctrl.is_empty() {
                if let Some(c) = &self.controller {
                    let s = stream::new(ctrl.to_vec());
                    (c.vtbl().set_state)(c.ptr, s as *mut c_void);
                    stream::free(s);
                }
            }
            self.latency = (self.processor.vtbl().get_latency_samples)(self.processor.ptr) as usize;
        }
        ok
    }

    pub fn tail_samples(&self) -> u32 {
        unsafe { (self.processor.vtbl().get_tail_samples)(self.processor.ptr) }
    }

    pub fn has_editor_api(&self) -> bool {
        self.controller.is_some()
    }

    /// The edit controller (for the plug-in's own editor window).
    pub fn controller(&self) -> Option<&ComPtr<EditControllerVtbl>> {
        self.controller.as_ref()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.deactivate();
        unsafe {
            if let Some(c) = &self.controller {
                (c.vtbl().set_component_handler)(c.ptr, null_mut());
            }
            if self.split {
                self.connect(false);
                if let Some(c) = &self.controller {
                    (c.vtbl().terminate)(c.ptr);
                }
            }
            (self.component.vtbl().terminate)(self.component.ptr);
            handler::free(self.handler);
        }
    }
}
