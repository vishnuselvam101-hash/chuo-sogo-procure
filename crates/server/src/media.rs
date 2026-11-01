//! Listing photos: size check and metadata removal before storage.
//!
//! Supplier photos often carry EXIF data (GPS position of the workshop, camera serials,
//! names). Only the image itself is kept: JPEG APP1–APP15 (except an ICC profile) and comment
//! segments, PNG text/EXIF/time chunks and WebP EXIF/XMP chunks are removed. Anything that
//! does not parse cleanly is refused rather than stored as is.

/// (width, height) from the image header.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let s = imagesize::blob_size(bytes).ok()?;
    Some((u32::try_from(s.width).ok()?, u32::try_from(s.height).ok()?))
}

/// The image without metadata, or `None` if it is not a well-formed image of `content_type`.
pub fn strip_metadata(content_type: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    match content_type {
        "image/jpeg" => strip_jpeg(bytes),
        "image/png" => strip_png(bytes),
        "image/webp" => strip_webp(bytes),
        _ => None,
    }
}

fn strip_jpeg(b: &[u8]) -> Option<Vec<u8>> {
    if !b.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut i = 2;
    loop {
        // Fill bytes between segments are allowed.
        while b.get(i) == Some(&0xFF) && b.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        if b.get(i) != Some(&0xFF) {
            return None;
        }
        let marker = *b.get(i + 1)?;
        match marker {
            // End of image: nothing after it is kept (data appended after EOI is dropped).
            0xD9 => {
                out.extend_from_slice(&[0xFF, 0xD9]);
                return Some(out);
            }
            // Markers without a length.
            0x01 | 0xD0..=0xD7 => {
                out.extend_from_slice(&b[i..i + 2]);
                i += 2;
                continue;
            }
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*b.get(i + 2)?, *b.get(i + 3)?]));
        if len < 2 {
            return None;
        }
        let end = i.checked_add(2 + len)?;
        let segment = b.get(i..end)?;
        // Kept: ICC colour profile (APP2) and the Adobe colour-transform flag (APP14).
        let icc = marker == 0xE2 && segment.get(4..16) == Some(b"ICC_PROFILE\0");
        let adobe = marker == 0xEE && segment.get(4..9) == Some(b"Adobe");
        let metadata = matches!(marker, 0xE1..=0xEF | 0xFE) && !icc && !adobe;
        if !metadata {
            out.extend_from_slice(segment);
        }
        i = end;
        if marker == 0xDA {
            // Entropy-coded scan data runs until the next real marker (FF00 is a stuffed
            // byte, FFD0–FFD7 are restart markers); progressive images have several scans.
            let start = i;
            loop {
                let ff = b.get(i..)?.iter().position(|&x| x == 0xFF)?;
                i += ff;
                match *b.get(i + 1)? {
                    0x00 | 0xD0..=0xD7 | 0xFF => i += 1,
                    _ => break,
                }
            }
            out.extend_from_slice(&b[start..i]);
        }
    }
}

fn strip_png(b: &[u8]) -> Option<Vec<u8>> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !b.starts_with(SIG) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(SIG);
    let mut i = SIG.len();
    let mut seen_end = false;
    while i < b.len() {
        let len = usize::try_from(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?)).ok()?;
        let kind = b.get(i + 4..i + 8)?;
        let end = i.checked_add(12)?.checked_add(len)?;
        let chunk = b.get(i..end)?;
        let crc = u32::from_be_bytes(b.get(end - 4..end)?.try_into().ok()?);
        if crc32(b.get(i + 4..end - 4)?) != crc {
            return None;
        }
        // Critical chunks (upper-case first letter) and rendering-relevant ancillary ones
        // only; text, EXIF, time and private chunks are dropped.
        let critical = kind[0].is_ascii_uppercase();
        let rendering = matches!(
            kind,
            b"tRNS" | b"gAMA" | b"cHRM" | b"sRGB" | b"iCCP" | b"pHYs" | b"sBIT" | b"bKGD"
        );
        if critical || rendering {
            out.extend_from_slice(chunk);
        }
        i = end;
        if kind == b"IEND" {
            seen_end = true;
            break;
        }
    }
    seen_end.then_some(out)
}

/// CRC-32 (IEEE) as used by PNG chunks.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn strip_webp(b: &[u8]) -> Option<Vec<u8>> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return None;
    }
    let riff_len = usize::try_from(u32::from_le_bytes(b[4..8].try_into().ok()?)).ok()?;
    let body_end = riff_len.checked_add(8)?;
    if body_end > b.len() {
        return None;
    }
    let mut chunks = Vec::new();
    let mut i = 12;
    while i < body_end {
        let kind = b.get(i..i + 4)?;
        let len =
            usize::try_from(u32::from_le_bytes(b.get(i + 4..i + 8)?.try_into().ok()?)).ok()?;
        let padded = len + (len & 1);
        let end = i.checked_add(8)?.checked_add(padded)?;
        let chunk = b.get(i..end.min(body_end))?;
        if !matches!(kind, b"EXIF" | b"XMP ") {
            let mut c = chunk.to_vec();
            // VP8X flags: clear "has EXIF" (0x08) and "has XMP" (0x04).
            if kind == b"VP8X" && c.len() > 8 {
                c[8] &= !0x0C;
            }
            chunks.push(c);
        }
        i = end;
    }
    let body: usize = chunks.iter().map(Vec::len).sum();
    let mut out = Vec::with_capacity(12 + body);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&u32::try_from(4 + body).ok()?.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    for c in chunks {
        out.extend_from_slice(&c);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn jpeg_loses_exif_and_comments_but_keeps_image_and_colour() {
        let mut j = vec![0xFF, 0xD8];
        j.extend(seg(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"));
        j.extend(seg(0xE1, b"Exif\0\0GPS 35.68N 139.76E"));
        j.extend(seg(0xE2, b"ICC_PROFILE\0\x01\x01data"));
        j.extend(seg(0xFE, b"photographer: Taro"));
        j.extend(seg(0xDB, &[0; 65]));
        j.extend(seg(0xEE, b"Adobe\0\x64\0\0\0\0\x01"));
        j.extend([0xFF, 0xDA, 0x00, 0x02, 0x12, 0x34, 0xFF, 0xD9]);
        // Payload appended after the end of the image.
        j.extend(b"Exif GPS 35.68N appended");
        let out = strip_jpeg(&j).unwrap();
        assert!(
            String::from_utf8_lossy(&out).contains("Adobe"),
            "colour transform kept"
        );
        let text = String::from_utf8_lossy(&out);
        assert!(!text.contains("GPS") && !text.contains("Taro"));
        assert!(text.contains("JFIF") && text.contains("ICC_PROFILE"));
        assert!(out.ends_with(&[0xFF, 0xDA, 0x00, 0x02, 0x12, 0x34, 0xFF, 0xD9]));
        assert!(strip_jpeg(b"\xFF\xD8\xFF\xE1\x00").is_none(), "truncated");
        assert!(
            strip_jpeg(b"\xFF\xD8\xFF\xDA\x00\x02\x12\x34").is_none(),
            "scan without end"
        );
        assert!(strip_jpeg(b"not a jpeg").is_none());
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(data);
        let mut covered = kind.to_vec();
        covered.extend_from_slice(data);
        v.extend_from_slice(&crc32(&covered).to_be_bytes());
        v
    }

    #[test]
    fn png_loses_text_and_exif_chunks() {
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        p.extend(chunk(b"IHDR", &[0; 13]));
        p.extend(chunk(b"tEXt", b"Author\0Hanako"));
        p.extend(chunk(b"eXIf", b"GPS"));
        p.extend(chunk(b"prVt", b"vendor secret"));
        p.extend(chunk(b"IDAT", b"pixels"));
        p.extend(chunk(b"IEND", b""));
        let out = strip_png(&p).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(!text.contains("Hanako") && !text.contains("GPS") && !text.contains("vendor"));
        assert!(text.contains("IHDR") && text.contains("pixels") && text.contains("IEND"));
        assert!(strip_png(&p[..p.len() - 12]).is_none(), "no IEND");
        let mut bad = p.clone();
        bad[30] ^= 1; // corrupt IHDR data → CRC mismatch
        assert!(strip_png(&bad).is_none());
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn webp_loses_exif_and_flags() {
        let mut body = b"WEBP".to_vec();
        let vp8x = {
            let mut c = b"VP8X".to_vec();
            c.extend_from_slice(&10u32.to_le_bytes());
            c.extend_from_slice(&[0x0C, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            c
        };
        body.extend(&vp8x);
        body.extend(b"EXIF");
        body.extend(3u32.to_le_bytes());
        body.extend(b"GPS\0");
        body.extend(b"VP8 ");
        body.extend(2u32.to_le_bytes());
        body.extend(b"px");
        let mut w = b"RIFF".to_vec();
        w.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
        w.extend(body);
        let out = strip_webp(&w).unwrap();
        assert!(!String::from_utf8_lossy(&out).contains("GPS"));
        assert_eq!(out[20] & 0x0C, 0, "flags cleared");
        assert_eq!(
            u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize,
            out.len() - 8
        );
    }
}
