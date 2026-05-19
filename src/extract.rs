use crate::{
    cooked::{iso_user_data_range, CookedSectorReader},
    error::{Error, Result},
    iso9660::{self, DirEntry, EntryKind},
    loader::load_mds,
    util::reader_for_track,
};
use std::{
    fs::{self, File},
    io::{BufWriter, Read, Seek, Write},
    path::{Path, PathBuf},
};

pub struct ExtractOptions {
    /// If `true`, print the tree to stdout and write nothing.
    pub list: bool,
    /// Overwrite a non-empty output directory.
    pub force: bool,
}

pub fn extract<P: AsRef<Path>>(
    mds_file: P,
    output_dir: Option<PathBuf>,
    opts: ExtractOptions,
) -> Result<()> {
    let mds = load_mds(&mds_file)?;
    let track = mds.single_track()?;

    let raw_reader = reader_for_track(&mds_file, track)?;
    let user_data = iso_user_data_range(track.mode, track.sector_data_size())?;
    let mut cooked = CookedSectorReader::new(
        raw_reader,
        track.track_start_offset,
        track.sector_size(),
        user_data,
        track.num_sectors() as u64,
    );

    let tree = iso9660::read_tree(&mut cooked)?;

    if opts.list {
        print_tree(&tree, "");
        return Ok(());
    }

    let out_dir = output_dir.unwrap_or_else(|| default_output_dir(&mds_file));
    prepare_output_dir(&out_dir, opts.force)?;
    write_tree(&mut cooked, &tree, &out_dir)?;
    Ok(())
}

fn check_not_symlink(p: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(p) {
        if meta.file_type().is_symlink() {
            return Err(Error::PathEscape(p.to_string_lossy().into_owned()));
        }
    }
    Ok(())
}

fn default_output_dir<P: AsRef<Path>>(mds_file: P) -> PathBuf {
    let path = mds_file.as_ref();
    let stem = path.file_stem().unwrap_or_else(|| path.as_os_str());
    PathBuf::from(stem)
}

fn prepare_output_dir(dir: &Path, force: bool) -> Result<()> {
    // Refuse a symlinked output dir up front. read_dir/create_dir_all
    // follow symlinks transparently, so without this an attacker who
    // can drop a link into the user's workspace could redirect writes.
    //
    // For relative paths, walk every component — `out/subdir` where the
    // user's `./out` is a pre-existing link to `/etc` must be caught
    // before create_dir_all follows the link.
    //
    // For absolute paths, only check the leaf — the user typed the rest
    // of the path explicitly and platform symlinks like macOS's
    // `/var -> /private/var` or `/tmp -> /private/tmp` are inherent and
    // not attacker-controlled.
    if dir.is_absolute() {
        check_not_symlink(dir)?;
    } else {
        let mut prefix = PathBuf::new();
        for component in dir.components() {
            prefix.push(component);
            check_not_symlink(&prefix)?;
        }
    }
    match fs::read_dir(dir) {
        Ok(mut iter) => {
            if iter.next().is_some() && !force {
                return Err(Error::OutputExists(dir.to_path_buf()));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(dir).map_err(Error::Io)?;
            Ok(())
        }
        Err(e) => Err(Error::Io(e)),
    }
}

fn print_tree(entries: &[DirEntry], indent: &str) {
    for e in entries {
        match &e.kind {
            EntryKind::Dir(children) => {
                println!("{indent}{}/", e.name);
                let next = format!("{indent}  ");
                print_tree(children, &next);
            }
            EntryKind::File { size, .. } => {
                println!("{indent}{}  ({size} bytes)", e.name);
            }
        }
    }
}

fn write_tree<R: Read + Seek>(
    reader: &mut R,
    entries: &[DirEntry],
    dest: &Path,
) -> Result<()> {
    for entry in entries {
        // Every component the ISO supplies must be a plain file/dir name.
        // ISO9660 forbids '/' and '..' in identifiers by spec — but never
        // trust input. Reject anything that could escape `dest`.
        if !is_safe_component(&entry.name) {
            return Err(Error::PathEscape(entry.name.clone()));
        }
        let child_path = dest.join(&entry.name);
        // Refuse to follow a pre-existing symlink at child_path. Without
        // this, an attacker who can drop symlinks into the output dir
        // (or who tricks the user into using --force on a populated dir)
        // could redirect create_dir_all/File::create outside `dest`.
        // symlink_metadata returns the link's own metadata, not the
        // target's — exactly what we need here.
        if let Ok(meta) = fs::symlink_metadata(&child_path) {
            if meta.file_type().is_symlink() {
                return Err(Error::PathEscape(entry.name.clone()));
            }
        }
        match &entry.kind {
            EntryKind::Dir(children) => {
                fs::create_dir_all(&child_path).map_err(Error::Io)?;
                write_tree(reader, children, &child_path)?;
            }
            EntryKind::File { lba, size } => {
                let f = File::create(&child_path).map_err(Error::Io)?;
                let mut bw = BufWriter::new(f);
                iso9660::copy_file(reader, &mut bw, *lba, *size)?;
                // Explicit flush — BufWriter's Drop impl swallows write
                // errors (e.g. ENOSPC on the final buffered chunk), so a
                // partial write would otherwise be reported as success.
                bw.flush().map_err(Error::Io)?;
            }
        }
    }
    Ok(())
}

fn is_safe_component(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    // Explicit reject for chars that are either path-meaningful on some
    // platform or forbidden by the Windows filename rules + ASCII control
    // chars. ISO9660 d-characters and Joliet d1-characters forbid all of
    // these, so a legitimately-authored disc will never trip this check
    // — only malformed/malicious metadata will.
    //
    // Note: this does NOT reject Windows reserved device names like CON,
    // NUL, COM1, etc. Real discs essentially never contain those, and a
    // case-insensitive-with-extension blocklist is a much larger change
    // for marginal benefit. Users extracting on Linux/macOS who then move
    // files to Windows will hit OS-level failures for those rare cases.
    if name
        .chars()
        .any(|c| matches!(c, '/' | '\\' | ':' | '\0' | '*' | '?' | '"' | '<' | '>' | '|' | '\x01'..='\x1F' | '\x7F'))
    {
        return false;
    }
    // Use the platform path parser as a defence-in-depth check for
    // anything else that isn't a plain file/dir name on the current OS:
    // Windows drive prefixes like `C:foo`, UNC roots, parent/current-dir
    // markers, etc. We require exactly one Normal component matching the
    // input verbatim.
    let mut comps = Path::new(name).components();
    matches!(
        (comps.next(), comps.next()),
        (Some(std::path::Component::Normal(c)), None) if c == std::ffi::OsStr::new(name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_separators() {
        assert!(!is_safe_component(""));
        assert!(!is_safe_component("."));
        assert!(!is_safe_component(".."));
        assert!(!is_safe_component("foo/bar"));
        assert!(!is_safe_component("foo\\bar"));
        assert!(!is_safe_component("a\0b"));
        assert!(is_safe_component("README.TXT"));
        assert!(is_safe_component("file with spaces.dat"));
    }

    #[test]
    fn rejects_windows_reserved_chars_and_control_chars() {
        for c in ['*', '?', '"', '<', '>', '|'] {
            let s = format!("foo{c}bar");
            assert!(!is_safe_component(&s), "expected rejection of {s:?}");
        }
        assert!(!is_safe_component("foo\x01bar"));
        assert!(!is_safe_component("foo\x1Fbar"));
        assert!(!is_safe_component("foo\x7Fbar"));
    }

    #[cfg(unix)]
    #[test]
    fn check_not_symlink_rejects_symlinked_path() {
        use std::os::unix::fs::symlink;

        let tmp = std::env::temp_dir().join(format!("mds_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let real = tmp.join("real");
        let link = tmp.join("link");
        fs::create_dir_all(&real).unwrap();
        symlink(&real, &link).unwrap();

        // The helper rejects the symlinked path directly. Used during the
        // prefix walk for relative output dirs and on the leaf for
        // absolute output dirs — see prepare_output_dir.
        let err = check_not_symlink(&link).unwrap_err();
        assert!(
            matches!(err, Error::PathEscape(_)),
            "expected PathEscape, got {err:?}"
        );
        // And accepts a real directory.
        assert!(check_not_symlink(&real).is_ok());
        // And accepts a non-existent path (we only reject existing symlinks).
        assert!(check_not_symlink(&tmp.join("does-not-exist")).is_ok());

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn rejects_windows_drive_prefixes_and_colons() {
        // ISO9660 d-characters and Joliet d1-characters both forbid ':',
        // and a colon would be interpreted as a Windows drive prefix on
        // an NTFS extraction target. Reject unconditionally on every
        // platform so the same disc behaves the same regardless of where
        // it's extracted.
        assert!(!is_safe_component("C:foo"));
        assert!(!is_safe_component(":foo"));
        assert!(!is_safe_component("a:b"));
    }
}
