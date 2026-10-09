# Audemo

A native, cross-platform waveform audio editor written in Rust. The workspace follows the layout of a
professional waveform editor: menu bar, toolbar, Files / Effects / Markers panel on the left, the
editor in the centre, Selection/View and History on the right, transport and level meters along the
bottom. It runs on Windows, macOS and Linux from one codebase.

Audemo is an independent project and is not affiliated with Adobe.

## Features

- **Open** WAV, AIFF, FLAC, MP3, OGG/Vorbis, M4A/AAC, ALAC, CAF and MKV/WebM audio (drag files onto the window, or pass them on the command line).
- **Save** WAV as 16-bit, 24-bit or 32-bit integer, or 32-bit float, with optional TPDF dither.
- **Waveform editor** with an overview/zoom navigator, time ruler, per-channel lanes, amplitude ruler in dB, sample-level zoom and channel enable toggles (edit L or R only).
- **Spectral frequency display** (Shift+D) shown under the waveform.
- **Edit**: cut, copy, paste, paste to new, delete, crop, select all, convert sample rate and channel count.
- **Unlimited-style history**: 60-step undo/redo plus a History panel you can click to jump to any state.
- **Fade handles** in the top corners of the waveform and a floating **clip gain** control.
- **Markers** (M), with rename, jump and delete in the Markers panel.
- **Playback** through the system output (WASAPI, CoreAudio, ALSA/PulseAudio/PipeWire), looping, monitor volume, peak meters with hold and clip indicator.
- **Recording** from the default input (Shift+Space); inserts at the cursor or creates a new file.
- **Effect windows** with presets, live looping preview (Space) and bypass, plus a draggable frequency-response curve for the EQs and filters.
- **Favorites** menu for one-click common jobs.

## Effects (39)

| Menu | Effects |
|---|---|
| Effects | Invert, Reverse, Silence |
| Amplitude and Compression | Amplify, Normalize, Fade In, Fade Out, Dynamics Processing, Hard Limiter, Noise Gate, Speech Volume Leveler |
| Delay and Echo | Delay, Echo (with ping-pong), Analog Delay (tape / tube / BBD) |
| Filter and EQ | Parametric Equalizer (5 bands + cut filters), Graphic Equalizer (10 bands), Scientific Filter (Butterworth LP/HP/BP/BS, up to 8-pole) |
| Modulation | Chorus, Flanger, Phaser |
| Noise Reduction / Restoration | Capture Noise Print, Noise Reduction (process), Adaptive Noise Reduction, DeHummer (50/60 Hz + harmonics), Click/Pop Eliminator, DeClipper |
| Reverb | Studio Reverb (algorithmic), Full Reverb (convolution with a synthesised impulse response) |
| Special | Distortion (soft/hard/tube/foldback/bit-crush), Vocal Enhancer |
| Stereo Imagery | Channel Mixer, Stereo Expander, Centre Channel Extractor, Pan |
| Time and Pitch | Stretch and Pitch (phase vocoder with phase locking), Pitch Shifter, Varispeed |
| Generate | Tones (sine/square/triangle/saw, sweeps), Noise (white/pink/brown), Silence |

All processing is 32-bit float. Effects run on a background thread, so the window stays responsive on long files.

### Noise reduction workflow

1. Select a stretch of audio that contains only the noise.
2. **Effects > Noise Reduction / Restoration > Capture Noise Print** (Shift+P).
3. Select the audio to clean (or nothing, for the whole file) and open **Noise Reduction (process)**.
4. Preview, adjust *Noise reduction* and *Reduce by*, then Apply.

## Keyboard shortcuts

| Action | Keys |
|---|---|
| Play / stop | Space |
| Record | Shift+Space |
| Undo / redo | Ctrl+Z / Ctrl+Shift+Z (Ctrl+Y) |
| Cut / copy / paste | Ctrl+X / Ctrl+C / Ctrl+V |
| Delete / crop | Delete / Ctrl+T |
| Select all / deselect | Ctrl+A / Esc |
| Add marker / previous / next | M / ← / → |
| Zoom in / out / full | = / - / \\ |
| Zoom time at pointer | Mouse wheel |
| Scroll | Shift+wheel, middle-drag, or the Hand tool |
| Zoom amplitude | Alt+wheel |
| Spectral display | Shift+D |
| Capture noise print | Shift+P |

On macOS use ⌘ instead of Ctrl.

## Building from source

Install Rust (stable, 1.80 or newer) from <https://rustup.rs>, then:

```sh
cargo run --release              # build and launch
cargo run --release -- file.wav  # open a file at start-up
cargo test --release             # DSP and editing tests
```

The binary is `target/release/audemo` (`audemo.exe` on Windows).

### Platform notes

**Windows** needs nothing extra (MSVC toolchain from the Visual Studio Build Tools, which rustup prompts for).

**macOS** needs the Xcode command line tools (`xcode-select --install`). To build an `.app` bundle with the microphone permission prompt, follow the macOS steps in `.github/workflows/build.yml`. The CI build is ad-hoc signed but not notarised, so the first launch needs right-click > Open (or `xattr -dr com.apple.quarantine Audemo.app`).

**Linux** (Debian/Ubuntu package names):

```sh
sudo apt install build-essential pkg-config libasound2-dev libxkbcommon-dev libwayland-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libgtk-3-dev
```

Fedora: `sudo dnf install alsa-lib-devel libxkbcommon-devel wayland-devel gtk3-devel`.
Open/save dialogs use the XDG desktop portal, which every mainstream desktop provides.

### Installers via GitHub Actions

Push the repository to GitHub. The workflow in `.github/workflows/build.yml` runs the tests and builds,
for every push:

| Platform | Installer | Portable |
|---|---|---|
| Windows | `Audemo-<version>-x64.msi` (Program Files, Start menu entry, uninstall from Settings > Apps) | `audemo-windows-x64-portable.zip` |
| macOS | `Audemo.dmg` (drag to Applications; universal Apple silicon + Intel) | `audemo-macos-universal.zip` |
| Linux | `audemo_<version>_amd64.deb` (`sudo apt install ./audemo_*.deb`) and `Audemo-x86_64.AppImage` | `audemo-linux-x64.tar.gz` |

Download them from the run's **Artifacts** section on the Actions tab. Push a tag such as `v0.1.0` to
publish them on a GitHub Release instead.

To package locally: `bash scripts/package-macos.sh` on a Mac, or `cargo build --release && bash scripts/package-linux.sh`
on Linux. The Windows MSI needs the WiX v5 CLI (`dotnet tool install --global wix --version 5.0.2`); see the
Windows step in the workflow for the exact command.

The installers are not code-signed. Windows SmartScreen will warn on first run (More info > Run anyway), and on
macOS right-click the app and choose Open the first time.

## Code layout

```
src/
  main.rs        window setup and panel layout
  app.rs         documents, undo history, actions, background jobs
  editor.rs      waveform / spectral editor, selection, fades, gain HUD
  panels.rs      menus, toolbar, side panels, transport, meters, status bar
  dialogs.rs     effect windows (presets, preview, EQ curve) and other dialogs
  engine.rs      cpal playback and recording
  io.rs          symphonia decoding, hound WAV export
  theme.rs       palette and hand-drawn transport icons
assets/          app icon, macOS Info.plist, Linux .desktop file, WiX installer source
scripts/         macOS and Linux packaging scripts
  dsp/           dependency-free DSP: FFT, biquads, STFT, resampler,
                 peak cache, spectrogram and every effect (+ tests)
```

Adding an effect means writing one `fn(&[Vec<f32>], &Ctx, &Params) -> Result<Vec<Vec<f32>>, String>`
and registering it with its parameters in `src/dsp/effects/`; the menu entry, dialog, presets and
preview are generated from that definition.

## Not included (yet)

Multitrack session view, MP3/FLAC export, VST/AU plug-in hosting, spectral painting/healing tools,
and batch processing.
