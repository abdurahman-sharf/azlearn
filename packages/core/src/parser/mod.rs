mod txt;
mod docx;
mod arabic;
mod pdf;
mod table;
mod csv;
mod excel;
mod epub;
mod html;
mod xml_text;
mod pptx;
mod odt;

use std::path::Path;

pub use txt::extract_txt;
pub use docx::extract_docx;
pub use pdf::extract_pdf;
pub use arabic::{fix_visual_order, has_arabic};
pub use csv::extract_csv;
pub use excel::extract_excel;
pub use epub::extract_epub;
pub use html::extract_html;
pub use pptx::extract_pptx;
pub use odt::extract_odt;

#[derive(Debug)]
pub enum FileFormat {
    PlainText,
    Docx,
    Pdf,
    Odt,
    Pptx,
    Csv,
    Excel,
    Html,
    Epub,
}

impl FileFormat {
    pub fn from_extension(path: &str) -> Result<Self, ParserError> {
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        match ext.as_str() {
            "docx" => Ok(FileFormat::Docx),
            "pdf" => Ok(FileFormat::Pdf),
            "csv" => Ok(FileFormat::Csv),
            "xlsx" | "xlsm" | "xls" | "ods" => Ok(FileFormat::Excel),
            "html" | "htm" => Ok(FileFormat::Html),
            "pptx" => Ok(FileFormat::Pptx),
            "odt" => Ok(FileFormat::Odt),
            "epub" => Ok(FileFormat::Epub),
            _ => Ok(FileFormat::PlainText),
        }
    }
}

#[derive(Debug)]
pub enum ParserError {
    Io(std::io::Error),
    Parse(String),
    Unsupported(String),
}

impl std::fmt::Display for ParserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParserError::Io(e) => write!(f, "IO error: {e}"),
            ParserError::Parse(e) => write!(f, "Parse error: {e}"),
            ParserError::Unsupported(e) => write!(f, "Unsupported: {e}"),
        }
    }
}

impl From<std::io::Error> for ParserError {
    fn from(e: std::io::Error) -> Self { ParserError::Io(e) }
}

pub fn parse_file(path: &str) -> Result<String, ParserError> {
    let format = FileFormat::from_extension(path)?;
    match format {
        FileFormat::PlainText => extract_txt(path),
        FileFormat::Docx => extract_docx(path),
        FileFormat::Pdf => extract_pdf(path),
        FileFormat::Pptx => extract_pptx(path),
        FileFormat::Odt => extract_odt(path),
        FileFormat::Csv => extract_csv(path),
        FileFormat::Excel => extract_excel(path),
        FileFormat::Html => extract_html(path),
        FileFormat::Epub => extract_epub(path),
    }
}
