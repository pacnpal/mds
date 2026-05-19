use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Utilities for reading and converting .mds/.mdf disk image files
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Convert .mdf/.mds files to other formats
    Convert(ConvertArgs),

    /// Extract files directly from an .mdf without producing an intermediate ISO
    Extract(ExtractArgs),

    /// Print metadata contained by .mds files
    Info(InfoArgs),
}

#[derive(Clone, Debug, ValueEnum)]
pub enum OutputFormat {
    /// Convert into an .iso file. ISO files can only contain one track.
    Iso,

    /// Convert into .bin and .cue files. This format supports multiple tracks.
    Cue,
}

#[derive(ClapArgs, Debug)]
pub struct InfoArgs {
    /// Path to the .mds file to print information about
    pub mds_file: PathBuf,
}

#[derive(ClapArgs, Debug)]
pub struct ConvertArgs {
    /// Path to the .mds file to convert
    pub mds_file: PathBuf,

    /// The format to convert into
    #[arg(long, value_enum)]
    pub format: OutputFormat,
}

#[derive(ClapArgs, Debug)]
pub struct ExtractArgs {
    /// Path to the .mds file to extract from
    pub mds_file: PathBuf,

    /// Directory to write extracted files to. Defaults to the .mds file's
    /// basename (e.g. `PMagic_8/` for `PMagic_8.mds`).
    #[arg(short = 'o', long = "output")]
    pub output: Option<PathBuf>,

    /// Print the directory tree to stdout without writing any files.
    #[arg(long)]
    pub list: bool,

    /// Allow extracting into a non-empty output directory.
    #[arg(long)]
    pub force: bool,
}
