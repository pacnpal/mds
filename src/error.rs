use std::{fmt::Display, io, path::PathBuf};

use crate::mds::TrackMode;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Iso9660(String),
    MissingInputFile,
    MultiTrackNotSupported,
    NoDataTracks,
    NoSessions,
    OutputExists(PathBuf),
    ParseError,
    PathEscape(String),
    TooManySessions,
    UnknownCueTrackSize(TrackMode, usize),
    UnknownIsoTrackSize(TrackMode, usize),
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use Error::*;

        match self {
            Io(err) => write!(f, "{err}"),
            Iso9660(msg) => write!(f, "ISO9660: {msg}"),
            MissingInputFile => write!(f, "No input file provided to read data from"),
            MultiTrackNotSupported => write!(f, "Multi-track discs are not yet supported"),
            NoDataTracks => write!(f, "There are no data tracks in this mdf"),
            NoSessions => write!(f, "There are no sessions in the image"),
            OutputExists(path) => write!(
                f,
                "Output directory '{}' is not empty (pass --force to overwrite)",
                path.display()
            ),
            ParseError => write!(f, "Error parsing mds file"),
            PathEscape(name) => write!(
                f,
                "Refusing to write file with unsafe path component: '{name}'"
            ),
            TooManySessions => write!(f, "Cannot convert multi-session images"),
            UnknownCueTrackSize(mode, data_size) => {
                write!(f, "Unusual track type: {mode:?} @ {data_size}")
            }
            UnknownIsoTrackSize(mode, data_size) => {
                write!(f, "Cannot convert track type to ISO: {mode:?} @ {data_size}")
            }
        }
    }
}

impl std::error::Error for Error {}
