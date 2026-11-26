uniffi::setup_scaffolding!();

mod mhtml;
mod table;
mod text;
mod tiff;

/// Any file we can't read. Kotlin sees this as `CoreException.BadFile`.
#[derive(Debug, uniffi::Error)]
#[uniffi(flat_error)]
pub enum CoreError {
    BadFile(String),
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let CoreError::BadFile(reason) = self;
        f.write_str(reason)
    }
}

impl std::error::Error for CoreError {}
