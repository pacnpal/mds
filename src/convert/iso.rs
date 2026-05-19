use crate::{
    cooked::iso_user_data_range,
    error::{Error, Result},
    loader::load_mds,
    mds::Track,
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

