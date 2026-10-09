# Audemo

A native, cross-platform waveform audio editor written in Rust. The workspace follows the layout of a
professional waveform editor: menu bar, toolbar, Files / Effects / Markers panel on the left, the
editor in the centre, Selection/View and History on the right, transport and level meters along the
bottom. It runs on Windows, macOS and Linux from one codebase.

Audemo is an independent project and is not affiliated with Adobe.

The interface uses Adobe's open-source Source Sans 3 and Source Code Pro typefaces, bundled under the SIL Open Font License (see `assets/fonts/`).

## Features

- **Open** WAV, AIFF, FLAC, MP3, OGG/Vorbis, M4A/AAC, ALAC, CAF and MKV/WebM audio (drag files onto the window, or pass them on the command line).
- **Save** as WAV (16/24/32-bit integer or 32-bit float), FLAC (16/24-bit, lossless), MP3 (32–320 kbps CBR or V0–V9 VBR, via LAME) or AAC in M4A (64–320 kbps, via Fraunhofer FDK AAC), with optional TPDF dither. Saving runs in the background with a progress bar and Cancel, and writes to a temporary file first so a failed save never damages the original. MP3 and M4A files are gapless (encoder delay and padding are recorded and removed on open).
- **Metadata** panel: title, artist, album, album artist, genre, year, track number, composer, comment and copyright, read from and written to every format (RIFF INFO, ID3v2, Vorbis comments, iTunes tags). WAV files also keep their **markers** as cue points.
- **Workspace** laid out like Audition's default: Files / Favorites, then Media Browser / Effects Rack / Markers / Properties, then History down the left; the Editor in the centre with its transport bar; Levels / Frequency Analysis / Phase Meter and Selection/View along the bottom; History / Match Loudness at the bottom left. Panel groups resize, and Window > Workspace > Reset to Default restores the layout.
- **Multitrack editor** (toolbar *Multitrack*, or 0): sessions of tracks with volume, pan, mute, solo and record-arm; drag files from the Files panel (or the desktop) onto tracks; move clips between tracks, trim their edges, drag fade handles, set clip gain, split at the playhead (Ctrl+K), with snapping to clip edges, the cursor and the selection. The mix plays live as you edit. Arm a track and record onto it while the other tracks play. **Volume and pan automation**: press A on a track to show its yellow (volume) and blue (pan) lines; drag or double-click a line to add points, drag points, double-click one to delete it, right-click to clear. Overlapping clips on a track **crossfade automatically** (equal-power). Peak **meters** on every track. **Recording latency compensation** (Preferences) lines overdubs up with what you heard. **Track effects racks**: every track, bus and the master has a 16-slot rack of real-time effects (EQs and filters, compressors, limiter, gate, de-esser, delay, echo, chorus/flanger, reverb, distortion, stereo tools), edited in the Effects Rack panel and heard live as the session plays. **Bus tracks** (Alt+B) take tracks routed to them (Out) and **sends** (pre- or post-fader) and have their own rack and fader. A **Mixer** view with faders, pan, meters, sends, output routing and rack access for every track, bus and the master. **Mixdown Session to New File** (entire session or time selection). Clips stay linked to their files, so edits made in the Waveform editor (double-click a clip) are heard in the session. Sessions save as `.audemo` files, with any unsaved audio written to a folder beside them.
- **Waveform editor** with an overview/zoom navigator, time ruler, per-channel lanes, amplitude ruler in dB, sample-level zoom and channel enable toggles (edit L or R only).
- **Spectral frequency display** (Shift+D) shown under the waveform, with **spectral editing**: Marquee (E), Lasso (D) and Paintbrush (P) tools select an area of time × frequency; any effect then changes only that area, Delete silences it, and **Auto Heal** (Ctrl+U) rebuilds it from the sound around it. The **Spot Healing Brush** (B) repairs whatever you paint over (clicks, coughs, chair squeaks, phone beeps) as soon as you let go.
- **Diagnostics** panel: scan a file or selection for clicks, clipping, silence or audio; jump to each finding, then Repair, Delete or Mark one or all.
- **Batch Process** panel: run the Effects Rack chain, Match Loudness or a Favorite over a list of files and save them as WAV, FLAC, MP3 or AAC (sample rate, folder, name suffix; tags and markers carried over), never overwriting anything.
- **Edit**: cut, copy, paste, paste to new, delete, crop, select all, convert sample rate and channel count.
- **Unlimited-style history**: 60-step undo/redo plus a History panel you can click to jump to any state.
- **Fade handles** in the top corners of the waveform and a floating **clip gain** control.
- **Markers** (M), with rename, jump and delete in the Markers panel.
- **Playback** through the system output (WASAPI, CoreAudio, ALSA/PulseAudio/PipeWire), looping, monitor volume, peak meters with hold and clip indicator.
- **Recording** (Shift+Space) with the waveform drawn live as it records; Stop or Space ends it. Records into the selection or at the cursor, or into a new file if none is open.
- **Audio Hardware preferences** (Edit > Preferences) for choosing input and output devices; remembered between sessions along with recent files.
- **Real-time Effects Rack**: chain up to 16 effects with power switches, reordering, input/output gain and dry/wet mix. With the rack's master power on, normal playback (Space, looping, seeking) is heard through the chain as you adjust it; Apply renders it into the file.
- **Analysis**: Frequency Analysis (live FFT at the playhead, 1k–32k sizes, scan-selection average, hover readout), Phase Meter (goniometer and correlation), and Window > Amplitude Statistics (peak, true peak, RMS, clipping, DC offset, integrated loudness and loudness range).
- **Match Loudness** panel: measure integrated loudness (ITU-R BS.1770 / EBU R128), true peak and LRA for open files and match them to EBU R128, ATSC A/85, podcast or streaming targets in one run.
- **Media Browser** to navigate folders, audition files with Auto-Play and double-click to open; **Favorites** panel; **Properties** panel with peak and RMS.
- **File and Edit menus** with Open Append, Open Recent, Close All, Save Selection As, Save All, Copy to New, Mix Paste (insert / overlap / overwrite / modulate) and Repeat Previous Command.
- **Effect windows** with presets, live looping preview (Space) and bypass, plus a draggable frequency-response curve for the EQs and filters.
- **Favorites** menu for one-click common jobs.

## Effects (57)

| Menu | Effects |
|---|---|
| Effects | Invert, Reverse, Silence |
| Amplitude and Compression | Amplify, Normalize, Fade In, Fade Out, Dynamics Processing, Hard Limiter, Single-band Compressor, Tube-modeled Compressor, Multiband Compressor, DeEsser, DC Offset Correction, Match Loudness, Noise Gate, Speech Volume Leveler |
| Delay and Echo | Delay, Echo (with ping-pong), Analog Delay (tape / tube / BBD), Multitap Delay |
| Filter and EQ | Parametric Equalizer (5 bands + cut filters), Graphic Equalizer (10, 20 and 30 bands), Notch Filter, Scientific Filter (Butterworth LP/HP/BP/BS, up to 8-pole) |
| Modulation | Chorus, Flanger, Chorus/Flanger, Phaser |
| Noise Reduction / Restoration | Capture Noise Print, Noise Reduction (process), Adaptive Noise Reduction, DeHummer (50/60 Hz + harmonics), Click/Pop Eliminator, DeClipper, Hiss Reduction, Delete Silence |
| Reverb | Studio Reverb (algorithmic), Full Reverb (convolution with a synthesised impulse response) |
| Special | Distortion (soft/hard/tube/foldback/bit-crush), Doppler Shifter, Guitar Suite, Mastering, Vocal Enhancer |
| Stereo Imagery | Channel Mixer, Stereo Expander, Centre Channel Extractor, Automatic Phase Correction, Pan |
| Time and Pitch | Stretch and Pitch (phase vocoder with phase locking), Pitch Shifter, Varispeed |
| Generate | Tones (sine/square/triangle/saw, sweeps), Noise (white/pink/brown), DTMF Tones, Silence |

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
| Record (Stop or Space ends it) | Shift+Space |
| Undo / redo | Ctrl+Z / Ctrl+Shift+Z (Ctrl+Y) |
| Cut / copy / paste | Ctrl+X / Ctrl+C / Ctrl+V |
| Copy to New / Paste to New | Alt+Shift+C / Ctrl+Alt+V |
| Mix Paste | Ctrl+Shift+V |
| Repeat previous command | Shift+R |
| Delete / crop | Delete / Ctrl+T |
| Select all / deselect | Ctrl+A / Esc |
| Add marker / previous / next | M / ← / → |
| Zoom in / out / full | = / - / \\ |
| Zoom time at pointer | Mouse wheel |
| Scroll | Shift+wheel, middle-drag, or the Hand tool |
| Zoom amplitude | Alt+wheel |
| Spectral display | Shift+D |
| Capture noise print | Shift+P |
| Tools: Time Selection / Hand / Marquee / Lasso / Paintbrush / Spot Healing | T / H / E / D / P / B |
| Auto Heal spectral selection | Ctrl+U |
| New multitrack session / new audio file | Ctrl+N / Ctrl+Shift+N |
| Waveform / Multitrack editor | 9 / 0 |
| Add audio track / bus track (Multitrack) | Alt+A / Alt+B |
| Split clip at playhead (Multitrack) | Ctrl+K |
| Snap off while dragging clips | hold Alt |
| Scroll tracks | wheel over the track headers, or Alt+wheel |
| Show automation (Multitrack) | A on the track header |
| Add / delete automation point | double-click the line / double-click the point |
| Shape a fade | drag the fade handle up (fast rise) or down (slow rise); hold Ctrl (⌘) for a cosine S-curve |

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
  panels.rs      menus, toolbar, docked panel layout, transport, levels, status bar
  workspace.rs   Favorites, Media Browser, Effects Rack and Properties panels
  analysis_ui.rs Frequency Analysis, Phase Meter, Match Loudness, Amplitude Statistics
  liverack.rs    real-time Effects Rack renderer (and the shared stream renderer)
  tools_ui.rs    Diagnostics and Batch Process panels
  dsp/spectral.rs    spectral selections: masks, apply-inside-selection, healing
  dsp/effects/diagnose.rs  click / clipping / silence / audio detection and repair
  dsp/effects/rt.rs  streaming versions of 24 effects for track, bus and master racks
  session.rs     multitrack sessions: tracks, clips, mixing, undo, .audemo files
  mt_ui.rs       Multitrack editor, Mixer, recording to tracks, mixdown
  prefs.rs       saved preferences (audio devices, recent files)
  dialogs.rs     effect windows (presets, preview, EQ curve) and other dialogs
  engine.rs      cpal playback and recording
  io.rs          symphonia decoding, tags, markers, MP4 edit lists
  export/        WAV, FLAC (own encoder), MP3 (LAME) and M4A (FDK AAC) writers; tags
  theme.rs       palette and hand-drawn transport icons
assets/          app icon, macOS Info.plist, Linux .desktop file, WiX installer source
scripts/         macOS and Linux packaging scripts
  dsp/           dependency-free DSP: FFT, biquads, STFT, resampler,
                 peak cache, spectrogram, loudness (BS.1770), analysis
                 and every effect (+ tests)
```

Adding an effect means writing one `fn(&[Vec<f32>], &Ctx, &Params) -> Result<Vec<Vec<f32>>, String>`
and registering it with its parameters in `src/dsp/effects/`; the menu entry, dialog, presets and
preview are generated from that definition.

## Roadmap to Audition parity

1. ~~Recording fixes, Audio Hardware preferences, Audition workspace layout~~ (done)
2. ~~Real-time Effects Rack, remaining Audition effects, Match Loudness, analysis panels~~ (done)
3. Multitrack sessions: ~~tracks, clips, Mixer, mixdown, recording~~ (0.6), ~~automation, crossfades, track meters, latency compensation~~ (0.6.5), ~~track/bus/master effects racks, bus tracks, sends~~ (0.7) — done
4. ~~Spectral selection/healing tools, Diagnostics panel, batch processing~~ (0.8)
5. ~~MP3/FLAC/AAC export, metadata~~ (0.9); VST3/AU plug-in hosting next

## Third-party codecs

MP3 encoding uses [LAME](https://lame.sourceforge.io/) (LGPL 2.0) and AAC encoding uses the
[Fraunhofer FDK AAC](https://android.googlesource.com/platform/external/aac/) library (FDK AAC
licence), both compiled from source into the binary through the `mp3lame-encoder` and `fdk-aac`
crates. The FLAC encoder is Audemo's own. Decoding uses Symphonia (MPL 2.0).
