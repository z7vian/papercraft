use std::io::Cursor;

use office_oxide::DocumentFormat;

use super::{OfficeFamily, bad};
use crate::CoreError;

/// The old binary formats (Office 97-2003, and WPS which shares them) as the matching OOXML package.
// Limit: the conversion keeps text and simple layout only (images move, sheet styles drop); a faithful converter lifts it.
pub(super) fn to_ooxml(bytes: Vec<u8>, family: OfficeFamily) -> Result<Vec<u8>, CoreError> {
    let (from, to) = match family {
        OfficeFamily::Document => (DocumentFormat::Doc, DocumentFormat::Docx),
        OfficeFamily::Presentation => (DocumentFormat::Ppt, DocumentFormat::Pptx),
        OfficeFamily::Spreadsheet => (DocumentFormat::Xls, DocumentFormat::Xlsx),
    };
    let doc = office_oxide::Document::from_reader(Cursor::new(bytes), from).map_err(|e| bad("file", e))?;
    let mut out = Cursor::new(Vec::new());
    office_oxide::create::create_from_ir_to_writer(&doc.to_ir(), to, &mut out).map_err(|e| bad("file", e))?;
    Ok(out.into_inner())
}
