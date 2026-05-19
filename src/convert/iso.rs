use crate::{
    error::{Error, Result},
    loader::load_mds,
    mds::{Track, TrackMode},
    util::{reader_for_track, writer_with_extension},
};
use std::{
    io::{Read, Seek, SeekFrom, Write},
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
    let sector_data = iso_user_data_range(track.mode, track.sector_data_size())?;
    // Hoist start/end out of the per-sector loop so we don't clone the Range
    // hundreds of thousands of times on large discs.
    let (data_start, data_end) = (sector_data.start, sector_data.end);
    let num_sectors = track.num_sectors();
    let mut reader = reader_for_track(&mds_path, track)?;
    // Seek to the track's start within the .mdf so pregap/lead-in data is
    // skipped — otherwise images with a non-zero start offset would read
    // misaligned sectors. Mirrors mds_to_bin's behaviour.
    reader
        .seek(SeekFrom::Start(track.track_start_offset))
        .map_err(Error::Io)?;

    let mut buf = vec![0; sector_size];
    for _ in 0..num_sectors {
        reader.read_exact(&mut buf).map_err(Error::Io)?;

        writer
            .write_all(&buf[data_start..data_end])
            .map_err(Error::Io)?;
    }

    Ok(())
}

fn iso_user_data_range(mode: TrackMode, data_size: usize) -> Result<std::ops::Range<usize>> {
    use Error::UnknownIsoTrackSize;
    use TrackMode::*;

    match (mode, data_size) {
        (Mode1, 0x800) | (Mode2Form1, 0x800) => Ok(0..0x800),
        (Mode1, 0x930) => Ok(16..16 + 0x800),
        (Mode2, 0x930) | (Mode2Form1, 0x930) => Ok(24..24 + 0x800),
        // MODE2/2336 (XA Form 1): no 16-byte sync+header prefix, but the 8-byte
        // XA subheader still precedes the 2048-byte user data region.
        (Mode2, 0x920) | (Mode2Form1, 0x920) => Ok(8..8 + 0x800),
        (mode, data_size) => Err(UnknownIsoTrackSize(mode, data_size)),
    }
}

#[cfg(test)]
mod tests {
    use super::iso_user_data_range;
    use crate::mds::TrackMode;

    #[test]
    fn mode1_2352_extracts_2048_user_data() {
        assert_eq!(
            iso_user_data_range(TrackMode::Mode1, 0x930).unwrap(),
            16..16 + 0x800
        );
    }

    #[test]
    fn mode2_2352_extracts_2048_user_data() {
        assert_eq!(
            iso_user_data_range(TrackMode::Mode2, 0x930).unwrap(),
            24..24 + 0x800
        );
    }

    #[test]
    fn mode2_2336_extracts_2048_user_data() {
        assert_eq!(
            iso_user_data_range(TrackMode::Mode2, 0x920).unwrap(),
            8..8 + 0x800
        );
    }

    #[test]
    fn mode2form1_2336_extracts_2048_user_data() {
        assert_eq!(
            iso_user_data_range(TrackMode::Mode2Form1, 0x920).unwrap(),
            8..8 + 0x800
        );
    }
}
