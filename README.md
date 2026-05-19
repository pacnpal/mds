# mds

Utilities for reading and converting .mds/.mdf disk image files

> **Fork notice:** This is a fork of [delta62/mds](https://github.com/delta62/mds).
> It adds an `mds extract` subcommand that reads files straight out of an `.mdf`
> without producing an intermediate ISO (Joliet-aware, with path-traversal and
> symlink guards). The ISO conversion path also carries fork fixes over upstream:
> correct cooked-sector extraction from raw-mode tracks, MODE2/2336 (0x920)
> handling, and seeking to the track start offset before reading.

This tool converts .mdf/.mds files into .iso or .cue/.bin files. I wrote this
since I found that `mdf2iso` was creating bad images for some discs that I
tried, and the iso file format cannot handle multi-track images at all.

This program reads from `.mds` files, which are binary metadata files that
describe the contents of their accompanying `.mdf` files. This is in contrast to
`mdf2iso`, which attempts to parse the type of disc image out of the mdf data
file itself. That said, you will need the .mds metadata file to use this
program.

## Installation

### Pre-built binaries

Grab the archive for your platform from
[the releases page](https://github.com/pacnpal/mds/releases). Each archive also
contains a matching `.sha256` checksum plus this README and the changelog.

| You're on | Download |
|---|---|
| Intel/AMD Linux | `mds-linux-x86_64.tar.gz` (or `-musl`) |
| ARM Linux (Raspberry Pi 4/5 64-bit, ARM servers) | `mds-linux-aarch64.tar.gz` (or `-musl`) |
| Intel Mac | `mds-macos-x86_64.tar.gz` |
| Apple Silicon Mac (M1–M4) | `mds-macos-aarch64.tar.gz` |
| Windows 10/11 (Intel/AMD) | `mds-windows-x86_64.zip` |
| Windows on ARM | `mds-windows-aarch64.zip` |

**glibc vs musl (Linux only):** the plain build links your system glibc — use it
on Ubuntu/Debian/Fedora/Arch/etc. The `-musl` build is fully static (no libc
dependency), for Alpine, minimal containers, old distros, or if the glibc build
reports a `GLIBC_x.xx not found` mismatch. When in doubt, `-musl` always works.

#### Linux

```bash
sha256sum -c mds-linux-x86_64.tar.gz.sha256   # optional integrity check
tar xzf mds-linux-x86_64.tar.gz
sudo install mds /usr/local/bin/              # or: chmod +x mds && ./mds info disc.mds
```

The glibc build needs glibc 2.31+ (Ubuntu 20.04 and newer); switch to `-musl` if
you hit a version error.

#### macOS

```bash
shasum -a 256 -c mds-macos-aarch64.tar.gz.sha256
tar xzf mds-macos-aarch64.tar.gz
xattr -d com.apple.quarantine mds             # clear Gatekeeper quarantine (see below)
./mds info disc.mds
```

These binaries aren't code-signed or notarized, so on first run macOS will say
*"mds cannot be opened because the developer cannot be verified."* Clear it with
the `xattr -d com.apple.quarantine ./mds` shown above, or via Finder →
right-click the binary → **Open** → **Open** (only needed once). Apple Silicon
Macs can run the `x86_64` build under Rosetta, but use `mds-macos-aarch64` for
native speed.

#### Windows

```powershell
Expand-Archive mds-windows-x86_64.zip
.\mds-windows-x86_64\mds.exe info disc.mds
```

- It's a command-line tool: run it from PowerShell or `cmd`, not by
  double-clicking.
- SmartScreen may warn about the unsigned exe — click **More info → Run anyway**.
- The build needs the Visual C++ Redistributable, present on essentially all
  Windows 10/11 systems. If you see `VCRUNTIME140.dll missing` on a bare install,
  grab the [latest VC++ redistributable](https://aka.ms/vs/17/release/vc_redist.x64.exe)
  (`vc_redist.arm64.exe` on ARM).

### Building from source

Use `cargo build --release` with the standard toolchain; the binary lands at
`target/release/mds`.

## Usage

Every command takes the `.mds` file as its argument and expects the matching
`.mdf` data file alongside it (same basename, e.g. `disc.mds` + `disc.mdf`). The
`.mds` is only the metadata; the `.mdf` holds the actual disc contents.

### Printing mds metadata

Run `mds info <my_file.mds>` to view the contents of an mdf image

```
# mds info my_file.mds
/home/sam/my_file.mds
MDS v1.3 | CD-ROM, 574 bytes, 1 session, 2 tracks
Session 1
  First sector:   -150      (0xFFFFFF6A)
  Last sector:    294066    (0x47CB2)
  Total sectors:  294216    (0x47D48)
  Track 1
    Mode:         Mode2
    Subchannels:  Eight
    Data file:    /home/sam/my_file.mdf
    Time offset:  00:02.000
    First byte:   0         (0x0)
    First sector: 0         (0x0)
    Sectors:      278166    (0x43E96)
    Sector size:  2448      (0x990)
    Approx Size:  680MB
  Track 2
    Mode:         Audio
    Subchannels:  Eight
    Data file:    /home/sam/my_file.mdf
    Time offset:  61:52.916
    First byte:   680950368 (0x28967A60)
    First sector: 278316    (0x43F2C)
    Sectors:      15750     (0x3D86)
    Sector size:  2448      (0x990)
    Approx Size:  38MB
```

### Converting to iso

Run `mds convert --format iso <my_image.mds>` to convert the contents of an mdf to an iso
file. Note that iso files can only contain one track, so if you have a
multi-track mdf you'll need to convert to a different format.

### Converting to bin/cue

Run `mds convert --format cue <my_image.mds>` to convert the contents of an mdf to bin
and cue files. This format does support multiple tracks.

### Extracting files

Run `mds extract <my_image.mds>` to read files straight out of an mdf without
producing an intermediate iso. The default output directory is the .mds basename
(e.g. `PMagic_8/` for `PMagic_8.mds`); override it with `-o <DIR>`. Joliet
(Unicode) names are preferred when the disc has a supplementary volume
descriptor; otherwise the primary ISO9660 names are used.

```bash
mds extract my_image.mds                 # write files to ./my_image/
mds extract my_image.mds -o out/         # write files to ./out/
mds extract my_image.mds --list          # print the tree, write nothing
mds extract my_image.mds --force         # allow writing into a non-empty dir
```

Only single-track data discs are supported. For multi-track discs, convert to
bin/cue first.
