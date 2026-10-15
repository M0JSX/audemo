# Audemo

A native, cross-platform waveform audio editor written in Rust. The workspace follows the layout of a
professional waveform editor: menu bar, toolbar, Files / Effects / Markers panel on the left, the
editor in the centre, Selection/View and History on the right, transport and level meters along the
bottom. It runs on Windows, macOS and Linux from one codebase.

Audemo is an independent project and is not affiliated with Adobe.

The interface uses Adobe's open-source Source Sans 3 and Source Code Pro typefaces, bundled under the SIL Open Font License (see `assets/fonts/`).

## Features

- **Long files and recordings**: audio is held in shared 64k-frame chunks, and whatever doesn't fit the memory budget (Preferences > Memory; a quarter of installed RAM by default) moves to a working file in the temporary folder and is read back as needed. Hours-long files open, play, record, edit, undo and save without being loaded whole: files are decoded and written block by block, edits and undo steps keep only the chunks they change, and Invert, Silence, Amplify, Normalize and the fades work a block at a time. Effects that need the whole selection at once (Noise Reduction, Stretch and the like) load just the selection and ask first when it's longer than 30 minutes.
- **Open** WAV, AIFF, FLAC, MP3, OGG/Vorbis, M4A/AAC, ALAC, CAF and MKV/WebM audio (drag files onto the window, or pass them on the command line).
- **Save** as WAV (16/24/32-bit integer or 32-bit float), FLAC (16/24-bit, lossless), MP3 (32–320 kbps CBR or V0–V9 VBR, via LAME) or AAC in M4A (64–320 kbps, via Fraunhofer FDK AAC), with optional TPDF dither. Saving runs in the background with a progress bar and Cancel, and writes to a temporary file first so a failed save never damages the original. MP3 and M4A files are gapless (encoder delay and padding are recorded and removed on open).
- **Plug-ins**: VST3 effects on Windows, macOS and Linux, and Audio Units on macOS, from Effects > Audio Units / VST 3 and in every effects rack (waveform rack, track, bus and master racks), batch processing and previews. **Effects > Audio Plug-In Manager** scans the standard folders (plus any you add) in a separate process, so a plug-in that crashes is listed instead of closing Audemo, and lets you enable or disable each one. Each plug-in's effect window shows its parameters, and **Show Plug-In Window** opens the plug-in's own interface (Windows and macOS). Plug-in settings are saved with sessions and rack slots; a session whose plug-in isn't installed keeps the slot and its settings and passes audio through until it's available. Plug-in latency is compensated when applying effects.
- **Metadata** panel: title, artist, album, album artist, genre, year, track number, composer, comment and copyright, read from and written to every format (RIFF INFO, ID3v2, Vorbis comments, iTunes tags). WAV files also keep their **markers** as cue points.
- **Workspace** laid out like Audition's default: Files / Favorites, then Media Browser / Effects Rack / Markers / Properties, then History down the left; the Editor in the centre with its transport bar; Levels / Frequency Analysis / Phase Meter and Selection/View along the bottom; History / Match Loudness at the bottom left. Panel groups resize, and Window > Workspace > Reset to Default restores the layout.
- **Multitrack editor** (toolbar *Multitrack*, or 0): sessions of tracks with volume, pan, mute, solo and record-arm; drag files from the Files panel (or the desktop) onto tracks; move clips between tracks, trim their edges, drag fade handles, set clip gain, split at the playhead (Ctrl+K), **clip volume and pan envelopes** (yellow and blue lines on each clip: double-click a line to add a point, drag points, double-click a point to delete it; they stay with the audio when the clip is trimmed, split or stretched), with snapping to clip edges, the cursor and the selection. The mix plays live as you edit. Arm a track and record onto it while the other tracks play. **Volume and pan automation**: press A on a track to show its yellow (volume) and blue (pan) lines; drag or double-click a line to add points, drag points, double-click one to delete it, right-click to clear. Overlapping clips on a track **crossfade automatically** (equal-power). Peak **meters** on every track. **Recording latency compensation** (Preferences) lines overdubs up with what you heard. **Track effects racks**: every track, bus and the master has a 16-slot rack of real-time effects (EQs and filters, compressors, limiter, gate, de-esser, delay, echo, chorus/flanger, reverb, distortion, stereo tools), edited in the Effects Rack panel and heard live as the session plays. **Bus tracks** (Alt+B) take tracks routed to them (Out) and **sends** (pre- or post-fader) and have their own rack and fader. A **Mixer** view with faders, pan, meters, sends, output routing and rack access for every track, bus and the master. **Mixdown Session to New File** (entire session or time selection). **Surround sessions**: a 5.1 or 7.1 master (File > New > Multitrack Session, or Multitrack > Master Channels) with a **surround panner** on every track and bus in the Mixer (drag the track around the room; spread, centre-speaker amount and LFE send), **surround tracks** that play a 5.1/7.1 file's channels straight to the matching master channels (set automatically when you drop a surround file on an empty track), per-channel master meters, and mixdowns in the session's format. The master effects rack is stereo, so it's bypassed in surround sessions. **Clip groups** (Ctrl+G) move, select and colour together; clips can be **looped** (drag the right edge past the source to repeat it), **locked in time**, **stretched** (Clip > Stretch, keeping pitch) and given their own **clip effects rack**. **Bounce** a track, the time selection or the selected clips to a new clip. **Sidechain** inputs for the Compressor and Noise Gate from any track or bus. **Parameter automation lanes** for mute and any effect parameter in a track's rack, alongside volume and pan. **Tempo** (bpm, time signature), a bar/beat **grid** with snapping and a **metronome**. **Session markers** (M): cue, CD track, cart timer and subclip markers, points or ranges over the time selection, shown as flags in the ruler with lines through the tracks; drag to move (snapping to clips and the cursor), drag a range's end, right-click to rename, change type or delete, double-click to edit in the Markers panel; previous/next marker, CSV import/export, undo, and saved in `.sesx` (as Audition markers) and `.ases` files. **Essential Sound** panel: tag clips as Dialogue, Music, SFX or Ambience and get one-click loudness matching, repair, clarity, EQ presets, reverb and **auto-ducking** of music under dialogue. **Remix** fits a music clip to a new length by re-joining it at beat boundaries. **Export to Final Cut Pro XML** (File > Export > Session to FCP XML) for Premiere Pro, Final Cut and DaVinci Resolve. Clips stay linked to their files, so edits made in the Waveform editor (double-click a clip) are heard in the session. Sessions save as Audemo's own `.ases` files by default, or as **Adobe Audition `.sesx`** files (choose it in Save As; Audition CC's format: tracks, buses and routing, volume, pan, mute, solo, clips with trim, gain, fades, loop and lock, clip and track volume and pan automation, tempo, 5.1/7.1 master and surround tracks), so Audition can open them; Audemo's own complete copy rides along inside the file, so everything Audition lacks (clip effects, sends, Essential Sound, automation lanes, groups, racks) comes back when Audemo reopens it. Audition's own `.sesx` sessions open too (without their effects, whose settings are Adobe's). Sessions saved as `.audemo` before 1.6 still open. Unsaved audio is written to a "<session> Files" folder beside the session.
- **Waveform editor** with an overview/zoom navigator, time ruler, per-channel lanes, amplitude ruler in dB, sample-level zoom and channel enable toggles (edit L or R only).
- **Spectral frequency display** (Shift+D) shown under the waveform, with **spectral editing**: Marquee (E), Lasso (D) and Paintbrush (P) tools select an area of time × frequency; any effect then changes only that area, Delete silences it, and **Auto Heal** (Ctrl+U) rebuilds it from the sound around it. The **Spot Healing Brush** (B) repairs whatever you paint over (clicks, coughs, chair squeaks, phone beeps) as soon as you let go.
- **Diagnostics** panel: scan a file or selection for clicks, clipping, silence or audio; jump to each finding, then Repair, Delete or Mark one or all.
- **Batch Process** panel: run the Effects Rack chain, Match Loudness or a Favorite over a list of files and save them as WAV, FLAC, MP3 or AAC (sample rate, folder, name suffix; tags and markers carried over), never overwriting anything.
- **Edit**: cut, copy, paste, paste to new, delete, crop, select all, convert sample rate and channel count.
- **Unlimited-style history**: 60-step undo/redo plus a History panel you can click to jump to any state.
- **Fade handles** in the top corners of the waveform and a floating **clip gain** control.
- **Markers and ranges** (M; with a selection and stopped, M adds a range marker). Cue, CD Track, Cart Timer and Subclip types; drag points and range edges in the timeline; rename, describe, merge, delete, convert between range and point, Select All. Markers follow edits and are stored in WAV files (cue points, labelled-text ranges and notes). **Import/Export Markers** as Audition-compatible CSV, **Export Audio** of each range to its own file, and a **Playlist** panel that plays ranges in order (with loop counts) or renders them to a new file.
- **Zero crossings** (Edit > Zero Crossings: adjust the selection inward, outward, or move the start/end left or right) and **snapping** to markers, the ruler and zero crossings.
- **Punch-in** (Punch In (Record over Selection), Ctrl+Shift+Space, in the Waveform and Multitrack editors): record over the time selection with pre-roll, keeping the audio either side.
- **Keyboard shortcut editor** (Edit > Keyboard Shortcuts): reassign any command, with conflict warnings and reset; the macOS menu shows your keys.
- **Pitch display** (View > Show Pitch Display): the detected pitch curve over the spectral display, used by Manual Pitch Correction.
- **Surround**: open, create (File > New, 5.1 and 7.1) and convert to 5.1/7.1, with labelled channels (L R C LFE Ls Rs …). WAV files are written as WAVE_FORMAT_EXTENSIBLE with the speaker mask. Playback uses as many outputs as the device has and folds down to stereo (ITU coefficients) otherwise; MP3 and AAC exports are folded to stereo.
- **Video files**: the audio track of MOV, MP4, M4V, MKV, WebM and 3GP files opens like any audio file (the picture isn't shown).
- **CD Layout** (View > CD Layout): build a track list from open files or CD Track markers, set pauses, titles and performers (CD-Text), and export a single WAV with a CUE sheet for burning software.
- **Control surfaces**: Mackie Control (and compatibles such as the Behringer X-Touch, Icon and PreSonus FaderPort in MCU mode) over MIDI, set up under Preferences > Control Surface: transport, eight track faders plus the master, pan, mute/solo/arm and bank buttons, with motor-fader feedback.
- **Audio drivers:** WASAPI and **ASIO** on Windows, Core Audio on macOS, ALSA (PulseAudio/PipeWire) and **JACK** on Linux, chosen under Edit > Preferences > Audio Hardware > Device Class. With ASIO one driver handles input and output, the I/O buffer size is passed to the driver and Settings… opens its control panel. JACK uses the server's rate and buffer size; Audemo's ports (Audemo_out, Audemo_in) connect to the system ports, and libjack is only loaded when JACK is chosen.
- **Playback** through the chosen device, looping, monitor volume, peak meters with hold and clip indicator.
- **Recording** (Shift+Space) with the waveform drawn live as it records; Stop or Space ends it. Records into the selection or at the cursor, or into a new file if none is open.
- **Preferences** (Edit > Preferences, Ctrl+, / ⌘,) with Audition's 17 pages: General (startup, tool tips, zoom factors, auto-scroll), Appearance (presets, element colours with hue/saturation, brightness from darkest to light, gradients, interface scale), Audio Channel Mapping (which device channels Audemo's L/R play and record on), Audio Hardware (device class: WASAPI/ASIO, Core Audio, ALSA/JACK; input and output devices or the ASIO driver, I/O buffer size, sample rate, force hardware to the file's sample rate, system audio settings), Auto Save (timed backups of sessions and edited files, how many to keep (3 by default), where; removed when Audemo quits normally, and offered for recovery at the next start if it didn't), Data (dither default, smoothing delete/cut and paste boundaries, sample rate conversion quality), Effects (open plug-in windows automatically, scan at startup), Markers & Metadata, Memory (installed RAM, audio in memory and on disk, memory budget, undo levels), Multitrack (pan mode: L/R cut or equal power; recording latency), Multitrack Clips (automatic crossfades and their curve, default clip fades, clip names), Playback and Recording (return playhead on stop, follow, play only the channels enabled for editing, pre-roll/post-roll for View > Play with Pre-roll and Post-roll, Alt+Space), Spectral Displays (window function, resolution, decibel range, logarithmic frequency scale) and Time Display (decimal, CD 75 fps, SMPTE 30/29.97 drop/29.97/25/24, samples, bars and beats, custom fps). Control Surface sets up a Mackie Control device; Media & Disk Cache and Video are there too, saying what Audemo does instead.
- **Real-time Effects Rack**: chain up to 16 effects with power switches, reordering, input/output gain and dry/wet mix. With the rack's master power on, normal playback (Space, looping, seeking) is heard through the chain as you adjust it; Apply renders it into the file.
- **Analysis**: Frequency Analysis (live FFT at the playhead, 1k–32k sizes, scan-selection average, hover readout), Phase Meter (goniometer and correlation), and Window > Amplitude Statistics (peak, true peak, RMS, clipping, DC offset, integrated loudness and loudness range).
- **Match Loudness** panel: measure integrated loudness (ITU-R BS.1770 / EBU R128), true peak and LRA for open files and match them to EBU R128, ATSC A/85, podcast or streaming targets in one run.
- **Files panel** lists open sessions and files: click to select, Ctrl/⌘-click and Shift-click to select several, drag from empty space (or from any row with Ctrl/⌘ held, adding to the selection) to sweep-select a run, Ctrl/⌘+A (with the panel clicked) to select all, and Delete/Backspace or right-click > Close Selected to close them; each unsaved one asks whether to save first, and Cancel stops there.
- **Media Browser** to navigate folders, audition files with Auto-Play and double-click to open; **Favorites** panel; **Properties** panel with peak and RMS.
- **File and Edit menus** with Open Append, Open Recent, Close All, Save Selection As, Save All, Copy to New, Mix Paste (insert / overlap / overwrite / modulate) and Repeat Previous Command.
- **Effect windows** with presets, live looping preview (Space) and bypass, plus a draggable frequency-response curve for the EQs and filters.
- **Favorites** menu for one-click common jobs.
- **Check for Updates** (Help menu, and at startup unless switched off in Preferences > General): asks GitHub for the latest release and, when it's newer, shows what's new. **Install and Restart** downloads the package for how Audemo is installed (the .msi or portable .zip on Windows, the .zip on macOS, the AppImage, tarball or .deb on Linux), checks its SHA-256 against GitHub's, swaps it in and restarts, offering to save unsaved work first; a .deb is handed to the system's software installer. Download, Skip This Version and Later are there too.

## Effects (66)

| Menu | Effects |
|---|---|
| Effects | Invert, Reverse, Silence |
| Amplitude and Compression | Amplify, Normalize, Fade In, Fade Out, Fade Envelope, Gain Envelope, Dynamics Processing, Hard Limiter, Single-band Compressor, Tube-modeled Compressor, Multiband Compressor, DeEsser, DC Offset Correction, Match Loudness, Noise Gate, Speech Volume Leveler |
| Delay and Echo | Delay, Echo (with ping-pong), Analog Delay (tape / tube / BBD), Multitap Delay |
| Filter and EQ | Parametric Equalizer (5 bands + cut filters), Graphic Equalizer (10, 20 and 30 bands), Notch Filter, Scientific Filter (Butterworth LP/HP/BP/BS, up to 8-pole), FFT Filter (drawn curve) |
| Modulation | Chorus, Flanger, Chorus/Flanger, Phaser |
| Noise Reduction / Restoration | Capture Noise Print, Noise Reduction (process), Adaptive Noise Reduction, DeHummer (50/60 Hz + harmonics), Click/Pop Eliminator, DeClipper, Hiss Reduction, Delete Silence, Learn Sound Model, Sound Remover |
| Reverb | Studio Reverb (algorithmic), Full Reverb (convolution with a synthesised impulse response) |
| Special | Distortion (soft/hard/tube/foldback/bit-crush), Doppler Shifter, Guitar Suite, Mastering, Vocal Enhancer, Frequency Band Splitter |
| Stereo Imagery | Channel Mixer, Stereo Expander, Centre Channel Extractor, Automatic Phase Correction, Pan, Graphic Phase Shifter |
| Time and Pitch | Stretch and Pitch (phase vocoder with phase locking), Pitch Shifter, Varispeed, Pitch Bender, Automatic Pitch Correction, Manual Pitch Correction (PSOLA) |
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
| Punch in | Ctrl+Shift+Space |
| Group / ungroup clips (Multitrack) | Ctrl+G / Ctrl+Shift+G |

On macOS use ⌘ instead of Ctrl. Every shortcut can be changed in Edit > Keyboard Shortcuts.

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
  audio.rs       chunked, shared audio with a memory budget and an on-disk working file
  editor.rs      waveform / spectral editor, selection, fades, gain HUD
  panels.rs      menus, toolbar, docked panel layout, transport, levels, status bar
  workspace.rs   Favorites, Media Browser, Effects Rack and Properties panels
  analysis_ui.rs Frequency Analysis, Phase Meter, Match Loudness, Amplitude Statistics
  liverack.rs    real-time Effects Rack renderer (and the shared stream renderer)
  tools_ui.rs    Diagnostics and Batch Process panels
  dsp/spectral.rs    spectral selections: masks, apply-inside-selection, healing
  dsp/effects/diagnose.rs  click / clipping / silence / audio detection and repair
  dsp/effects/rt.rs  streaming versions of 24 effects for track, bus and master racks
  session.rs     multitrack sessions: tracks, clips, mixing, undo, .ases files
  mt_ui.rs       Multitrack editor, Mixer, recording to tracks, mixdown
  prefs.rs       saved preferences (every setting, key=value file)
  prefs_ui.rs    the Preferences window
  autosave.rs    timed backups (Preferences > Auto Save)
  dialogs.rs     effect windows (presets, preview, EQ curve) and other dialogs
  engine.rs      cpal playback and recording
  io.rs          symphonia decoding, tags, markers, MP4 edit lists
  export/        WAV, FLAC (own encoder), MP3 (LAME) and M4A (FDK AAC) writers; tags
  plugin/        VST3 host (interfaces written from the MIT-licensed SDK headers), Audio Units
                 (macOS), plug-in windows, scanning, the Plug-In Manager's registry
  markers_ui.rs  Markers and Playlist panels, zero crossings, marker CSV
  shortcuts.rs   command table, keymap and the Keyboard Shortcuts editor
  essential.rs   Essential Sound panel, auto-ducking, Remix
  cd.rs          CD Layout window, CUE sheets
  interchange.rs Final Cut Pro XML export of sessions
  sesx.rs        Adobe Audition .sesx sessions (read and write)
  control_surface.rs  Mackie Control over MIDI (midir)
  dsp/pitch.rs   YIN pitch tracking and PSOLA
  dsp/remix.rs   beat analysis and re-joining for Remix
  dsp/effects/drawn.rs  curve-driven effects (envelopes, Pitch Bender, FFT Filter,
                 phase shifter), pitch correction, band splitter, Sound Remover
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
5. ~~MP3/FLAC/AAC export, metadata~~ (0.9), ~~VST3 and Audio Unit plug-in hosting~~ (0.10)
6. ~~Preferences, ASIO and JACK~~ (1.1–1.2)
7. ~~Range markers and Playlist, zero crossings, punch-in, shortcut editor; envelope, pitch, FFT, phase, band-splitter and Sound Remover effects; pitch correction; clip groups, clip effects, stretch/loop/lock, bounce, sidechain, parameter automation; Essential Sound and Remix; 5.1/7.1, tempo grid and metronome, video audio import, CD Layout, FCP XML export, control surfaces~~ (1.5)

8. ~~Long files (chunked, disk-backed audio), clip volume and pan envelopes, surround multitrack sessions~~ (1.7)

Not yet: OMF export, a video panel, effects racks on surround tracks and the surround master, control-surface protocols other than Mackie Control, and direct disc burning.

## Third-party code

The VST3 interface definitions are written from Steinberg's VST3 SDK headers, which are MIT-licensed. VST is a registered trademark of Steinberg Media Technologies GmbH.

### Codecs

MP3 encoding uses [LAME](https://lame.sourceforge.io/) (LGPL 2.0) and AAC encoding uses the
[Fraunhofer FDK AAC](https://android.googlesource.com/platform/external/aac/) library (FDK AAC
licence), both compiled from source into the binary through the `mp3lame-encoder` and `fdk-aac`
crates. The FLAC encoder is Audemo's own. Decoding uses Symphonia (MPL 2.0).

## Third-party notices

ASIO is a trademark and software of Steinberg Media Technologies GmbH. The
Windows build includes Steinberg's ASIO SDK host code for ASIO driver support.
