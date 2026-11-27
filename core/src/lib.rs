uniffi::setup_scaffolding!();

mod formats;
mod mhtml;
mod office;
mod pdf;
mod sheet;
mod table;
mod text;
mod tiff;

/// Any file we can't read. Kotlin sees this as `CoreException.BadFile`, `.PasswordRequired` or `.WrongPassword`.
#[derive(Debug, uniffi::Error)]
#[uniffi(flat_error)]
pub enum CoreError {
    BadFile(String),
    /// The file is locked and no password was given.
    PasswordRequired,
    /// The given password doesn't unlock the file.
    WrongPassword,
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CoreError::BadFile(reason) => reason,
            CoreError::PasswordRequired => "This file is password-protected.",
            CoreError::WrongPassword => "That password doesn't open this file.",
        })
    }
}

impl std::error::Error for CoreError {}
