//! Recording what other apps play (macOS 14.2 and later), through Core
//! Audio process taps: a tap on chosen apps (or on everything except
//! Audemo) is wrapped in a private aggregate device, whose input is read
//! like a microphone's. macOS asks once for permission (Privacy & Security >
//! Screen & System Audio Recording); until it's given, the tap is silent.
//!
//! The tap API is looked up at run time, so Audemo still runs (without this
//! source) on older macOS.

#![allow(non_upper_case_globals, non_snake_case, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_void, CStr};
use std::ptr::null_mut;

type Id = *mut c_void;
type Sel = *mut c_void;
type CFRef = *const c_void;
type OSStatus = i32;
type AudioObjectID = u32;

const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32)
}

const kAudioObjectSystemObject: AudioObjectID = 1;
const kScopeGlobal: u32 = fourcc(b"glob");
const kScopeInput: u32 = fourcc(b"inpt");
const kElementMain: u32 = 0;
const kAudioHardwarePropertyProcessObjectList: u32 = fourcc(b"prs#");
const kAudioHardwarePropertyTranslatePIDToProcessObject: u32 = fourcc(b"id2p");
const kAudioHardwarePropertyDefaultOutputDevice: u32 = fourcc(b"dOut");
const kAudioProcessPropertyPID: u32 = fourcc(b"ppid");
const kAudioProcessPropertyBundleID: u32 = fourcc(b"pbid");
const kAudioProcessPropertyIsRunningOutput: u32 = fourcc(b"piro");
const kAudioDevicePropertyDeviceUID: u32 = fourcc(b"uid ");
const kAudioDevicePropertyStreamConfiguration: u32 = fourcc(b"slay");
const kAudioTapPropertyUID: u32 = fourcc(b"tuid");
const kAudioTapPropertyFormat: u32 = fourcc(b"tfmt");
const kCFStringEncodingUTF8: u32 = 0x0800_0100;
const kCFNumberSInt32Type: isize = 3;

#[repr(C)]
#[derive(Clone, Copy)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct StreamDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    channels: u32,
    bytes: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    count: u32,
    buffers: [AudioBuffer; 1],
}

type IOProc = unsafe extern "C" fn(AudioObjectID, *const c_void, *const AudioBufferList, *const c_void, *mut AudioBufferList, *const c_void, *mut c_void) -> OSStatus;

#[cfg_attr(target_os = "macos", link(name = "CoreAudio", kind = "framework"))]
extern "C" {
    fn AudioObjectGetPropertyDataSize(id: AudioObjectID, addr: *const PropertyAddress, qsize: u32, qual: *const c_void, size: *mut u32) -> OSStatus;
    fn AudioObjectGetPropertyData(id: AudioObjectID, addr: *const PropertyAddress, qsize: u32, qual: *const c_void, size: *mut u32, data: *mut c_void) -> OSStatus;
    fn AudioHardwareCreateAggregateDevice(desc: CFRef, out: *mut AudioObjectID) -> OSStatus;
    fn AudioHardwareDestroyAggregateDevice(id: AudioObjectID) -> OSStatus;
    fn AudioDeviceCreateIOProcID(dev: AudioObjectID, proc_: IOProc, client: *mut c_void, out: *mut *mut c_void) -> OSStatus;
    fn AudioDeviceDestroyIOProcID(dev: AudioObjectID, id: *mut c_void) -> OSStatus;
    fn AudioDeviceStart(dev: AudioObjectID, id: *mut c_void) -> OSStatus;
    fn AudioDeviceStop(dev: AudioObjectID, id: *mut c_void) -> OSStatus;
}

#[cfg_attr(target_os = "macos", link(name = "CoreFoundation", kind = "framework"))]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFTypeArrayCallBacks: c_void;
    static kCFBooleanTrue: CFRef;
    fn CFStringCreateWithCString(alloc: CFRef, s: *const c_char, enc: u32) -> CFRef;
    fn CFStringGetCString(s: CFRef, buf: *mut c_char, size: isize, enc: u32) -> u8;
    fn CFNumberCreate(alloc: CFRef, kind: isize, value: *const c_void) -> CFRef;
    fn CFDictionaryCreate(alloc: CFRef, keys: *const CFRef, values: *const CFRef, n: isize, kcb: *const c_void, vcb: *const c_void) -> CFRef;
    fn CFArrayCreate(alloc: CFRef, values: *const CFRef, n: isize, cb: *const c_void) -> CFRef;
    fn CFRelease(r: CFRef);
}

#[cfg_attr(target_os = "macos", link(name = "objc"))]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
}

#[cfg_attr(target_os = "macos", link(name = "AppKit", kind = "framework"))]
extern "C" {}

fn sel(name: &[u8]) -> Sel {
    unsafe { sel_registerName(name.as_ptr() as *const c_char) }
}

fn class(name: &[u8]) -> Id {
    unsafe { objc_getClass(name.as_ptr() as *const c_char) }
}

unsafe fn send0(o: Id, s: &[u8]) -> Id {
    let f: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(o, sel(s))
}

unsafe fn send_id(o: Id, s: &[u8], a: Id) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, Id) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(o, sel(s), a)
}

unsafe fn send_i32(o: Id, s: &[u8], a: i32) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, i32) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(o, sel(s), a)
}

unsafe fn send_usize(o: Id, s: &[u8], a: usize) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, usize) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(o, sel(s), a)
}

unsafe fn send_count(o: Id) -> usize {
    let f: unsafe extern "C" fn(Id, Sel) -> usize = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(o, sel(b"count\0"))
}

unsafe fn ns_string(o: Id) -> Option<String> {
    if o.is_null() {
        return None;
    }
    let f: unsafe extern "C" fn(Id, Sel) -> *const c_char = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    let p = f(o, sel(b"UTF8String\0"));
    (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
}

unsafe fn cf_string(s: &str) -> CFRef {
    let c = std::ffi::CString::new(s).unwrap_or_default();
    CFStringCreateWithCString(null_mut(), c.as_ptr(), kCFStringEncodingUTF8)
}

unsafe fn cf_to_string(s: CFRef) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let mut buf = vec![0 as c_char; 1024];
    if CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, kCFStringEncodingUTF8) == 0 {
        return None;
    }
    Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
}

unsafe fn cf_i32(v: i32) -> CFRef {
    CFNumberCreate(null_mut(), kCFNumberSInt32Type, &v as *const i32 as *const c_void)
}

/// A dictionary from (key, value) pairs; takes ownership of the values.
unsafe fn cf_dict(pairs: &[(&str, CFRef)]) -> CFRef {
    let keys: Vec<CFRef> = pairs.iter().map(|(k, _)| cf_string(k)).collect();
    let vals: Vec<CFRef> = pairs.iter().map(|(_, v)| *v).collect();
    let d = CFDictionaryCreate(null_mut(), keys.as_ptr(), vals.as_ptr(), keys.len() as isize, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    for k in keys {
        CFRelease(k);
    }
    for (_, v) in pairs {
        if *v != kCFBooleanTrue {
            CFRelease(*v);
        }
    }
    d
}

/// An array; takes ownership of the values.
unsafe fn cf_array(vals: &[CFRef]) -> CFRef {
    let a = CFArrayCreate(null_mut(), vals.as_ptr(), vals.len() as isize, &kCFTypeArrayCallBacks);
    for v in vals {
        CFRelease(*v);
    }
    a
}

unsafe fn get<T: Copy + Default>(id: AudioObjectID, selector: u32, scope: u32) -> Option<T> {
    let addr = PropertyAddress { selector, scope, element: kElementMain };
    let mut v = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    (AudioObjectGetPropertyData(id, &addr, 0, std::ptr::null(), &mut size, &mut v as *mut T as *mut c_void) == 0).then_some(v)
}

unsafe fn get_cf_string(id: AudioObjectID, selector: u32) -> Option<String> {
    let s: usize = get::<usize>(id, selector, kScopeGlobal)?;
    let r = s as CFRef;
    let out = cf_to_string(r);
    if !r.is_null() {
        CFRelease(r);
    }
    out
}

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}
/// dlsym's "search everything loaded" handle on macOS.
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

type CreateTap = unsafe extern "C" fn(Id, *mut AudioObjectID) -> OSStatus;
type DestroyTap = unsafe extern "C" fn(AudioObjectID) -> OSStatus;

fn tap_fns() -> Option<(CreateTap, DestroyTap)> {
    unsafe {
        let c = dlsym(RTLD_DEFAULT, b"AudioHardwareCreateProcessTap\0".as_ptr() as *const c_char);
        let d = dlsym(RTLD_DEFAULT, b"AudioHardwareDestroyProcessTap\0".as_ptr() as *const c_char);
        if c.is_null() || d.is_null() || class(b"CATapDescription\0").is_null() {
            return None;
        }
        Some((std::mem::transmute::<*mut c_void, CreateTap>(c), std::mem::transmute::<*mut c_void, DestroyTap>(d)))
    }
}

/// Whether this Mac can record other apps (macOS 14.2 or later).
pub fn available() -> bool {
    tap_fns().is_some()
}

/// An app that has audio with Core Audio, with all its helper processes
/// (browsers play through helpers: Chrome's "Helper" processes, Safari's
/// WebKit processes).
#[derive(Clone, Debug)]
pub struct AppAudio {
    /// What Preferences remembers: the app's bundle identifier.
    pub key: String,
    pub name: String,
    pub objects: Vec<AudioObjectID>,
    /// Playing sound right now.
    pub playing: bool,
}

/// The app a helper process belongs to, by bundle identifier.
pub fn owner_key(bundle: &str) -> String {
    let b = bundle.trim();
    if b.starts_with("com.apple.WebKit.") || b.starts_with("com.apple.WebKit") {
        return "com.apple.Safari".into();
    }
    if b == "org.mozilla.plugincontainer" {
        return "org.mozilla.firefox".into();
    }
    let lower = b.to_ascii_lowercase();
    for marker in [".helper", ".framework.", ".gpu", ".renderer", ".plugin", ".xpc"] {
        if let Some(i) = lower.find(marker) {
            return b[..i].to_string();
        }
    }
    b.to_string()
}

fn app_name(key: &str) -> String {
    unsafe {
        let cls = class(b"NSRunningApplication\0");
        if !cls.is_null() {
            let k = cf_string(key);
            let list = send_id(cls, b"runningApplicationsWithBundleIdentifier:\0", k as Id);
            CFRelease(k);
            if !list.is_null() && send_count(list) > 0 {
                if let Some(n) = ns_string(send0(send_usize(list, b"objectAtIndex:\0", 0), b"localizedName\0")) {
                    if key == "com.apple.Safari" {
                        return format!("{n} (and other WebKit apps)");
                    }
                    return n;
                }
            }
        }
    }
    key.rsplit('.').next().unwrap_or(key).to_string()
}

fn pid_name(pid: i32) -> Option<String> {
    unsafe {
        let cls = class(b"NSRunningApplication\0");
        if cls.is_null() {
            return None;
        }
        let app = send_i32(cls, b"runningApplicationWithProcessIdentifier:\0", pid);
        if app.is_null() {
            return None;
        }
        ns_string(send0(app, b"localizedName\0"))
    }
}

/// Apps with audio, grouped with their helpers, playing ones first.
pub fn apps() -> Vec<AppAudio> {
    let mut out: Vec<AppAudio> = Vec::new();
    unsafe {
        let addr = PropertyAddress { selector: kAudioHardwarePropertyProcessObjectList, scope: kScopeGlobal, element: kElementMain };
        let mut size = 0u32;
        if AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &addr, 0, std::ptr::null(), &mut size) != 0 {
            return out;
        }
        let mut ids = vec![0u32; size as usize / 4];
        if AudioObjectGetPropertyData(kAudioObjectSystemObject, &addr, 0, std::ptr::null(), &mut size, ids.as_mut_ptr() as *mut c_void) != 0 {
            return out;
        }
        ids.truncate(size as usize / 4);
        let me = std::process::id() as i32;
        for id in ids {
            let pid = get::<i32>(id, kAudioProcessPropertyPID, kScopeGlobal).unwrap_or(-1);
            if pid == me {
                continue;
            }
            let bundle = get_cf_string(id, kAudioProcessPropertyBundleID).unwrap_or_default();
            let playing = get::<u32>(id, kAudioProcessPropertyIsRunningOutput, kScopeGlobal).unwrap_or(0) != 0;
            let key = if bundle.is_empty() { format!("pid:{pid}") } else { owner_key(&bundle) };
            match out.iter_mut().find(|a| a.key == key) {
                Some(a) => {
                    a.objects.push(id);
                    a.playing |= playing;
                }
                None => {
                    let name = if bundle.is_empty() { pid_name(pid).unwrap_or_else(|| format!("Process {pid}")) } else { app_name(&key) };
                    out.push(AppAudio { key, name, objects: vec![id], playing });
                }
            }
        }
    }
    out.sort_by(|a, b| b.playing.cmp(&a.playing).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// What the IO thread needs.
struct State {
    on_audio: Box<dyn FnMut(&[f32], usize) + Send>,
    /// Channels in the aggregate's input before the tap's (from the clock device).
    skip: usize,
    tap_ch: usize,
    scratch: Vec<f32>,
}

unsafe extern "C" fn io_proc(
    _dev: AudioObjectID,
    _now: *const c_void,
    input: *const AudioBufferList,
    _input_time: *const c_void,
    _output: *mut AudioBufferList,
    _out_time: *const c_void,
    client: *mut c_void,
) -> OSStatus {
    if input.is_null() || client.is_null() {
        return 0;
    }
    let st = &mut *(client as *mut State);
    let list = &*input;
    let bufs = std::slice::from_raw_parts(list.buffers.as_ptr(), list.count as usize);
    // (buffer, channel within it, channels in it) for each of the tap's channels.
    let mut chans: [(usize, usize, usize); 8] = [(0, 0, 1); 8];
    let mut n = 0;
    let mut seen = 0;
    let mut frames = usize::MAX;
    for (bi, b) in bufs.iter().enumerate() {
        let nch = b.channels.max(1) as usize;
        for c in 0..nch {
            if seen >= st.skip && n < st.tap_ch.min(8) {
                chans[n] = (bi, c, nch);
                n += 1;
                frames = frames.min(b.bytes as usize / 4 / nch);
            }
            seen += 1;
        }
    }
    if n == 0 || frames == usize::MAX || frames == 0 {
        return 0;
    }
    st.scratch.clear();
    st.scratch.reserve(frames * n);
    for f in 0..frames {
        for &(bi, c, nch) in &chans[..n] {
            let data = bufs[bi].data as *const f32;
            st.scratch.push(if data.is_null() { 0.0 } else { *data.add(f * nch + c) });
        }
    }
    let scratch = std::mem::take(&mut st.scratch);
    (st.on_audio)(&scratch, n);
    st.scratch = scratch;
    0
}

/// A running tap; stops and cleans up when dropped.
pub struct Tap {
    tap: AudioObjectID,
    device: AudioObjectID,
    proc_id: *mut c_void,
    state: *mut State,
    destroy: DestroyTap,
    running: bool,
}

// The pointers are only used to stop and free from the owning thread.
unsafe impl Send for Tap {}

impl Tap {
    pub fn pause(&mut self) {
        if self.running {
            unsafe { AudioDeviceStop(self.device, self.proc_id) };
            self.running = false;
        }
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        unsafe {
            self.pause();
            AudioDeviceDestroyIOProcID(self.device, self.proc_id);
            AudioHardwareDestroyAggregateDevice(self.device);
            (self.destroy)(self.tap);
            drop(Box::from_raw(self.state));
        }
    }
}

/// Start recording the apps with these bundle identifiers (all of their
/// processes), or everything except Audemo when `keys` is empty.
/// `on_audio` gets interleaved float frames and the channel count.
/// Returns the tap with its (sample rate, channels).
pub fn start(keys: &[String], on_audio: Box<dyn FnMut(&[f32], usize) + Send>) -> Result<(Tap, u32, usize), String> {
    let (create, destroy) = tap_fns().ok_or("Recording other apps needs macOS 14.2 or later.")?;
    unsafe {
        // The tap's description.
        let cls = class(b"CATapDescription\0");
        let desc = send0(cls, b"alloc\0");
        let objects: Vec<AudioObjectID> = if keys.is_empty() {
            // Leave Audemo itself out, so its own playback isn't recorded.
            let pid = std::process::id() as i32;
            let addr = PropertyAddress { selector: kAudioHardwarePropertyTranslatePIDToProcessObject, scope: kScopeGlobal, element: kElementMain };
            let mut me: AudioObjectID = 0;
            let mut size = 4u32;
            let ok = AudioObjectGetPropertyData(kAudioObjectSystemObject, &addr, 4, &pid as *const i32 as *const c_void, &mut size, &mut me as *mut u32 as *mut c_void) == 0;
            if ok && me != 0 {
                vec![me]
            } else {
                Vec::new()
            }
        } else {
            apps().into_iter().filter(|a| keys.contains(&a.key)).flat_map(|a| a.objects).collect()
        };
        if !keys.is_empty() && objects.is_empty() {
            return Err("None of the chosen apps is running. Start the app (and play something), or choose others in Preferences > Audio Hardware.".into());
        }
        let nums: Vec<CFRef> = objects.iter().map(|&o| cf_i32(o as i32)).collect();
        let arr = cf_array(&nums);
        let desc = if keys.is_empty() {
            send_id(desc, b"initStereoGlobalTapButExcludeProcesses:\0", arr as Id)
        } else {
            send_id(desc, b"initStereoMixdownOfProcesses:\0", arr as Id)
        };
        CFRelease(arr);
        if desc.is_null() {
            return Err("macOS refused the audio tap description.".into());
        }
        let mut tap: AudioObjectID = 0;
        let st = create(desc, &mut tap);
        let uuid = ns_string(send0(send0(desc, b"UUID\0"), b"UUIDString\0"));
        send0(desc, b"release\0");
        if st != 0 || tap == 0 {
            return Err(format!("Couldn't start recording other apps (Core Audio error {st})."));
        }
        let tap_uid = get_cf_string(tap, kAudioTapPropertyUID).or(uuid).unwrap_or_default();
        let fmt: StreamDescription = get(tap, kAudioTapPropertyFormat, kScopeGlobal).unwrap_or_default();
        if fmt.bits_per_channel != 0 && fmt.bits_per_channel != 32 {
            destroy(tap);
            return Err(format!("The tap's audio format isn't supported ({} bits).", fmt.bits_per_channel));
        }
        let rate = if fmt.sample_rate > 0.0 { fmt.sample_rate as u32 } else { 48000 };
        let tap_ch = (fmt.channels_per_frame as usize).clamp(1, 8);

        // A private aggregate device: the default output as its clock, plus the tap.
        let out_dev: AudioObjectID = get(kAudioObjectSystemObject, kAudioHardwarePropertyDefaultOutputDevice, kScopeGlobal).unwrap_or(0);
        let out_uid = get_cf_string(out_dev, kAudioDevicePropertyDeviceUID).unwrap_or_default();
        let agg_uid = format!("audemo-tap-{}-{}", std::process::id(), tap);
        let tap_entry = cf_dict(&[("uid", cf_string(&tap_uid)), ("drift", kCFBooleanTrue)]);
        let taps = cf_array(&[tap_entry]);
        let mut pairs: Vec<(&str, CFRef)> = vec![
            ("name", cf_string("Audemo App Recording")),
            ("uid", cf_string(&agg_uid)),
            ("private", kCFBooleanTrue),
            ("tapautostart", kCFBooleanTrue),
            ("stacked", cf_i32(0)),
            ("taps", taps),
        ];
        if !out_uid.is_empty() {
            let sub = cf_dict(&[("uid", cf_string(&out_uid))]);
            pairs.push(("subdevices", cf_array(&[sub])));
            pairs.push(("master", cf_string(&out_uid)));
        }
        let dict = cf_dict(&pairs);
        let mut device: AudioObjectID = 0;
        let st = AudioHardwareCreateAggregateDevice(dict, &mut device);
        CFRelease(dict);
        if st != 0 || device == 0 {
            destroy(tap);
            return Err(format!("Couldn't set up recording other apps (Core Audio error {st})."));
        }
        // Channels of the aggregate's input that come before the tap's.
        let total_in = input_channels(device);
        let skip = total_in.saturating_sub(tap_ch);

        let state = Box::into_raw(Box::new(State { on_audio, skip, tap_ch, scratch: Vec::new() }));
        let mut proc_id: *mut c_void = null_mut();
        let st = AudioDeviceCreateIOProcID(device, io_proc, state as *mut c_void, &mut proc_id);
        if st != 0 {
            AudioHardwareDestroyAggregateDevice(device);
            destroy(tap);
            drop(Box::from_raw(state));
            return Err(format!("Couldn't read the app audio (Core Audio error {st})."));
        }
        let mut t = Tap { tap, device, proc_id, state, destroy, running: false };
        let st = AudioDeviceStart(device, proc_id);
        if st != 0 {
            return Err(format!("Couldn't start reading the app audio (Core Audio error {st})."));
        }
        t.running = true;
        Ok((t, rate, tap_ch))
    }
}

unsafe fn input_channels(device: AudioObjectID) -> usize {
    let addr = PropertyAddress { selector: kAudioDevicePropertyStreamConfiguration, scope: kScopeInput, element: kElementMain };
    let mut size = 0u32;
    if AudioObjectGetPropertyDataSize(device, &addr, 0, std::ptr::null(), &mut size) != 0 || size == 0 {
        return 0;
    }
    // An AudioBufferList of `size` bytes (8-byte aligned).
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    if AudioObjectGetPropertyData(device, &addr, 0, std::ptr::null(), &mut size, buf.as_mut_ptr() as *mut c_void) != 0 {
        return 0;
    }
    let list = &*(buf.as_ptr() as *const AudioBufferList);
    let bufs = std::slice::from_raw_parts(list.buffers.as_ptr(), list.count as usize);
    bufs.iter().map(|b| b.channels as usize).sum()
}

#[cfg(test)]
mod tests {
    use super::owner_key;

    #[test]
    fn helpers_belong_to_their_apps() {
        assert_eq!(owner_key("com.google.Chrome.helper"), "com.google.Chrome");
        assert_eq!(owner_key("com.google.Chrome.helper.Renderer"), "com.google.Chrome");
        assert_eq!(owner_key("com.microsoft.edgemac.helper"), "com.microsoft.edgemac");
        assert_eq!(owner_key("com.apple.WebKit.GPU"), "com.apple.Safari");
        assert_eq!(owner_key("org.mozilla.plugincontainer"), "org.mozilla.firefox");
        assert_eq!(owner_key("com.spotify.client"), "com.spotify.client");
        assert_eq!(owner_key("com.tinyspeck.slackmacgap.helper"), "com.tinyspeck.slackmacgap");
    }
}
