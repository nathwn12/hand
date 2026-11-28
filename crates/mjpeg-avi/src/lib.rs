//! Dependency-free MJPEG AVI writer shared by hand's browser and desktop engines.
//!
//! One RIFF `AVI ` stream: `hdrl` (56-byte `avih` + `strl`/`strh`/`strf`),
//! `movi` with one `00dc` chunk per JPEG frame, and an `idx1` index whose
//! offsets are relative to the `movi` FOURCC. Odd-length chunks carry a zero
//! pad byte that is NOT counted in the chunk length. No decoder, ffmpeg or
//! external binary is involved: JPEG dimensions come from the SOF marker.
//!
//! This is the writer that used to live in `browser_engine::trace`. Moving it
//! here does not change a byte of its output: `browser_trace` and `desktop_record`
//! call the same function and produce the same container.
//!
//! The stream is constant-frame-rate: `dwRate=fps`, `dwScale=1`, with no
//! per-frame timestamps. Playback time is therefore `frames.len() / fps`.
//! Callers that miss a capture slot must repeat the previous JPEG in that
//! slot so the file keeps its real-time duration; a short frame list plays
//! back faster than the wall-clock window it represents.

fn le16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn le32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn fourcc(out: &mut Vec<u8>, id: &[u8; 4]) {
    out.extend_from_slice(id);
}

/// One RIFF chunk. Odd data lengths get a trailing zero pad byte; the
/// chunk's own length never counts that pad.
fn chunk(out: &mut Vec<u8>, id: &[u8; 4], data: &[u8]) {
    fourcc(out, id);
    le32(out, data.len() as u32);
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
}

fn list(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 12);
    fourcc(&mut out, b"LIST");
    le32(&mut out, (data.len() + 4) as u32);
    fourcc(&mut out, id);
    out.extend_from_slice(data);
    out
}

/// JPEG dimensions from the SOF marker, without a decoder. CDP screencast
/// frames and desktop PrintWindow JPEGs both carry a real SOF header, so the
/// AVI `strh`/`strf` can name the true frame size.
pub fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 0usize;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
            if width > 0 && height > 0 {
                return Some((width, height));
            }
        }
        i += 2;
    }
    None
}

/// One stream, constant-frame-rate MJPEG AVI: RIFF 'AVI ' with hdrl
/// (avih + strl/strh/strf), movi ('00dc' JPEG chunks) and an idx1 index.
/// Every chunk is one `1/fps` tick; no per-frame timestamps are written, so
/// callers must repeat the last frame to represent a real-time gap and keep
/// `frames.len() / fps` equal to the intended duration. No external binaries
/// or dependencies: all headers are written by hand.
pub fn avi_mjpeg(frames: &[Vec<u8>], fps: u32) -> Vec<u8> {
    let fps = fps.clamp(1, 60);
    let (width, height) = frames
        .first()
        .and_then(|f| jpeg_dimensions(f))
        .unwrap_or((320, 240));
    let max_frame = frames.iter().map(|f| f.len() as u32).max().unwrap_or(0);
    let usec = 1_000_000u32 / fps;

    let mut avih = Vec::with_capacity(56);
    le32(&mut avih, usec);
    le32(&mut avih, max_frame.saturating_mul(fps));
    le32(&mut avih, 0);
    le32(&mut avih, 0x10); // AVIF_HASINDEX
    le32(&mut avih, frames.len() as u32);
    le32(&mut avih, 0);
    le32(&mut avih, 1);
    le32(&mut avih, max_frame);
    le32(&mut avih, width);
    le32(&mut avih, height);
    // AVIMAINHEADER is 56 bytes: four reserved DWORDs follow dwHeight.
    // A 40-byte avih desyncs strict parsers from the following strl LIST.
    for _ in 0..4 {
        le32(&mut avih, 0);
    }

    let mut strh = Vec::with_capacity(56);
    fourcc(&mut strh, b"vids");
    fourcc(&mut strh, b"MJPG");
    le32(&mut strh, 0);
    le16(&mut strh, 0);
    le16(&mut strh, 0);
    le32(&mut strh, 0);
    le32(&mut strh, 1); // dwScale
    le32(&mut strh, fps); // dwRate
    le32(&mut strh, 0);
    le32(&mut strh, frames.len() as u32);
    le32(&mut strh, max_frame);
    le32(&mut strh, 0xFFFF_FFFF);
    le32(&mut strh, 0); // dwSampleSize
    for v in [0i16, 0, width as i16, height as i16] {
        le16(&mut strh, v as u16); // rcFrame
    }

    let mut strf = Vec::with_capacity(40);
    le32(&mut strf, 40);
    le32(&mut strf, width);
    le32(&mut strf, height);
    le16(&mut strf, 1);
    le16(&mut strf, 24);
    fourcc(&mut strf, b"MJPG");
    le32(
        &mut strf,
        max_frame.max(width.saturating_mul(height).saturating_mul(3)),
    );
    for _ in 0..4 {
        le32(&mut strf, 0);
    }

    let mut strl = Vec::new();
    chunk(&mut strl, b"strh", &strh);
    chunk(&mut strl, b"strf", &strf);
    let strl_list = list(b"strl", &strl);
    let mut hdrl = Vec::new();
    chunk(&mut hdrl, b"avih", &avih);
    hdrl.extend_from_slice(&strl_list);
    let hdrl_list = list(b"hdrl", &hdrl);

    let mut movi_data = Vec::new();
    let mut index: Vec<(u32, u32)> = Vec::with_capacity(frames.len());
    for frame in frames {
        // idx1 offsets are relative to the 'movi' fourcc; the first chunk
        // header starts at offset 4.
        let offset = 4u32 + movi_data.len() as u32;
        chunk(&mut movi_data, b"00dc", frame);
        index.push((offset, frame.len() as u32));
    }
    let movi_list = list(b"movi", &movi_data);

    let mut idx1 = Vec::with_capacity(frames.len() * 16);
    for (offset, len) in index {
        fourcc(&mut idx1, b"00dc");
        le32(&mut idx1, 0x10); // AVIIF_KEYFRAME: each JPEG is standalone
        le32(&mut idx1, offset);
        le32(&mut idx1, len);
    }

    let mut body = Vec::new();
    body.extend_from_slice(&hdrl_list);
    body.extend_from_slice(&movi_list);
    chunk(&mut body, b"idx1", &idx1);

    let mut out = Vec::with_capacity(body.len() + 12);
    fourcc(&mut out, b"RIFF");
    le32(&mut out, (body.len() + 4) as u32);
    fourcc(&mut out, b"AVI ");
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::{avi_mjpeg, jpeg_dimensions};

    fn le32_at(buf: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(buf[at..at + 4].try_into().unwrap())
    }

    fn find(buf: &[u8], needle: &[u8]) -> Option<usize> {
        buf.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn avi_writer_emits_riff_movi_chunks_and_index() {
        // Two odd-length frames and one even: every odd chunk must carry a
        // zero pad byte that is NOT counted in its chunk length.
        let frames = vec![
            vec![0xFF, 0xD8, 0x01],
            vec![0xFF, 0xD8, 0x02, 0x03],
            vec![0xFF, 0xD8, 0x04, 0x05, 0xFF],
        ];
        let avi = avi_mjpeg(&frames, 10);
        // Independent spec checks: nothing below reuses the writer's
        // running offset arithmetic.
        assert_eq!(&avi[0..4], b"RIFF", "RIFF magic");
        assert_eq!(&avi[8..12], b"AVI ", "AVI form type");
        assert_eq!(
            le32_at(&avi, 4) as usize,
            avi.len() - 8,
            "RIFF size = file - 8"
        );
        for id in [
            &b"hdrl"[..],
            &b"avih"[..],
            &b"strl"[..],
            &b"strh"[..],
            &b"strf"[..],
        ] {
            assert!(find(&avi, id).is_some(), "missing {id:?}");
        }
        let avih = find(&avi, b"avih").expect("avih chunk");
        assert_eq!(le32_at(&avi, avih + 4), 56, "AVIMAINHEADER is 56 bytes");
        let strl = avih + 8 + 56; // hard spec constant, not writer arithmetic
        assert_eq!(&avi[strl..strl + 4], b"LIST", "strl LIST follows avih");
        assert_eq!(&avi[strl + 8..strl + 12], b"strl", "strl list type");

        let movi = find(&avi, b"movi").expect("movi list");
        let idx = find(&avi, b"idx1").expect("idx1 index");
        assert!(movi < idx, "movi precedes idx1");

        // Every '00dc' FOURCC before idx1 is a movi chunk. idx1 repeats
        // those FOURCCs after itself, so it is excluded from this count.
        let chunks: Vec<usize> = avi[..idx]
            .windows(4)
            .enumerate()
            .filter_map(|(at, w)| (w == b"00dc").then_some(at))
            .collect();
        assert_eq!(chunks.len(), frames.len(), "one 00dc chunk per frame");
        for (at, frame) in chunks.iter().zip(&frames) {
            let len = le32_at(&avi, at + 4) as usize;
            assert_eq!(len, frame.len(), "chunk length excludes pad");
            assert_eq!(&avi[at + 8..at + 8 + len], frame.as_slice());
            if len % 2 == 1 {
                assert_eq!(avi[at + 8 + len], 0, "odd frame is zero padded");
            }
        }

        let idx_entries = le32_at(&avi, idx + 4) as usize / 16;
        assert_eq!(idx_entries, chunks.len(), "idx1 count equals 00dc count");
        let first_offset = le32_at(&avi, idx + 16) as usize;
        assert_eq!(
            first_offset, 4,
            "first idx1 offset is 4 from the movi FOURCC"
        );
        assert_eq!(&avi[movi + first_offset..movi + first_offset + 4], b"00dc");
        for (k, &chunk_at) in chunks.iter().enumerate() {
            let entry = idx + 8 + k * 16;
            assert_eq!(&avi[entry..entry + 4], b"00dc");
            let offset = le32_at(&avi, entry + 8) as usize;
            let len = le32_at(&avi, entry + 12) as usize;
            assert_eq!(movi + offset, chunk_at, "idx1 points at a real movi chunk");
            assert_eq!(len, le32_at(&avi, chunk_at + 4) as usize);
        }
    }

    #[test]
    fn avih_is_56_bytes_and_strl_starts_after_it() {
        let avi = avi_mjpeg(&[vec![0xFF, 0xD8, 0x01]], 10);
        let avih = find(&avi, b"avih").expect("avih chunk");
        assert_eq!(le32_at(&avi, avih + 4), 56, "AVIMAINHEADER size is 56");
        let strl = avih + 8 + 56;
        assert_eq!(&avi[strl..strl + 4], b"LIST");
        assert_eq!(
            &avi[strl + 8..strl + 12],
            b"strl",
            "56-byte avih leaves strl here"
        );
    }

    #[test]
    fn jpeg_dimensions_read_sof_without_a_decoder() {
        // SOF0: FFC0, length 11, precision 8, height 240, width 320.
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0xF0, 0x01, 0x40, 0xFF, 0xD9,
        ];
        assert_eq!(jpeg_dimensions(&jpeg), Some((320, 240)));
        assert_eq!(jpeg_dimensions(&[0xFF, 0xD8, 0xFF, 0xD9]), None);
        let avi = avi_mjpeg(&[jpeg.to_vec()], 8);
        let avih = find(&avi, b"avih").expect("avih chunk");
        assert_eq!(le32_at(&avi, avih + 8 + 32), 320, "avih width slot");
        assert_eq!(le32_at(&avi, avih + 8 + 36), 240, "avih height slot");
    }
}
