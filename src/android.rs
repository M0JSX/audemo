//! Android: the entry point (`android_main`, called by NativeActivity),
//! storage and microphone permissions, and the on-screen keyboard.
//!
//! Audemo works with ordinary file paths, so it asks for "All files
//! access" (MANAGE_EXTERNAL_STORAGE) rather than going through the document
//! picker; its own file browser (filedlg.rs) does the rest.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use jni::objects::{JObject, JValue};
use jni::JNIEnv;
use winit::platform::android::activity::AndroidApp;

static APP: OnceLock<AndroidApp> = OnceLock::new();

#[no_mangle]
fn android_main(app: AndroidApp) {
    let _ = APP.set(app.clone());
    // Preferences, autosave and temporary files live in the app's private
    // storage. (Before other threads read the environment.)
    if let Some(home) = app.internal_data_path() {
        let tmp = home.join("tmp");
        let _ = std::fs::create_dir_all(&tmp);
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join("config"));
        std::env::set_var("TMPDIR", &tmp);
    }
    crate::install_crash_log();
    crate::prefs::apply_temp_dir(&crate::prefs::Prefs::load());
    request_microphone();
    let mut options = crate::native_options();
    options.android_app = Some(app);
    if let Err(e) = crate::run(options, Vec::new()) {
        eprintln!("Audemo: {e}");
    }
}

/// Run `f` with the JNI environment and the activity.
fn with_activity<R>(f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<R>) -> Option<R> {
    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
    let mut env = vm.attach_current_thread_permanently().ok()?;
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
    let r = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    r.ok()
}

/// Ask for the microphone (Android shows its own prompt the first time).
fn request_microphone() {
    with_activity(|env, act| {
        let perm = env.new_string("android.permission.RECORD_AUDIO")?;
        let granted = env.call_method(act, "checkSelfPermission", "(Ljava/lang/String;)I", &[JValue::Object(&perm)])?.i()?;
        if granted != 0 {
            let arr = env.new_object_array(1, "java/lang/String", &perm)?;
            env.call_method(act, "requestPermissions", "([Ljava/lang/String;I)V", &[JValue::Object(&arr), JValue::Int(1)])?;
        }
        Ok(())
    });
}

/// Whether Audemo has All files access.
pub fn storage_granted() -> bool {
    with_activity(|env, _| env.call_static_method("android/os/Environment", "isExternalStorageManager", "()Z", &[])?.z()).unwrap_or(false)
}

/// Open the system page where All files access is switched on for Audemo.
pub fn open_storage_settings() {
    with_activity(|env, act| {
        let pkg = env.call_method(act, "getPackageName", "()Ljava/lang/String;", &[])?.l()?;
        let pkg: String = env.get_string(&pkg.into())?.into();
        let action = env.new_string("android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION")?;
        let uri_text = env.new_string(format!("package:{pkg}"))?;
        let uri = env.call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&uri_text)])?.l()?;
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;Landroid/net/Uri;)V", &[JValue::Object(&action), JValue::Object(&uri)])?;
        env.call_method(act, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        Ok(())
    });
}

static KEYBOARD_SHOWN: AtomicBool = AtomicBool::new(false);
static STORAGE_OK: AtomicBool = AtomicBool::new(false);
static STORAGE_DISMISSED: AtomicBool = AtomicBool::new(false);
static STORAGE_CHECKED_MS: AtomicU64 = AtomicU64::new(0);

fn since_start() -> Duration {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed()
}

/// Per frame: show the on-screen keyboard while a text field has focus,
/// and ask for All files access until it's given.
pub fn frame(ctx: &egui::Context) {
    let want = ctx.wants_keyboard_input();
    if want != KEYBOARD_SHOWN.swap(want, Ordering::Relaxed) {
        if let Some(app) = APP.get() {
            if want {
                app.show_soft_input(true);
            } else {
                app.hide_soft_input(true);
            }
        }
    }

    // Re-check about once a second (the user may be coming back from
    // Settings).
    let now = since_start().as_millis() as u64;
    if !STORAGE_OK.load(Ordering::Relaxed) && now.saturating_sub(STORAGE_CHECKED_MS.load(Ordering::Relaxed)) > 1000 {
        STORAGE_CHECKED_MS.store(now.max(1), Ordering::Relaxed);
        STORAGE_OK.store(storage_granted(), Ordering::Relaxed);
    }
    if STORAGE_OK.load(Ordering::Relaxed) || STORAGE_DISMISSED.load(Ordering::Relaxed) {
        return;
    }
    ctx.request_repaint_after(Duration::from_millis(1000));
    egui::Window::new("Allow access to your files")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.label("Audemo opens and saves audio in your folders (Music, Recordings, Download and so on), so it needs All files access.");
            ui.add_space(4.0);
            ui.label(RichText::new("Tap Open Settings, switch on \"Allow access to manage all files\", then come back.").color(crate::theme::TEXT_DIM()));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new("Open Settings").min_size(egui::vec2(140.0, 36.0))).clicked() {
                    open_storage_settings();
                }
                if ui.add(egui::Button::new("Not Now").min_size(egui::vec2(100.0, 36.0))).clicked() {
                    STORAGE_DISMISSED.store(true, Ordering::Relaxed);
                }
            });
        });
}
