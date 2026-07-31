//! Getting a finished figure out of lilook.
//!
//! The point of the whole tool is a figure that ends up in a paper, and until
//! this existed the only ways out were a screenshot and the clipboard. No
//! journal takes a raster figure, so **PDF and SVG are the formats that matter**;
//! PNG is here for slides, issue threads and eyeballing a result.
//!
//! All three come from the *same compiled document* the canvas is already
//! showing -- `Backend::document()` -- so an export is never a second, subtly
//! different compile. That also means exporting costs nothing but the encode.
//!
//! The PNG encoder is forty lines rather than a dependency. It writes stored
//! (uncompressed) deflate blocks: larger on disk than a compressed PNG and
//! readable by everything, which is the right trade for a file written once and
//! opened once. PDF and SVG go through typst's own exporters, which is the whole
//! argument for building on typst in the first place.

use typst_layout::PagedDocument;

/// What a figure can be exported as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Vector, one file, embeds its fonts. What a journal wants.
    Pdf,
    /// Vector, editable in Illustrator or Inkscape. What a co-author wants when
    /// they need to nudge a label.
    Svg,
    /// Raster, at a chosen resolution. Slides and screenshots.
    Png,
}

impl Format {
    pub fn of_path(path: &str) -> Option<Format> {
        match path.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
            "pdf" => Some(Format::Pdf),
            "svg" => Some(Format::Svg),
            "png" => Some(Format::Png),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Svg => "svg",
            Format::Png => "png",
        }
    }

    /// Every format, in the order a chooser should list them: the one a paper
    /// needs first.
    pub const ALL: [Format; 3] = [Format::Pdf, Format::Svg, Format::Png];
}

/// Render `doc` to bytes.
///
/// `ppi` is used by `Png` only; the vector formats carry no resolution, which is
/// the reason to prefer them.
pub fn export(doc: &PagedDocument, format: Format, ppi: f32) -> Result<Vec<u8>, String> {
    match format {
        Format::Pdf => {
            let options = typst_pdf::PdfOptions::default();
            typst_pdf::pdf(doc, &options).map_err(|e| {
                e.first()
                    .map(|d| d.message.to_string())
                    .unwrap_or_else(|| "PDF export failed".into())
            })
        }
        Format::Svg => {
            // One page: a lilaq figure is one page by construction, and
            // `svg_merged` would add a gap between pages that is not part of the
            // figure. Falling back to the merged form keeps a multi-page document
            // exportable rather than erroring.
            let options = typst_svg::SvgOptions::default();
            match doc.pages() {
                [page] => Ok(typst_svg::svg(page, &options).into_bytes()),
                _ => Ok(
                    typst_svg::svg_merged(doc, &options, typst::layout::Abs::zero()).into_bytes(),
                ),
            }
        }
        Format::Png => {
            let page = doc.pages().first().ok_or("nothing to export")?;
            let size = page.frame.size();
            let ppp = ppi / 72.0;
            let pixels = (size.x.to_pt() * ppp as f64) * (size.y.to_pt() * ppp as f64);
            // The same guard `rasterize` uses: `typst_render` unwraps its pixmap
            // allocation, so an implausible size panics inside the renderer with
            // no way to catch it.
            if !pixels.is_finite() || pixels > crate::backend::MAX_RASTER_PIXELS {
                return Err(format!(
                    "{ppi} ppi would be {:.0} megapixels; try fewer",
                    pixels / 1e6
                ));
            }
            let pixmap = typst_render::render(
                page,
                &typst_render::RenderOptions {
                    pixel_per_pt: typst::utils::Scalar::new(ppp as f64),
                    render_bleed: false,
                },
            );
            Ok(png(pixmap.width(), pixmap.height(), &pixmap.take()))
        }
    }
}

/// A PNG, 8-bit RGBA, with stored deflate blocks.
pub fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc(bytes: &[u8]) -> u32 {
        let mut c: u32 = 0xffff_ffff;
        for &b in bytes {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = tag.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }

    let mut raw = Vec::with_capacity((w * h * 4 + h) as usize);
    for y in 0..h {
        raw.push(0); // filter: none
        let row = (y * w * 4) as usize;
        raw.extend_from_slice(&rgba[row..row + (w * 4) as usize]);
    }

    let mut z = vec![0x78, 0x01];
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    let blocks = raw.chunks(65535).count().max(1);
    for (i, part) in raw.chunks(65535).enumerate() {
        z.push(u8::from(i + 1 == blocks));
        z.extend_from_slice(&(part.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(part.len() as u16)).to_le_bytes());
        z.extend_from_slice(part);
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}
