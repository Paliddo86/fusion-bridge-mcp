//! A minimal PDF writer: one page per PNG, the image filling the page at its own DPI. The PNG is
//! flattened onto white and re-encoded as 8-bit RGB, and its IDAT stream goes into the PDF as is
//! (FlateDecode with the PNG predictor), so no other image or compression crate is needed.

fn rgb_png(path: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut dec = png::Decoder::new(f);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut rd = dec.read_info().map_err(|e| format!("{path}: {e}"))?;
    let mut buf = vec![0; rd.output_buffer_size()];
    let info = rd.next_frame(&mut buf).map_err(|e| format!("{path}: {e}"))?;
    let (w, h) = (info.width, info.height);
    let px = &buf[..info.buffer_size()];
    let rgb: Vec<u8> = match info.color_type {
        png::ColorType::Rgb => px.to_vec(),
        png::ColorType::Rgba => px.chunks(4).flat_map(|p| { let a = p[3] as u32; (0..3).map(move |k| ((p[k] as u32 * a + 255 * (255 - a)) / 255) as u8) }).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|g| [*g, *g, *g]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks(2).flat_map(|p| { let a = p[1] as u32; let v = ((p[0] as u32 * a + 255 * (255 - a)) / 255) as u8; [v, v, v] }).collect(),
        other => return Err(format!("{path}: unsupported colour type {other:?}")),
    };
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(&rgb).map_err(|e| e.to_string())?;
    }
    // the IDAT chunks, concatenated, are one zlib stream of filtered rows
    let mut idat = Vec::new();
    let mut i = 8;
    while i + 8 <= out.len() {
        let len = u32::from_be_bytes([out[i], out[i + 1], out[i + 2], out[i + 3]]) as usize;
        if &out[i + 4..i + 8] == b"IDAT" { idat.extend_from_slice(&out[i + 8..i + 8 + len]); }
        i += 12 + len;
    }
    Ok((w, h, idat))
}

/// Write `pages` (PNG paths with their DPI) as one PDF at `out`.
pub fn images_to_pdf(pages: &[(String, f64)], out: &str) -> Result<usize, String> {
    let mut body: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let n = pages.len();
    // objects: 1 catalog, 2 pages, then per page: page, image, content
    let mut objs: Vec<Vec<u8>> = vec![Vec::new(), Vec::new()];
    let kids: Vec<String> = (0..n).map(|k| format!("{} 0 R", 3 + 3 * k)).collect();
    objs[0] = b"<< /Type /Catalog /Pages 2 0 R >>".to_vec();
    objs[1] = format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), n).into_bytes();
    for (k, (path, dpi)) in pages.iter().enumerate() {
        let (w, h, idat) = rgb_png(path)?;
        let (pw, ph) = (w as f64 * 72.0 / dpi.max(1.0), h as f64 * 72.0 / dpi.max(1.0));
        let (img, content) = (4 + 3 * k, 5 + 3 * k);
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] /Resources << /XObject << /Im{k} {img} 0 R >> >> /Contents {content} 0 R >>").into_bytes());
        let mut im = format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /DecodeParms << /Predictor 15 /Colors 3 /BitsPerComponent 8 /Columns {w} >> /Length {} >>\nstream\n", idat.len()).into_bytes();
        im.extend_from_slice(&idat);
        im.extend_from_slice(b"\nendstream");
        objs.push(im);
        let draw = format!("q {pw:.2} 0 0 {ph:.2} 0 0 cm /Im{k} Do Q");
        objs.push(format!("<< /Length {} >>\nstream\n{draw}\nendstream", draw.len()).into_bytes());
    }
    for (k, o) in objs.iter().enumerate() {
        offsets.push(body.len());
        body.extend_from_slice(format!("{} 0 obj\n", k + 1).as_bytes());
        body.extend_from_slice(o);
        body.extend_from_slice(b"\nendobj\n");
    }
    let xref = body.len();
    body.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in &offsets { body.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes()); }
    body.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    std::fs::write(out, &body).map_err(|e| format!("{out}: {e}"))?;
    Ok(n)
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn writes_a_pdf_page_per_png() {
        let dir = std::env::temp_dir().join("adom-pdf-test");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("a.png");
        {
            let f = std::fs::File::create(&p).unwrap();
            let mut e = png::Encoder::new(f, 4, 2);
            e.set_color(png::ColorType::Rgba); e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(&[255u8; 4 * 2 * 4]).unwrap();
        }
        let out = dir.join("a.pdf");
        let pages = vec![(p.to_string_lossy().to_string(), 72.0), (p.to_string_lossy().to_string(), 144.0)];
        assert_eq!(2, images_to_pdf(&pages, &out.to_string_lossy()).unwrap());
        let b = std::fs::read(&out).unwrap();
        assert!(b.starts_with(b"%PDF-1.4") && b.ends_with(b"%%EOF\n"));
        let t = String::from_utf8_lossy(&b);
        assert!(t.contains("/Count 2") && t.contains("/MediaBox [0 0 4.00 2.00]") && t.contains("/MediaBox [0 0 2.00 1.00]"));
    }
}
