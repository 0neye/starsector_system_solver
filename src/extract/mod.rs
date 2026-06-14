//! Save-game extraction layer. See workspace/SAVE_EXTRACTION_DESIGN.md for the full spec.

pub mod cli;
pub mod db;
pub mod gamedata;
pub mod locate;
pub mod mapping;
pub mod model;
pub mod save;
pub mod scan;
pub mod xml;

use std::fmt;

#[derive(Debug)]
pub enum ExtractError {
    Io(std::io::Error),
    Xml(String),
    Json(String),
    Db(String),
    BadSave(String),
    NotFound(String),
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtractError::Io(e) => write!(f, "IO error: {e}"),
            ExtractError::Xml(e) => write!(f, "XML error: {e}"),
            ExtractError::Json(e) => write!(f, "JSON error: {e}"),
            ExtractError::Db(e) => write!(f, "DB error: {e}"),
            ExtractError::BadSave(e) => write!(f, "Bad save: {e}"),
            ExtractError::NotFound(e) => write!(f, "Not found: {e}"),
        }
    }
}

impl std::error::Error for ExtractError {}

impl From<std::io::Error> for ExtractError {
    fn from(e: std::io::Error) -> Self {
        ExtractError::Io(e)
    }
}

impl From<quick_xml::Error> for ExtractError {
    fn from(e: quick_xml::Error) -> Self {
        ExtractError::Xml(e.to_string())
    }
}

impl From<serde_json::Error> for ExtractError {
    fn from(e: serde_json::Error) -> Self {
        ExtractError::Json(e.to_string())
    }
}

impl From<rusqlite::Error> for ExtractError {
    fn from(e: rusqlite::Error) -> Self {
        ExtractError::Db(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ExtractError>;
