//! A plug-in's own editor (IPlugView) in a window of its own: a Win32
//! window on Windows, an NSWindow on macOS. Everything here runs on the UI
//! thread, whose event loop (winit's) also drives these windows.

#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_void};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering};

use super::vst3::{uid, ComPtr, FUnknownVtbl, TResult, Tuid, IID_FUNKNOWN, NO_INTERFACE, OK};

#[allow(dead_code)]
pub const IID_PLUG_VIEW: Tuid = uid(0x5BC32507, 0xD06049EA, 0xA6151B52, 0x2B755B29);
pub const IID_PLUG_FRAME: Tuid = uid(0x367FAF01, 0xAFA94693, 0x8D4DA2A0, 0xED0882A3);
pub const IID_CONTENT_SCALE: Tuid = uid(0x65ED9690, 0x8AC44525, 0x8AADEF7A, 0x72EA703F);

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct ViewRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ViewRect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

#[repr(C)]
pub struct PlugViewVtbl {
    pub unk: FUnknownVtbl,
    pub is_platform_type_supported: unsafe extern "system" fn(*mut c_void, *const c_char) -> TResult,
    pub attached: unsafe extern "system" fn(*mut c_void, *mut c_void, *const c_char) -> TResult,
    pub removed: unsafe extern "system" fn(*mut c_void) -> TResult,
    pub on_wheel: unsafe extern "system" fn(*mut c_void, f32) -> TResult,
    pub on_key_down: unsafe extern "system" fn(*mut c_void, u16, i16, i16) -> TResult,
    pub on_key_up: unsafe extern "system" fn(*mut c_void, u16, i16, i16) -> TResult,
    pub get_size: unsafe extern "system" fn(*mut c_void, *mut ViewRect) -> TResult,
    pub on_size: unsafe extern "system" fn(*mut c_void, *mut ViewRect) -> TResult,
    pub on_focus: unsafe extern "system" fn(*mut c_void, u8) -> TResult,
    pub set_frame: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    pub can_resize: unsafe extern "system" fn(*mut c_void) -> TResult,
    pub check_size_constraint: unsafe extern "system" fn(*mut c_void, *mut ViewRect) -> TResult,
}

#[repr(C)]
struct ContentScaleVtbl {
    unk: FUnknownVtbl,
    set_content_scale_factor: unsafe extern "system" fn(*mut c_void, f32) -> TResult,
}

#[cfg(windows)]
const PLATFORM: &[u8] = b"HWND\0";
#[cfg(target_os = "macos")]
const PLATFORM: &[u8] = b"NSView\0";
#[cfg(not(any(windows, target_os = "macos")))]
const PLATFORM: &[u8] = b"X11EmbedWindowID\0";

/// Whether this build can show plug-in editors.
pub const SUPPORTED: bool = cfg!(any(windows, target_os = "macos"));

// ------------------------------------------------------------------ IPlugFrame

#[repr(C)]
struct FrameVtbl {
    unk: FUnknownVtbl,
    resize_view: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut ViewRect) -> TResult,
}

#[repr(C)]
struct Frame {
    vtbl: *const FrameVtbl,
    refs: AtomicU32,
    /// The window to resize (owned by EditorWindow, outlives the frame's use).
    win: *mut native::Win,
}

unsafe extern "system" fn frame_qi(this: *mut c_void, iid: *const Tuid, obj: *mut *mut c_void) -> TResult {
    if !iid.is_null() && (*iid == IID_FUNKNOWN || *iid == IID_PLUG_FRAME) {
        frame_add_ref(this);
        *obj = this;
        OK
    } else {
        *obj = null_mut();
        NO_INTERFACE
    }
}
unsafe extern "system" fn frame_add_ref(this: *mut c_void) -> u32 {
    (*(this as *mut Frame)).refs.fetch_add(1, Ordering::AcqRel) + 1
}
unsafe extern "system" fn frame_release(this: *mut c_void) -> u32 {
    let n = (*(this as *mut Frame)).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if n == 0 {
        drop(Box::from_raw(this as *mut Frame));
    }
    n
}
unsafe extern "system" fn resize_view(this: *mut c_void, view: *mut c_void, r: *mut ViewRect) -> TResult {
    let f = &*(this as *mut Frame);
    if r.is_null() || view.is_null() || f.win.is_null() {
        return super::vst3::INVALID_ARG;
    }
    let rect = *r;
    native::resize(&mut *f.win, rect.width().max(1), rect.height().max(1));
    // The SDK asks hosts to confirm the new size with onSize.
    let v = &**(view as *mut *const PlugViewVtbl);
    let mut rr = ViewRect { left: 0, top: 0, right: rect.width(), bottom: rect.height() };
    (v.on_size)(view, &mut rr);
    OK
}
static FRAME_VTBL: FrameVtbl = FrameVtbl {
    unk: FUnknownVtbl { query_interface: frame_qi, add_ref: frame_add_ref, release: frame_release },
    resize_view,
};

// ------------------------------------------------------------------ editor window

/// A plug-in's own window, whatever the plug-in format.
pub trait Window {
    /// True once the user has closed the window.
    fn closed(&self) -> bool;
}

pub struct EditorWindow {
    view: Option<ComPtr<PlugViewVtbl>>,
    frame: *mut Frame,
    win: Box<native::Win>,
}

impl EditorWindow {
    /// Open the editor of the edit controller at `controller`.
    pub fn open(controller: &ComPtr<super::vst3::EditControllerVtbl>, title: &str) -> Result<EditorWindow, String> {
        if !SUPPORTED {
            return Err("Plug-in windows aren't available on this system yet; use the controls in this window.".into());
        }
        let raw = unsafe { (controller.vtbl().create_view)(controller.ptr, b"editor\0".as_ptr() as *const c_char) };
        let view: ComPtr<PlugViewVtbl> = unsafe { ComPtr::from_raw(raw) }.ok_or("This plug-in has no window of its own.")?;
        let v = view.vtbl();
        unsafe {
            if (v.is_platform_type_supported)(view.ptr, PLATFORM.as_ptr() as *const c_char) != OK {
                return Err("This plug-in's window doesn't support this system.".into());
            }
            let scale = native::scale_factor();
            if scale != 1.0 {
                if let Some(cs) = view.query::<ContentScaleVtbl>(&IID_CONTENT_SCALE) {
                    (cs.vtbl().set_content_scale_factor)(cs.ptr, scale);
                }
            }
            let mut r = ViewRect::default();
            if (v.get_size)(view.ptr, &mut r) != OK || r.width() <= 0 || r.height() <= 0 {
                r = ViewRect { left: 0, top: 0, right: 600, bottom: 400 };
            }
            let resizable = (v.can_resize)(view.ptr) == OK;
            let mut win = native::create(title, r.width(), r.height(), resizable)?;
            win.view = view.ptr;
            let frame = Box::into_raw(Box::new(Frame { vtbl: &FRAME_VTBL, refs: AtomicU32::new(1), win: &mut *win }));
            (v.set_frame)(view.ptr, frame as *mut c_void);
            if (v.attached)(view.ptr, native::parent(&win), PLATFORM.as_ptr() as *const c_char) != OK {
                (v.set_frame)(view.ptr, null_mut());
                frame_release(frame as *mut c_void);
                native::destroy(&mut win);
                return Err("The plug-in couldn't open its window.".into());
            }
            native::show(&mut win);
            Ok(EditorWindow { view: Some(view), frame, win })
        }
    }

}

impl Window for EditorWindow {
    fn closed(&self) -> bool {
        native::closed(&self.win)
    }
}

impl Drop for EditorWindow {
    fn drop(&mut self) {
        unsafe {
            if let Some(view) = self.view.take() {
                (view.vtbl().removed)(view.ptr);
                (view.vtbl().set_frame)(view.ptr, null_mut());
                self.win.view = null_mut();
                drop(view);
            }
            (*self.frame).win = null_mut();
            frame_release(self.frame as *mut c_void);
            native::destroy(&mut self.win);
        }
    }
}

// ------------------------------------------------------------------ Windows

#[cfg(windows)]
mod native {
    use super::*;

    type Hwnd = *mut c_void;
    type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

    #[repr(C)]
    struct WndClassExW {
        cb_size: u32,
        style: u32,
        wnd_proc: WndProc,
        cls_extra: i32,
        wnd_extra: i32,
        instance: *mut c_void,
        icon: *mut c_void,
        cursor: *mut c_void,
        background: *mut c_void,
        menu_name: *const u16,
        class_name: *const u16,
        icon_sm: *mut c_void,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassExW(c: *const WndClassExW) -> u16;
        fn CreateWindowExW(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: Hwnd, menu: *mut c_void, inst: *mut c_void, param: *mut c_void) -> Hwnd;
        fn DefWindowProcW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
        fn DestroyWindow(h: Hwnd) -> i32;
        fn ShowWindow(h: Hwnd, cmd: i32) -> i32;
        fn SetForegroundWindow(h: Hwnd) -> i32;
        fn SetWindowPos(h: Hwnd, after: Hwnd, x: i32, y: i32, w: i32, h2: i32, flags: u32) -> i32;
        fn AdjustWindowRectEx(r: *mut Rect, style: u32, menu: i32, ex: u32) -> i32;
        fn GetClientRect(h: Hwnd, r: *mut Rect) -> i32;
        fn SetWindowLongPtrW(h: Hwnd, idx: i32, v: isize) -> isize;
        fn GetWindowLongPtrW(h: Hwnd, idx: i32) -> isize;
        fn GetActiveWindow() -> Hwnd;
        fn LoadCursorW(inst: *mut c_void, name: *const u16) -> *mut c_void;
        fn GetDpiForWindow(h: Hwnd) -> u32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }

    const WM_SIZE: u32 = 0x0005;
    const WM_CLOSE: u32 = 0x0010;
    const GWLP_USERDATA: i32 = -21;
    const WS_CAPTION: u32 = 0x00C0_0000;
    const WS_SYSMENU: u32 = 0x0008_0000;
    const WS_THICKFRAME: u32 = 0x0004_0000;
    const WS_MINIMIZEBOX: u32 = 0x0002_0000;
    const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
    const WS_CLIPCHILDREN: u32 = 0x0200_0000;
    const WS_EX_DLGMODALFRAME: u32 = 0x0000_0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;

    pub struct Win {
        hwnd: Hwnd,
        style: u32,
        ex: u32,
        pub view: *mut c_void,
        closed: bool,
        /// Inside a resize we asked for (don't echo it back to the plug-in).
        resizing: bool,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    unsafe extern "system" fn wnd_proc(h: Hwnd, m: u32, w: usize, l: isize) -> isize {
        let win = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Win;
        match m {
            WM_CLOSE if !win.is_null() => {
                // Hide now; the editor is torn down on the next UI frame.
                (*win).closed = true;
                ShowWindow(h, 0);
                0
            }
            // Ignore minimising (SIZE_MINIMIZED): the plug-in keeps its size.
            WM_SIZE if !win.is_null() && !(*win).resizing && !(*win).view.is_null() && w != 1 => {
                let mut r = Rect::default();
                GetClientRect(h, &mut r);
                if r.right <= r.left || r.bottom <= r.top {
                    return 0;
                }
                let view = (*win).view;
                let v = &**(view as *mut *const PlugViewVtbl);
                let mut vr = ViewRect { left: 0, top: 0, right: r.right - r.left, bottom: r.bottom - r.top };
                if (v.check_size_constraint)(view, &mut vr) == OK {
                    (v.on_size)(view, &mut vr);
                }
                0
            }
            _ => DefWindowProcW(h, m, w, l),
        }
    }

    fn class() -> *const u16 {
        static NAME: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *NAME.get_or_init(|| unsafe {
            let name = Box::leak(wide("AudemoPluginWindow").into_boxed_slice());
            let wc = WndClassExW {
                cb_size: std::mem::size_of::<WndClassExW>() as u32,
                style: 0,
                wnd_proc,
                cls_extra: 0,
                wnd_extra: 0,
                instance: GetModuleHandleW(std::ptr::null()),
                icon: null_mut(),
                cursor: LoadCursorW(null_mut(), 32512 as *const u16),
                background: null_mut(),
                menu_name: std::ptr::null(),
                class_name: name.as_ptr(),
                icon_sm: null_mut(),
            };
            RegisterClassExW(&wc);
            name.as_ptr() as usize
        }) as *const u16
    }

    pub fn scale_factor() -> f32 {
        unsafe {
            let h = GetActiveWindow();
            if h.is_null() {
                1.0
            } else {
                (GetDpiForWindow(h) as f32 / 96.0).max(1.0)
            }
        }
    }

    pub fn create(title: &str, w: i32, h: i32, resizable: bool) -> Result<Box<Win>, String> {
        unsafe {
            let mut style = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
            if resizable {
                style |= WS_THICKFRAME | WS_MAXIMIZEBOX;
            }
            let ex = WS_EX_DLGMODALFRAME;
            let mut r = Rect { left: 0, top: 0, right: w, bottom: h };
            AdjustWindowRectEx(&mut r, style, 0, ex);
            let t = wide(title);
            // Owned by Audemo's main window: stays above it, minimises with it.
            let owner = GetActiveWindow();
            let hwnd = CreateWindowExW(ex, class(), t.as_ptr(), style, CW_USEDEFAULT, CW_USEDEFAULT, r.right - r.left, r.bottom - r.top, owner, null_mut(), GetModuleHandleW(std::ptr::null()), null_mut());
            if hwnd.is_null() {
                return Err("Couldn't create the plug-in window.".into());
            }
            let mut win = Box::new(Win { hwnd, style, ex, view: null_mut(), closed: false, resizing: false });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *win as *mut Win as isize);
            Ok(win)
        }
    }

    pub fn parent(win: &Win) -> *mut c_void {
        win.hwnd
    }

    pub fn show(win: &mut Win) {
        unsafe {
            ShowWindow(win.hwnd, 5);
            SetForegroundWindow(win.hwnd);
        }
        win.closed = false;
    }

    pub fn closed(win: &Win) -> bool {
        win.closed
    }

    pub fn resize(win: &mut Win, w: i32, h: i32) {
        unsafe {
            let mut r = Rect { left: 0, top: 0, right: w, bottom: h };
            AdjustWindowRectEx(&mut r, win.style, 0, win.ex);
            win.resizing = true;
            SetWindowPos(win.hwnd, null_mut(), 0, 0, r.right - r.left, r.bottom - r.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            win.resizing = false;
        }
    }

    pub fn destroy(win: &mut Win) {
        unsafe {
            if !win.hwnd.is_null() {
                SetWindowLongPtrW(win.hwnd, GWLP_USERDATA, 0);
                DestroyWindow(win.hwnd);
                win.hwnd = null_mut();
            }
        }
    }
}

// ------------------------------------------------------------------ macOS

#[cfg(target_os = "macos")]
pub(crate) mod native {
    use super::*;

    pub(crate) type Id = *mut c_void;
    pub(crate) type Sel = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub(crate) struct NsRect {
        pub x: f64,
        pub y: f64,
        pub w: f64,
        pub h: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub(crate) struct NsSize {
        pub w: f64,
        pub h: f64,
    }

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        pub(crate) fn objc_msgSend();
        #[cfg(target_arch = "x86_64")]
        fn objc_msgSend_stret();
        fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
        fn objc_registerClassPair(cls: Id);
        fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const c_char) -> i8;
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    /// Windows the user has closed (reported by the delegate below).
    static CLOSED: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

    unsafe extern "C" fn window_will_close(_this: Id, _sel: Sel, note: Id) {
        let w = send0(note, b"object\0");
        if let Ok(mut c) = CLOSED.lock() {
            c.push(w as usize);
        }
    }

    /// A shared NSWindow delegate that records windowWillClose:.
    fn delegate() -> Id {
        static D: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *D.get_or_init(|| unsafe {
            let mut cls = objc_allocateClassPair(class(b"NSObject\0"), b"AudemoPluginWindowDelegate\0".as_ptr() as *const c_char, 0);
            if cls.is_null() {
                cls = class(b"AudemoPluginWindowDelegate\0");
            } else {
                class_addMethod(cls, sel(b"windowWillClose:\0"), window_will_close as *const c_void, b"v@:@\0".as_ptr() as *const c_char);
                objc_registerClassPair(cls);
            }
            send0(send0(cls, b"alloc\0"), b"init\0") as usize
        }) as Id
    }

    /// `[o frame]`: an NSRect return (by hidden pointer on x86_64).
    pub(crate) unsafe fn frame_of(o: Id) -> NsRect {
        #[cfg(target_arch = "x86_64")]
        {
            let mut r = NsRect::default();
            let f: unsafe extern "C" fn(*mut NsRect, Id, Sel) = std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
            f(&mut r, o, sel(b"frame\0"));
            r
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let f: unsafe extern "C" fn(Id, Sel) -> NsRect = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            f(o, sel(b"frame\0"))
        }
    }

    /// Put `view` in the window and size the window to it.
    pub(crate) fn set_content(win: &mut Win, view: Id) {
        unsafe {
            let r = frame_of(view);
            send_id(win.window, b"setContentView:\0", view);
            if r.w > 0.0 && r.h > 0.0 {
                send_size(win.window, b"setContentSize:\0", NsSize { w: r.w, h: r.h });
            }
        }
    }
    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    pub(crate) fn sel(name: &[u8]) -> Sel {
        unsafe { sel_registerName(name.as_ptr() as *const c_char) }
    }
    pub(crate) fn class(name: &[u8]) -> Id {
        unsafe { objc_getClass(name.as_ptr() as *const c_char) }
    }
    // objc_msgSend cast to each signature used.
    pub(crate) unsafe fn send0(o: Id, s: &[u8]) -> Id {
        let f: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, sel(s))
    }
    pub(crate) unsafe fn send_id(o: Id, s: &[u8], a: Id) -> Id {
        let f: unsafe extern "C" fn(Id, Sel, Id) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, sel(s), a)
    }
    pub(crate) unsafe fn send_bool(o: Id, s: &[u8], a: bool) {
        let f: unsafe extern "C" fn(Id, Sel, i8) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, sel(s), a as i8)
    }
    pub(crate) unsafe fn send_long(o: Id, s: &[u8], a: isize) {
        let f: unsafe extern "C" fn(Id, Sel, isize) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, sel(s), a)
    }
    pub(crate) unsafe fn get_bool(o: Id, s: &[u8]) -> bool {
        let f: unsafe extern "C" fn(Id, Sel) -> i8 = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, sel(s)) != 0
    }
    pub(crate) unsafe fn send_size(o: Id, s: &[u8], a: NsSize) {
        let f: unsafe extern "C" fn(Id, Sel, NsSize) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, sel(s), a)
    }

    pub(crate) unsafe fn ns_string(s: &str) -> Id {
        let c = std::ffi::CString::new(s.replace('\0', "")).unwrap_or_default();
        let f: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(class(b"NSString\0"), sel(b"stringWithUTF8String:\0"), c.as_ptr())
    }

    pub struct Win {
        window: Id,
        pub view: *mut c_void,
        shown: bool,
    }

    /// NSView coordinates are in points; plug-ins scale themselves.
    pub fn scale_factor() -> f32 {
        1.0
    }

    pub fn create(title: &str, w: i32, h: i32, resizable: bool) -> Result<Box<Win>, String> {
        unsafe {
            // Titled | closable | miniaturizable (| resizable).
            let mut mask: usize = 1 | 2 | 4;
            if resizable {
                mask |= 8;
            }
            let alloc = send0(class(b"NSWindow\0"), b"alloc\0");
            let init: unsafe extern "C" fn(Id, Sel, NsRect, usize, usize, i8) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let window = init(alloc, sel(b"initWithContentRect:styleMask:backing:defer:\0"), NsRect { x: 0.0, y: 0.0, w: w as f64, h: h as f64 }, mask, 2, 0);
            if window.is_null() {
                return Err("Couldn't create the plug-in window.".into());
            }
            send_bool(window, b"setReleasedWhenClosed:\0", false);
            send_id(window, b"setDelegate:\0", delegate());
            send_id(window, b"setTitle:\0", ns_string(title));
            // Float over Audemo's window, and hide while another app is active.
            send_long(window, b"setLevel:\0", 3);
            send_bool(window, b"setHidesOnDeactivate:\0", true);
            send0(window, b"center\0");
            Ok(Box::new(Win { window, view: null_mut(), shown: false }))
        }
    }

    pub fn parent(win: &Win) -> *mut c_void {
        unsafe { send0(win.window, b"contentView\0") }
    }

    pub fn show(win: &mut Win) {
        unsafe {
            send_id(win.window, b"makeKeyAndOrderFront:\0", null_mut());
        }
        win.shown = true;
    }

    pub fn closed(win: &Win) -> bool {
        win.shown && CLOSED.lock().map(|c| c.contains(&(win.window as usize))).unwrap_or(false)
    }

    pub fn resize(win: &mut Win, w: i32, h: i32) {
        unsafe {
            send_size(win.window, b"setContentSize:\0", NsSize { w: w as f64, h: h as f64 });
        }
    }

    /// Close and free the window. Its content view (the plug-in's) is gone
    /// when this returns, so the plug-in can be destroyed right after.
    pub fn destroy(win: &mut Win) {
        unsafe {
            if !win.window.is_null() {
                let pool = objc_autoreleasePoolPush();
                send_id(win.window, b"setDelegate:\0", null_mut());
                send_id(win.window, b"setContentView:\0", null_mut());
                send_id(win.window, b"orderOut:\0", null_mut());
                send0(win.window, b"close\0");
                send0(win.window, b"release\0");
                objc_autoreleasePoolPop(pool);
                if let Ok(mut c) = CLOSED.lock() {
                    c.retain(|w| *w != win.window as usize);
                }
                win.window = null_mut();
            }
        }
    }
}

// ------------------------------------------------------------------ other systems

#[cfg(not(any(windows, target_os = "macos")))]
mod native {
    use super::*;

    pub struct Win {
        pub view: *mut c_void,
    }
    pub fn scale_factor() -> f32 {
        1.0
    }
    pub fn create(_: &str, _: i32, _: i32, _: bool) -> Result<Box<Win>, String> {
        Err("Plug-in windows aren't available on this system yet.".into())
    }
    pub fn parent(_: &Win) -> *mut c_void {
        null_mut()
    }
    pub fn show(_: &mut Win) {}
    pub fn closed(_: &Win) -> bool {
        true
    }
    pub fn resize(_: &mut Win, _: i32, _: i32) {}
    pub fn destroy(_: &mut Win) {}
}
