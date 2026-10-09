//! Round trips through the real encoders and the decoder used to open
//! files. These need the real codec crates, so the type-checking stubs skip
//! them (`--cfg audemo_stub`).

use super::*;

fn signal(len: usize, rate: f32) -> Vec<Vec<f32>> {
    let l: Vec<f32> = (0..len).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate).sin() * 0.5).collect();
    let r: Vec<f32> = (0..len).map(|i| (i as f32 * 660.0 * std::f32::consts::TAU / rate).sin() * 0.3).collect();
    vec![l, r]
}

fn tags() -> Metadata {
    let mut m = Metadata::default();
    m.set(Tag::Title, "Round Trip ✓");
    m.set(Tag::Artist, "Audemo");
    m.set(Tag::Album, "Tests");
    m.set(Tag::Track, "3/12");
    m
}

fn dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("audemo-export-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
#[cfg_attr(audemo_stub, ignore)]
fn lossless_round_trips_are_exact() {
    let rate = 44100;
    let chs = signal(30_000, rate as f32);
    let markers = vec![(1000usize, "Intro".to_string()), (20_000, "Verse".to_string())];
    for (c, bits) in [(Container::Wav, 16u32), (Container::Wav, 24), (Container::Flac, 16), (Container::Flac, 24)] {
        let s = ExportSettings {
            container: c,
            wav: WavFormat::from_bits(Some(bits)),
            flac_bits: bits,
            dither: false,
            ..Default::default()
        };
        let path = dir().join(format!("rt{bits}.{}", c.ext()));
        save(&path, &chs, rate, &s, &tags(), &markers, &Progress::default()).unwrap();
        let dec = crate::io::load(&path).unwrap();
        assert_eq!(dec.sample_rate, rate);
        assert_eq!(dec.channels.len(), 2);
        assert_eq!(dec.channels[0].len(), chs[0].len(), "{path:?}");
        let scale = (1u64 << (bits - 1)) as f32;
        let mut q = Quantizer::new(false);
        for i in 0..chs[0].len() {
            for c in 0..2 {
                let want = q.quantize(chs[c][i], bits) as f32 / scale;
                assert!((dec.channels[c][i] - want).abs() < 1e-6, "{path:?} sample {i}: {} vs {want}", dec.channels[c][i]);
            }
        }
        assert_eq!(dec.meta.get(Tag::Title), "Round Trip ✓", "{path:?}");
        assert_eq!(dec.meta.get(Tag::Artist), "Audemo", "{path:?}");
        if c == Container::Wav {
            assert_eq!(dec.markers, markers);
        }
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
#[cfg_attr(audemo_stub, ignore)]
fn lossy_round_trips_keep_length_and_tags() {
    for (c, rate, ch) in [(Container::Mp3, 44100u32, 2usize), (Container::Mp3, 48000, 1), (Container::M4a, 44100, 2), (Container::M4a, 48000, 1)] {
        let mut chs = signal(48_000, rate as f32);
        chs.truncate(ch);
        let s = ExportSettings { container: c, mp3_kbps: 192, aac_kbps: 128, ..Default::default() };
        let path = dir().join(format!("rt{rate}_{ch}.{}", c.ext()));
        save(&path, &chs, rate, &s, &tags(), &[], &Progress::default()).unwrap();
        let dec = crate::io::load(&path).unwrap();
        assert_eq!(dec.sample_rate, rate, "{path:?}");
        assert_eq!(dec.channels.len(), ch, "{path:?}");
        let n = dec.channels[0].len() as i64;
        assert!((n - 48_000).abs() <= 3000, "{path:?}: {n} frames");
        // The tone survives: similar RMS in the middle of the file.
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        let a = rms(&chs[0][10_000..30_000]);
        let b = rms(&dec.channels[0][10_000..30_000]);
        assert!((a - b).abs() < 0.05, "{path:?}: rms {a} vs {b}");
        assert_eq!(dec.meta.get(Tag::Title), "Round Trip ✓", "{path:?}");
        assert_eq!(dec.meta.get(Tag::Album), "Tests", "{path:?}");
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
#[cfg_attr(audemo_stub, ignore)]
fn unusual_rates_are_converted_for_lossy_formats() {
    let chs = signal(20_000, 96_000.0);
    for c in [Container::Mp3, Container::M4a] {
        let path = dir().join(format!("hi.{}", c.ext()));
        let s = ExportSettings { container: c, ..Default::default() };
        save(&path, &chs, 37_800, &s, &Metadata::default(), &[], &Progress::default()).unwrap();
        let dec = crate::io::load(&path).unwrap();
        assert!(dec.sample_rate == 44100 || dec.sample_rate == 48000, "{c:?}: {}", dec.sample_rate);
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn a_failed_save_leaves_the_target_alone() {
    let path = dir().join("keep.wav");
    std::fs::write(&path, b"original").unwrap();
    let r = save(&path, &[Vec::new()], 44100, &ExportSettings::default(), &Metadata::default(), &[], &Progress::default());
    assert!(r.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    let p = Progress::default();
    p.cancel();
    let r = save(&path, &signal(100_000, 44100.0), 44100, &ExportSettings::default(), &Metadata::default(), &[], &p);
    assert!(r.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert!(!temp_path(&path).exists());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn settings_survive_the_preferences_file() {
    let s = ExportSettings { container: Container::Mp3, mp3_vbr: true, mp3_vbr_quality: 4, mp3_kbps: 256, aac_kbps: 192, dither: false, include_meta: false, wav: WavFormat::Pcm16, flac_bits: 16 };
    assert_eq!(ExportSettings::from_text(&s.to_text()), Some(s));
    assert_eq!(ExportSettings::from_text("nonsense"), None);
}

#[test]
fn id3_and_vorbis_tags_are_well_formed() {
    let id3 = meta::id3v2(&tags(), "Audemo");
    assert_eq!(&id3[..5], b"ID3\x03\x00");
    let size = id3[6..10].iter().fold(0usize, |a, b| (a << 7) | (*b as usize & 0x7F));
    assert_eq!(size + 10, id3.len());
    let vc = meta::vorbis_comment(&tags(), "Audemo");
    let vendor = u32::from_le_bytes([vc[0], vc[1], vc[2], vc[3]]) as usize;
    let count = u32::from_le_bytes(vc[4 + vendor..8 + vendor].try_into().unwrap());
    assert_eq!(count, 4);
    assert!(meta::id3v2(&Metadata::default(), "").is_empty());
    assert!(meta::riff_info(&Metadata::default(), "").is_empty());
}
