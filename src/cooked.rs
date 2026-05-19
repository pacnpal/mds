use crate::{
    error::{Error, Result},
    mds::TrackMode,
};
use std::{
    cmp::min,
    io::{Read, Seek, SeekFrom},
    ops::Range,
};

/// Cooked logical sector size — the 2048 bytes of user data per ISO9660 block.
pub const COOKED_SECTOR_SIZE: usize = 0x800;

/// Compute the byte range within one raw .mdf sector that contains 2048 bytes
/// of ISO9660 user data, given the track's mode and raw data size (sector_size
/// minus any subchannel bytes).
///
/// This was previously private to `convert/iso.rs`; it's lifted here so the
/// CookedSectorReader can use the same single source of truth.
pub fn iso_user_data_range(mode: TrackMode, data_size: usize) -> Result<Range<usize>> {
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

/// Presents a 2048-byte-per-sector logical view over a reader positioned at
/// the start of a track's raw sectors. The underlying reader yields raw
/// sectors of `raw_sector_size` bytes (e.g. 2352, 2336, 2448); this adapter
/// reads one raw sector at a time, exposes only the `user_data` slice within
/// it, and presents a contiguous Read + Seek stream of logical 2048-byte
/// sectors.
///
/// The current raw sector is cached so small or cross-sector reads don't
/// re-issue I/O. Seeks within the current sector reuse the cache; seeks across
/// sectors invalidate it.
///
/// Logical offset 0 corresponds to LBA 0 of the data track (i.e. the byte at
/// `base_offset` in the underlying reader). This is what an ISO9660 reader
/// expects when computing `lba * 2048`.
pub struct CookedSectorReader<R> {
    inner: R,
    /// Absolute offset in `inner` where the track's first raw sector begins.
    base_offset: u64,
    /// Size of one raw sector in `inner` (e.g. 2352, 2336, 2448).
    raw_sector_size: usize,
    /// Byte range within each raw sector containing 2048 bytes of user data.
    user_data: Range<usize>,
    /// Number of logical (2048-byte) sectors available.
    num_sectors: u64,
    /// Logical byte position within the cooked stream.
    logical_pos: u64,
    /// Buffer for the most recently read raw sector, if any.
    cache: Vec<u8>,
    /// LBA currently held in `cache`, or u64::MAX if cache is empty.
    cached_lba: u64,
}

impl<R: Read + Seek> CookedSectorReader<R> {
    /// Construct a CookedSectorReader for one data track.
    ///
    /// * `inner`: the underlying reader (e.g. a BufReader on the .mdf).
    /// * `base_offset`: where the track's data starts in `inner`
    ///   (typically `track.track_start_offset`).
    /// * `raw_sector_size`: bytes per raw sector (`track.sector_size()`).
    /// * `user_data`: from `iso_user_data_range(track.mode, track.sector_data_size())`.
    /// * `num_sectors`: logical sectors available (`track.num_sectors()`).
    pub fn new(
        inner: R,
        base_offset: u64,
        raw_sector_size: usize,
        user_data: Range<usize>,
        num_sectors: u64,
    ) -> Self {
        debug_assert_eq!(user_data.end - user_data.start, COOKED_SECTOR_SIZE);
        Self {
            inner,
            base_offset,
            raw_sector_size,
            user_data,
            num_sectors,
            logical_pos: 0,
            cache: Vec::new(),
            cached_lba: u64::MAX,
        }
    }

    /// Total number of cooked bytes available (logical EOF).
    pub fn cooked_len(&self) -> u64 {
        self.num_sectors * COOKED_SECTOR_SIZE as u64
    }

    /// Ensure `cache` contains raw sector `lba`. No-op if already cached.
    fn load_sector(&mut self, lba: u64) -> std::io::Result<()> {
        if self.cached_lba == lba && !self.cache.is_empty() {
            return Ok(());
        }
        if self.cache.len() != self.raw_sector_size {
            self.cache.resize(self.raw_sector_size, 0);
        }
        let raw_offset = self.base_offset + lba * self.raw_sector_size as u64;
        self.inner.seek(SeekFrom::Start(raw_offset))?;
        self.inner.read_exact(&mut self.cache)?;
        self.cached_lba = lba;
        Ok(())
    }
}

impl<R: Read + Seek> Read for CookedSectorReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let total_cooked = self.cooked_len();
        if self.logical_pos >= total_cooked || buf.is_empty() {
            return Ok(0);
        }

        let lba = self.logical_pos / COOKED_SECTOR_SIZE as u64;
        let within = (self.logical_pos % COOKED_SECTOR_SIZE as u64) as usize;

        self.load_sector(lba)?;

        let user_start = self.user_data.start + within;
        let user_end = self.user_data.end;
        let available_in_sector = user_end - user_start;
        // Do the comparison in u64 so the remaining-bytes count doesn't
        // truncate on 32-bit pointer-width targets like wasm32-wasi for
        // images larger than 4 GB. The final value is bounded by
        // available_in_sector (<= 2048), so the usize cast is safe.
        let remaining_total = total_cooked - self.logical_pos;
        let n = min(
            buf.len() as u64,
            min(available_in_sector as u64, remaining_total),
        ) as usize;

        buf[..n].copy_from_slice(&self.cache[user_start..user_start + n]);
        self.logical_pos += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for CookedSectorReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let total = self.cooked_len() as i64;
        let new_pos: i64 = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(n) => total + n,
            SeekFrom::Current(n) => self.logical_pos as i64 + n,
        };
        if new_pos < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.logical_pos = new_pos as u64;
        Ok(self.logical_pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mds::TrackMode;
    use std::io::Cursor;

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

    /// Build a synthetic raw image: `pregap` bytes of 0xFF, then `n_sectors`
    /// raw sectors of `raw_size` bytes each. Each sector is laid out as
    /// `header_size` bytes of header (filled with 0xEE), then 2048 bytes of
    /// user data (filled with a per-sector pattern), then `trailer_size`
    /// trailer bytes (filled with 0xCC). The user-data pattern for sector
    /// `i` is `i as u8` repeated.
    fn synth_image(
        pregap: usize,
        n_sectors: usize,
        raw_size: usize,
        header_size: usize,
        trailer_size: usize,
    ) -> Vec<u8> {
        assert_eq!(header_size + COOKED_SECTOR_SIZE + trailer_size, raw_size);
        let mut buf = vec![0xFFu8; pregap];
        for i in 0..n_sectors {
            buf.extend(std::iter::repeat(0xEE).take(header_size));
            buf.extend(std::iter::repeat(i as u8).take(COOKED_SECTOR_SIZE));
            buf.extend(std::iter::repeat(0xCC).take(trailer_size));
        }
        buf
    }

    #[test]
    fn reads_user_data_sequentially_mode1_2352() {
        let pregap = 256;
        let n = 4;
        let raw = 0x930;
        let img = synth_image(pregap, n, raw, 16, 288);
        let reader =
            CookedSectorReader::new(Cursor::new(img), pregap as u64, raw, 16..16 + 0x800, n as u64);
        let mut all = Vec::new();
        let mut reader = reader;
        std::io::Read::read_to_end(&mut reader, &mut all).unwrap();
        assert_eq!(all.len(), n * COOKED_SECTOR_SIZE);
        for i in 0..n {
            let start = i * COOKED_SECTOR_SIZE;
            assert!(all[start..start + COOKED_SECTOR_SIZE]
                .iter()
                .all(|&b| b == i as u8));
        }
    }

    #[test]
    fn reads_user_data_sequentially_mode2_2336() {
        // XA Form 1, MODE2/2336: 8 subheader + 2048 user + 280 EDC/ECC = 2336.
        let n = 3;
        let raw = 0x920;
        let img = synth_image(0, n, raw, 8, 0x920 - 8 - COOKED_SECTOR_SIZE);
        let reader = CookedSectorReader::new(
            Cursor::new(img),
            0,
            raw,
            8..8 + 0x800,
            n as u64,
        );
        let mut all = Vec::new();
        let mut reader = reader;
        std::io::Read::read_to_end(&mut reader, &mut all).unwrap();
        for i in 0..n {
            let start = i * COOKED_SECTOR_SIZE;
            assert_eq!(all[start], i as u8);
            assert_eq!(all[start + COOKED_SECTOR_SIZE - 1], i as u8);
        }
    }

    #[test]
    fn handles_subchannel_trailers_2448() {
        // 2448 = 2352 + 96 subchannel bytes. For an iso-style Mode1 layout we
        // use header=16 + 2048 user + 288 EDC/ECC + 96 subchannel = 2448.
        let n = 2;
        let raw = 2448;
        let img = synth_image(0, n, raw, 16, 288 + 96);
        let reader =
            CookedSectorReader::new(Cursor::new(img), 0, raw, 16..16 + 0x800, n as u64);
        let mut all = Vec::new();
        let mut reader = reader;
        std::io::Read::read_to_end(&mut reader, &mut all).unwrap();
        assert_eq!(all.len(), n * COOKED_SECTOR_SIZE);
        assert!(all[0..COOKED_SECTOR_SIZE].iter().all(|&b| b == 0));
        assert!(all[COOKED_SECTOR_SIZE..].iter().all(|&b| b == 1));
    }

    #[test]
    fn cross_sector_read_returns_partial_then_continues() {
        // A single Read call should return at most one sector's worth of
        // data so the std::io::Read contract is honored; the caller (or
        // read_to_end) is responsible for re-issuing.
        let n = 2;
        let raw = 0x930;
        let img = synth_image(0, n, raw, 16, 288);
        let mut reader =
            CookedSectorReader::new(Cursor::new(img), 0, raw, 16..16 + 0x800, n as u64);
        // Seek to 100 bytes before the second sector boundary.
        std::io::Seek::seek(&mut reader, SeekFrom::Start(COOKED_SECTOR_SIZE as u64 - 100))
            .unwrap();
        let mut buf = [0u8; 200];
        let n1 = std::io::Read::read(&mut reader, &mut buf).unwrap();
        assert_eq!(n1, 100, "should stop at the sector boundary");
        assert!(buf[..100].iter().all(|&b| b == 0));
        let n2 = std::io::Read::read(&mut reader, &mut buf[100..]).unwrap();
        assert_eq!(n2, 100);
        assert!(buf[100..200].iter().all(|&b| b == 1));
    }

    #[test]
    fn seek_within_sector_uses_cache() {
        // Wrap the cursor in something that counts seeks so we can confirm
        // the cache prevents re-reading the same raw sector.
        struct Counting<R> {
            inner: R,
            seeks: usize,
        }
        impl<R: Read> Read for Counting<R> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.inner.read(buf)
            }
        }
        impl<R: Seek> Seek for Counting<R> {
            fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
                self.seeks += 1;
                self.inner.seek(pos)
            }
        }

        let raw = 0x930;
        let img = synth_image(0, 1, raw, 16, 288);
        let counting = Counting {
            inner: Cursor::new(img),
            seeks: 0,
        };
        let mut reader =
            CookedSectorReader::new(counting, 0, raw, 16..16 + 0x800, 1);

        // First read loads the sector — that's one underlying seek.
        let mut buf = [0u8; 10];
        std::io::Read::read_exact(&mut reader, &mut buf).unwrap();

        let after_first_seeks = reader.inner.seeks;
        // Seek to a different offset within the same logical sector.
        std::io::Seek::seek(&mut reader, SeekFrom::Start(1000)).unwrap();
        std::io::Read::read_exact(&mut reader, &mut buf).unwrap();
        // No new underlying seek should have happened.
        assert_eq!(reader.inner.seeks, after_first_seeks);
    }

    #[test]
    fn read_at_eof_returns_zero() {
        let raw = 0x930;
        let img = synth_image(0, 1, raw, 16, 288);
        let mut reader =
            CookedSectorReader::new(Cursor::new(img), 0, raw, 16..16 + 0x800, 1);
        std::io::Seek::seek(&mut reader, SeekFrom::End(0)).unwrap();
        let mut buf = [0u8; 16];
        assert_eq!(std::io::Read::read(&mut reader, &mut buf).unwrap(), 0);
    }
}
