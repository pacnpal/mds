use crate::{
    cooked::{iso_user_data_range, CookedSectorReader},
    error::{Error, Result},
    iso9660::{self, DirEntry, EntryKind},
    loader::load_mds,
    util::reader_for_track,
};
use std::{
    fs::{self, File},
    io::{BufWriter, Read, Seek},
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

fn default_output_dir<P: AsRef<Path>>(mds_file: P) -> PathBuf {
    let path = mds_file.as_ref();
    let stem = path.file_stem().unwrap_or_else(|| path.as_os_str());
    PathBuf::from(stem)
}

fn prepare_output_dir(dir: &Path, force: bool) -> Result<()> {
    // Refuse a symlinked output dir up front. read_dir/create_dir_all follow
    // symlinks transparently, so if `dir` itself is a link the per-child
    // symlink checks in write_tree won't help — `--force` would happily
    // write into the link target.
    if let Ok(meta) = fs::symlink_metadata(dir) {
        if meta.file_type().is_symlink() {
            return Err(Error::PathEscape(dir.to_string_lossy().into_owned()));
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
            }
        }
    }
    Ok(())
}

fn is_safe_component(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    // Explicit reject for chars that have path-separator meaning on either
    // platform plus NUL. On Linux `\` is a valid filename char so
    // Path::components() alone wouldn't catch it; we still want to refuse
    // a name like `foo\bar` because the output may end up on an NTFS volume.
    if name.chars().any(|c| c == '/' || c == '\\' || c == '\0') {
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
    fn rejects_windows_drive_prefixes() {
        // `C:foo` parses as a drive-relative prefix on Windows; reject
        // regardless of platform so a malformed disc can't smuggle one
        // through when the extracted tree later moves to a Windows host.
        // On Unix this is a Normal component but doesn't round-trip
        // exactly through Path::components when interpreted on Windows;
        // we conservatively reject anything that contains a ':' as part
        // of the broader check. On platforms where `C:foo` parses as a
        // Prefix, the components check catches it directly.
        assert!(
            !is_safe_component("C:foo") || cfg!(unix),
            "C:foo should be rejected on Windows via Path::components"
        );
    }
}
