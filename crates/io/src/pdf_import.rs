//! PDF import: one page rasterised at a chosen resolution onto white, as File › Open does for a
//! PDF in Photoshop. Rendering is `hayro`, a pure-Rust PDF rasteriser (native builds only: the
//! browser build keeps its size budget). It does not open password-protected files; blending
//! isolation and knockout groups are approximated by the renderer. Every call into it runs under
//! `catch_unwind`, so a renderer panic on a hostile file is an error.
//!
//! Vector content is rasterised: this is not Photoshop's "Import PDF" with editable text, and a
//! multi-page file opens one page per call.

use photocraft_codecs::{ChannelLayout, Image};

use crate::{ImportResult, IoError};

/// Resolution a PDF opens at when none is given (pixels per inch).
pub const DEFAULT_DPI: f32 = 150.0;
/// Pixels on the longest side a page may be rendered to (the renderer's own limit is 65535).
const MAX_SIDE: u32 = 30000;

pub fn is_pdf(bytes: &[u8]) -> bool {
    // The header may follow up to 1 KiB of junk (the specification allows it).
    bytes.windows(5).take(1024).any(|w| w == b"%PDF-")
}

/// Page count and each page's size in points (1/72 inch).
#[derive(Debug, Clone, PartialEq)]
pub struct PdfInfo {
    pub pages: Vec<(f32, f32)>,
}

#[cfg(target_arch = "wasm32")]
pub fn info(_bytes: &[u8]) -> Result<PdfInfo, IoError> {
    Err(IoError::Unsupported("PDF can't be opened in the browser version of PhotoCraft; open it in the desktop app".into()))
}

#[cfg(target_arch = "wasm32")]
pub fn import(_name: &str, _bytes: &[u8], _page: usize, _dpi: f32) -> Result<ImportResult, IoError> {
    Err(IoError::Unsupported("PDF can't be opened in the browser version of PhotoCraft; open it in the desktop app".into()))
}

#[cfg(not(target_arch = "wasm32"))]
fn guarded<T>(f: impl FnOnce() -> Result<T, IoError>) -> Result<T, IoError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| Err(IoError::Unsupported("the PDF renderer failed on this file".into())))
}

#[cfg(not(target_arch = "wasm32"))]
fn open(bytes: &[u8]) -> Result<hayro::hayro_syntax::Pdf, IoError> {
    // Junk before the header shifts every offset: hand the renderer the file from the header on.
    let start = bytes.windows(5).take(1024).position(|w| w == b"%PDF-").unwrap_or(0);
    hayro::hayro_syntax::Pdf::new(bytes[start..].to_vec()).map_err(|e| IoError::Unsupported(format!("not a readable PDF: {e:?}")))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn info(bytes: &[u8]) -> Result<PdfInfo, IoError> {
    guarded(|| {
        let pdf = open(bytes)?;
        let pages: Vec<(f32, f32)> = pdf.pages().iter().map(|p| p.render_dimensions()).collect();
        if pages.is_empty() {
            return Err(IoError::Unsupported("the PDF has no pages".into()));
        }
        Ok(PdfInfo { pages })
    })
}

/// Page `page` (0-based) at `dpi` pixels per inch, on a white background, as an RGB document.
#[cfg(not(target_arch = "wasm32"))]
pub fn import(name: &str, bytes: &[u8], page: usize, dpi: f32) -> Result<ImportResult, IoError> {
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
    if !(1.0..=2400.0).contains(&dpi) {
        return Err(IoError::Unsupported("the resolution must be between 1 and 2400 pixels per inch".into()));
    }
    guarded(|| {
        let pdf = open(bytes)?;
        let pages = pdf.pages();
        let p = pages.get(page).ok_or_else(|| IoError::Unsupported(format!("the PDF has {} page(s); page {} does not exist", pages.len(), page + 1)))?;
        let (wpt, hpt) = p.render_dimensions();
        let scale = dpi / 72.0;
        let (w, h) = ((wpt * scale).round().max(1.0), (hpt * scale).round().max(1.0));
        if w > MAX_SIDE as f32 || h > MAX_SIDE as f32 {
            return Err(IoError::Unsupported(format!("{w} x {h} pixels is too large: lower the resolution")));
        }
        // The same size budget as every other decoder (the page is drawn to memory first).
        photocraft_codecs::Limits::default().check(w as u32, h as u32, ChannelLayout::Rgba, photocraft_codecs::SampleType::U8).map_err(|e| IoError::Unsupported(format!("{w} x {h} pixels is too large: lower the resolution ({e})")))?;
        // The renderer sizes its bitmap from the scale; pass the exact one that gives w x h.
        let pixmap = render(
            p,
            &RenderCache::new(),
            &hayro::hayro_interpret::InterpreterSettings::default(),
            &RenderSettings::default(),
            &PixmapSettings { x_scale: w / wpt, y_scale: h / hpt, bg_color: WHITE },
        );
        let (pw, ph) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
        let rgb: Vec<u8> = pixmap.data_as_u8_slice().as_chunks::<4>().0.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
        let img = Image::from_u8(pw, ph, ChannelLayout::Rgb, rgb).map_err(IoError::from)?;
        let mut r = crate::flat::image_to_document(name, &img)?;
        r.document.resolution_dpi = dpi;
        if pages.len() > 1 {
            r.warnings.push(format!("Opened page {} of {}", page + 1, pages.len()));
        }
        Ok(r)
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    const PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 10] >>\nendobj\ntrailer\n<< /Size 4 /Root 1 0 R >>\n%%EOF\n";

    #[test]
    fn damaged_and_hostile_pdfs_are_errors_not_crashes() {
        assert!(is_pdf(PDF));
        assert!(!is_pdf(b"not a pdf"));
        // Whatever the renderer makes of a damaged file, it comes back as a result.
        for cut in 0..PDF.len() {
            let _ = import("x.pdf", &PDF[..cut], 0, 72.0);
        }
        for i in 0..PDF.len() {
            let mut b = PDF.to_vec();
            b[i] ^= 0x55;
            let _ = info(&b);
        }
        // A page of 1,000,000 points at 2400 ppi is refused up front.
        let huge = String::from_utf8_lossy(PDF).replace("[0 0 20 10]", "[0 0 1000000 1000000]");
        assert!(import("x.pdf", huge.as_bytes(), 0, 2400.0).is_err());
        assert!(import("x.pdf", PDF, 0, 0.0).is_err(), "resolution out of range");
        assert!(import("x.pdf", PDF, 3, 72.0).is_err(), "no such page");
        assert_eq!(import("x.pdf", PDF, 0, 72.0).unwrap().document.size.width, 20);
    }
}
