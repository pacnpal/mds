use crate::{
    error::{Error, Result},
    loader::load_mds,
    mds::{Track, TrackMode},
    util::{reader_for_track, writer_with_extension},
};
use std::{
    io::{Read, Write},
    path::Path,
};

/// Convert a .mdf file (described by a .mds file) into a .iso file. This is not always possible,
/// because an ISO represents the bytes of one track. If the .mds refers to a multi-track disc,
/// writing to an ISO doesn't make sense. Try using BIN/CUE in that case.
pub fn convert<P: AsRef<Path>>(mds_file: P) -> Result<()> {
    let mds = load_mds(&mds_file)?;
    let track = mds.single_track()?;
    let writer = writer_with_extension(&mds_file, "iso")?;

    track_to_iso(&track, &mds_file, writer)
}

fn track_to_iso<P: AsRef<Path>, W: Write>(track: &Track, mds_path: P, mut writer: W) -> Result<()> {
    let sector_size = track.sector_size();
    let (data_offset, data_size) = iso_data_layout(track.mode, track.sector_data_size())?;
    let num_sectors = track.num_sectors();
    let mut reader = reader_for_track(&mds_path, track)?;

    let mut buf = vec![0; sector_size];
    for _ in 0..num_sectors {
        reader.read_exact(&mut buf).map_err(Error::Io)?;

        // In order to convert the .mdf, take only the main track's data from each sector. Each
        // sector may also contain subchannel data which is stored at the end of the sector. ISO
        // files don't store subchannel data, so just discard this.
        writer
            .write_all(&buf[data_offset..(data_offset + data_size)])
            .map_err(Error::Io)?;
    }

    Ok(())
}

fn iso_data_layout(mode: TrackMode, sector_data_size: usize) -> Result<(usize, usize)> {
    use TrackMode::*;

    match (mode, sector_data_size) {
        // Already cooked sectors
        (Mode1, 0x800) | (Mode2, 0x800) | (Mode2Form1, 0x800) => Ok((0, 0x800)),
        // Raw sectors; skip sync/header bytes
        (Mode1, 0x930) => Ok((0x10, 0x800)),
        // Raw MODE2 sectors have an extra 8-byte subheader vs MODE1
        (Mode2, 0x930) | (Mode2Form1, 0x930) => Ok((0x18, 0x800)),
        _ => Err(Error::UnknownIsoTrackSize(mode, sector_data_size)),
    }
}

#[cfg(test)]
mod test {
    use super::iso_data_layout;
    use crate::{error::Error, mds::TrackMode};

    #[test]
    fn raw_mode1_2352_uses_16_byte_offset() {
        let (offset, len) = iso_data_layout(TrackMode::Mode1, 0x930).unwrap();
        assert_eq!(offset, 0x10);
        assert_eq!(len, 0x800);
    }

    #[test]
    fn raw_mode2_2352_uses_24_byte_offset() {
        let (offset, len) = iso_data_layout(TrackMode::Mode2, 0x930).unwrap();
        assert_eq!(offset, 0x18);
        assert_eq!(len, 0x800);
    }

    #[test]
    fn unsupported_iso_layout_returns_error() {
        let err = iso_data_layout(TrackMode::Mode2Form2, 0x914).unwrap_err();

        match err {
            Error::UnknownIsoTrackSize(TrackMode::Mode2Form2, 0x914) => (),
            _ => panic!("unexpected error variant"),
        }
    }
}
