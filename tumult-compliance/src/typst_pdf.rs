//! Typst compilation: embedded OFL fonts, an in-memory [`World`], and
//! `ReportDoc` → Typst markup → PDF bytes.

use std::collections::HashMap;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};

use crate::model::ReportDoc;

/// Errors from the docs pipeline.
#[derive(Debug, thiserror::Error)]
pub enum DocsError {
    #[error("typst compile failed: {0}")]
    Compile(String),
    #[error("pdf export failed: {0}")]
    Pdf(String),
}

/// Vendored OFL fonts (Inter + Source Serif 4, variable TTFs), embedded
/// into the binary so docker builds stay offline-reproducible.
static FONT_FILES: &[&[u8]] = &[
    include_bytes!("../../assets/fonts/Inter-var.ttf"),
    include_bytes!("../../assets/fonts/Inter-Italic-var.ttf"),
    include_bytes!("../../assets/fonts/SourceSerif4-var.ttf"),
    include_bytes!("../../assets/fonts/SourceSerif4-Italic-var.ttf"),
];

/// Loaded font faces + book, built once per compile (cheap enough: four
/// faces parsed from embedded bytes).
struct Fonts {
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}

fn load_fonts() -> Fonts {
    let mut book = FontBook::new();
    let mut fonts = Vec::new();
    for bytes in FONT_FILES {
        let bytes = Bytes::new(*bytes);
        // A TTC would have several faces; our files are single-face TTFs,
        // but probe the first few indices anyway.
        for index in 0..4 {
            let Some(font) = Font::new(bytes.clone(), index) else {
                break;
            };
            book.push(font.info().clone());
            fonts.push(font);
        }
    }
    if fonts.is_empty() {
        tracing::error!("no vendored fonts loaded; PDF text will be missing");
    }
    Fonts {
        book: LazyHash::new(book),
        fonts,
    }
}

/// In-memory Typst world: one main source document plus virtual files for
/// chart SVGs (`/charts/cN.svg`).
struct DocWorld {
    library: LazyHash<Library>,
    fonts: Fonts,
    main: Source,
    files: HashMap<VirtualPath, Bytes>,
}

impl World for DocWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.fonts.book
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.fonts.get(index).cloned()
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            return Ok(self.main.clone());
        }
        Err(FileError::NotFound(id.vpath().get_with_slash().into()))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files
            .get(id.vpath())
            .cloned()
            .ok_or_else(|| FileError::NotFound(id.vpath().get_with_slash().into()))
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        // Dates are baked into the markup as strings; no live clock needed.
        None
    }
}

/// Compile Typst `markup` (with `files` serving virtual paths) to PDF bytes.
///
/// # Errors
/// Returns [`DocsError::Compile`] on Typst diagnostics, [`DocsError::Pdf`]
/// on export failure.
pub fn compile_markup(
    markup: &str,
    files: HashMap<VirtualPath, Bytes>,
) -> Result<Vec<u8>, DocsError> {
    let world = DocWorld {
        library: LazyHash::new(Library::default()),
        fonts: load_fonts(),
        main: Source::detached(markup),
        files,
    };
    let warned = typst::compile(&world);
    for warning in &warned.warnings {
        tracing::debug!(message = %warning.message, "typst warning");
    }
    let document = warned
        .output
        .map_err(|errors| DocsError::Compile(format_diagnostics(&errors)))?;
    let pdf = typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default())
        .map_err(|errors| DocsError::Pdf(format_diagnostics(&errors)))?;
    Ok(pdf)
}

fn format_diagnostics(errors: &typst::diag::EcoVec<typst::diag::SourceDiagnostic>) -> String {
    errors
        .iter()
        .map(|e| format!("{} ({:?})", e.message, e.span))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Render a [`ReportDoc`] to PDF bytes via the Typst pipeline.
///
/// # Errors
/// See [`compile_markup`].
pub fn render_pdf(doc: &ReportDoc) -> Result<Vec<u8>, DocsError> {
    let (markup, files) = crate::markup::doc_to_typst(doc);
    compile_markup(&markup, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts_load() {
        let fonts = load_fonts();
        assert!(fonts.fonts.len() >= 4, "got {}", fonts.fonts.len());
    }

    #[test]
    fn minimal_document_compiles_to_pdf() {
        let pdf = compile_markup(
            "#set page(paper: \"a4\")\n#set text(font: \"Inter\")\nHello, kronika.",
            HashMap::new(),
        )
        .expect("compile");
        assert!(pdf.starts_with(b"%PDF"), "missing magic bytes");
        assert!(pdf.len() > 2_000, "suspiciously small: {}", pdf.len());
    }
}
