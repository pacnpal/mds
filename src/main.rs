mod args;
mod convert;
mod cooked;
mod error;
mod extract;
mod info;
mod iso9660;
mod loader;
mod mds;
mod timecode;
mod util;

use args::{Args, Command, ConvertArgs, ExtractArgs, OutputFormat};
use clap::Parser;
use convert::{convert_to_cue_bin, convert_to_iso};
use extract::{extract as extract_files, ExtractOptions};
use info::info;

fn main() {
    let args = Args::parse();

    let result = match args.command {
        Command::Info(args) => info(&args.mds_file),
        Command::Convert(ConvertArgs { mds_file, format }) => match format {
            OutputFormat::Iso => convert_to_iso(&mds_file),
            OutputFormat::Cue => convert_to_cue_bin(&mds_file),
        },
        Command::Extract(ExtractArgs {
            mds_file,
            output,
            list,
            force,
        }) => extract_files(&mds_file, output, ExtractOptions { list, force }),
    };

    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
