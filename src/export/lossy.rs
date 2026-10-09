//! MP3 (LAME) and AAC (Fraunhofer FDK) encoding.

use std::io::{Seek, SeekFrom, Write};

use super::{meta, mp4, ExportSettings, Metadata, Progress, Quantizer};

const MP3_RATES: [u32; 9] = [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000];
const AAC_RATES: [u32; 12] = [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000];

/// Pick a rate the codec supports and convert to it if needed. Returns
/// the (possibly resampled) audio, the rate, and at most two channels.
fn prepare(chs: &[Vec<f32>], rate: u32, allowed: &[u32]) -> (Vec<Vec<f32>>, u32) {
    let chs: Vec<Vec<f32>> = chs.iter().take(2).cloned().collect();
    if allowed.contains(&rate) {
        return (chs, rate);
    }
    let target = if rate % 11025 == 0 && allowed.contains(&44100) && rate > 44100 {
        44100
    } else {
        *allowed.iter().find(|r| **r >= rate).unwrap_or(&48000)
    };
    let out = crate::dsp::resample::resample_channels(&chs, target as f64 / rate as f64);
    (out, target)
}

pub fn write_mp3<W: Write + Seek>(
    out: &mut W,
    chs: &[Vec<f32>],
    rate: u32,
    s: &ExportSettings,
    meta: &Metadata,
    progress: &Progress,
) -> Result<(), String> {
    use mp3lame_encoder::{max_required_buffer_size, Bitrate, Builder, FlushGap, InterleavedPcm, Mode, MonoPcm, Quality, VbrMode};
    let err = |e: std::io::Error| format!("Write failed: {e}");
    let (chs, rate) = prepare(chs, rate, &MP3_RATES);
    let n = chs.len();
    let len = chs[0].len();
    let be = |e: mp3lame_encoder::BuildError| format!("MP3 encoder: {e}");
    let mut b = Builder::new().ok_or("Couldn't start the MP3 encoder.")?;
    b.set_num_channels(n as u8).map_err(be)?;
    b.set_sample_rate(rate).map_err(be)?;
    b.set_mode(if n == 1 { Mode::Mono } else { Mode::JointStereo }).map_err(be)?;
    b.set_quality(Quality::NearBest).map_err(be)?;
    if s.mp3_vbr {
        b.set_vbr_mode(VbrMode::Mtrh).map_err(be)?;
        b.set_vbr_quality(quality(s.mp3_vbr_quality)).map_err(be)?;
    } else {
        b.set_vbr_mode(VbrMode::Off).map_err(be)?;
        b.set_brate(match s.mp3_kbps {
            0..=32 => Bitrate::Kbps32,
            33..=48 => Bitrate::Kbps48,
            49..=64 => Bitrate::Kbps64,
            65..=96 => Bitrate::Kbps96,
            97..=128 => Bitrate::Kbps128,
            129..=160 => Bitrate::Kbps160,
            161..=192 => Bitrate::Kbps192,
            193..=224 => Bitrate::Kbps224,
            225..=256 => Bitrate::Kbps256,
            _ => Bitrate::Kbps320,
        })
        .map_err(be)?;
    }
    b.set_to_write_vbr_tag(true).map_err(be)?;
    let mut enc = b.build().map_err(be)?;

    let id3 = meta::id3v2(meta, &super::encoder_name());
    let start = out.stream_position().map_err(err)?;
    out.write_all(&id3).map_err(err)?;
    let ee = |e: mp3lame_encoder::EncodeError| format!("MP3 encoder: {e}");
    let block = 8192;
    let mut pcm: Vec<f32> = Vec::with_capacity(block * n);
    let mut buf: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < len {
        if progress.cancelled() {
            return Err("Cancelled".into());
        }
        let end = (i + block).min(len);
        buf.clear();
        buf.reserve(max_required_buffer_size((end - i) * n));
        if n == 1 {
            enc.encode_to_vec(MonoPcm(&chs[0][i..end]), &mut buf).map_err(ee)?;
        } else {
            pcm.clear();
            for f in i..end {
                pcm.push(chs[0][f]);
                pcm.push(chs[1][f]);
            }
            enc.encode_to_vec(InterleavedPcm(&pcm[..]), &mut buf).map_err(ee)?;
        }
        out.write_all(&buf).map_err(err)?;
        progress.set(end as f32 / len as f32);
        i = end;
    }
    buf.clear();
    buf.reserve(16384);
    enc.flush_to_vec::<FlushGap>(&mut buf).map_err(ee)?;
    out.write_all(&buf).map_err(err)?;
    // The first frame is a placeholder for the LAME/Xing tag (length, seek
    // table, encoder delay and padding); fill it in now.
    if enc.is_lame_tag_written() {
        let mut tag = Vec::with_capacity(enc.lame_tag_size().max(1));
        if enc.lame_tag_encode_to_vec(&mut tag).is_some() {
            let end = out.stream_position().map_err(err)?;
            out.seek(SeekFrom::Start(start + id3.len() as u64)).map_err(err)?;
            out.write_all(&tag).map_err(err)?;
            out.seek(SeekFrom::Start(end)).map_err(err)?;
        }
    }
    Ok(())
}

fn quality(q: u8) -> mp3lame_encoder::Quality {
    use mp3lame_encoder::Quality::*;
    match q {
        0 => Best,
        1 => SecondBest,
        2 => NearBest,
        3 => VeryNice,
        4 => Nice,
        5 => Good,
        6 => Decent,
        7 => Ok,
        8 => SecondWorst,
        _ => Worst,
    }
}

pub fn write_m4a<W: Write + Seek>(
    out: &mut W,
    chs: &[Vec<f32>],
    rate: u32,
    s: &ExportSettings,
    meta: &Metadata,
    progress: &Progress,
) -> Result<(), String> {
    use fdk_aac::enc::{AudioObjectType, BitRate, ChannelMode, Encoder, EncoderParams, Transport};
    let (chs, rate) = prepare(chs, rate, &AAC_RATES);
    let n = chs.len();
    let len = chs[0].len();
    let fe = |e: fdk_aac::enc::EncoderError| format!("AAC encoder: {e}");
    let enc = Encoder::new(EncoderParams {
        bit_rate: BitRate::Cbr(s.aac_kbps * 1000),
        sample_rate: rate,
        transport: Transport::Raw,
        channels: if n == 1 { ChannelMode::Mono } else { ChannelMode::Stereo },
        audio_object_type: AudioObjectType::Mpeg4LowComplexity,
    })
    .map_err(fe)?;
    let info = enc.info().map_err(fe)?;
    let frame_len = info.frameLength as usize;
    let delay = info.nDelay as usize;
    let asc = info.confBuf[..(info.confSize as usize).min(info.confBuf.len())].to_vec();
    if frame_len == 0 || asc.is_empty() {
        return Err("AAC encoder: unexpected configuration.".into());
    }
    let mut outbuf = vec![0u8; (info.maxOutBufBytes as usize).max(8192)];

    // The encoder has no end-of-stream call here, so feed enough silence
    // after the audio to push every real sample out; the edit list trims it.
    let padded = len + delay + 2 * frame_len;
    let mut q = Quantizer::new(s.dither);
    let mut frames: Vec<Vec<u8>> = Vec::with_capacity(padded / frame_len + 2);
    let mut pcm: Vec<i16> = Vec::with_capacity(frame_len * n);
    let mut i = 0;
    while i < padded {
        if progress.cancelled() {
            return Err("Cancelled".into());
        }
        let end = (i + frame_len).min(padded);
        pcm.clear();
        for f in i..end {
            for c in &chs {
                let v = if f < len { c[f] } else { 0.0 };
                pcm.push(q.quantize(v, 16) as i16);
            }
        }
        let mut off = 0;
        let mut stalls = 0;
        while off < pcm.len() {
            let r = enc.encode(&pcm[off..], &mut outbuf).map_err(fe)?;
            if r.output_size > 0 {
                frames.push(outbuf[..r.output_size].to_vec());
            }
            if r.input_consumed == 0 && r.output_size == 0 {
                stalls += 1;
                if stalls > 4 {
                    return Err("AAC encoder stopped accepting audio.".into());
                }
            } else {
                stalls = 0;
            }
            off += r.input_consumed;
        }
        progress.set((end.min(len) as f32 / len as f32) * 0.98);
        i = end;
    }
    // Any frames entirely past the audio and its delay are surplus.
    let needed = (len + delay + frame_len - 1) / frame_len;
    if frames.len() > needed {
        frames.truncate(needed);
    }
    if frames.len() < needed {
        return Err("AAC encoder returned too little audio.".into());
    }
    let track = mp4::AacTrack {
        asc,
        sample_rate: rate,
        channels: n as u16,
        frames,
        frame_len: frame_len as u32,
        delay: delay as u32,
        samples: len as u64,
    };
    let ilst = meta::mp4_items(meta, &super::encoder_name());
    mp4::write(out, &track, &ilst)?;
    progress.set(1.0);
    Ok(())
}
