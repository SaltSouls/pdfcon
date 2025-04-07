use crate::cli::build_command;
use crate::constants::physical_cores;
use crate::pack::Pack;
use crate::unpack::Unpack;
use clap::{ArgAction, Command, arg, command, value_parser};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PDFCon {
    UNPACK(Unpack),
    PACK(Pack),
}

pub fn get_command() -> PDFCon {
    let matches = build_command().get_matches();

    let total_physical = physical_cores();
    match matches.subcommand() {
        Some(("pack", sub_matches)) => PDFCon::PACK(Pack {
            optimize: sub_matches.get_flag("OPTIMIZE"),
            in_directory: sub_matches
                .get_one::<PathBuf>("IN_DIRECTORY")
                .unwrap()
                .to_owned(),
            out_file: sub_matches
                .get_one::<PathBuf>("OUT_FILE")
                .unwrap_or(&PathBuf::from("output.pdf"))
                .to_owned(),
            threads: sub_matches
                .get_one::<usize>("THREADS")
                .copied()
                .unwrap_or(total_physical / 2)
                .clamp(1usize, total_physical * 2),
        }),
        Some(("unpack", sub_matches)) => PDFCon::UNPACK(Unpack {
            threads: sub_matches
                .get_one::<usize>("THREADS")
                .copied()
                .unwrap_or(total_physical / 2)
                .clamp(1usize, total_physical * 2),
            out_directory: sub_matches
                .get_one::<PathBuf>("OUT_DIRECTORY")
                .unwrap_or(&PathBuf::from("output/"))
                .to_owned(),
            in_file: sub_matches
                .get_one::<PathBuf>("IN_FILE")
                .unwrap()
                .to_owned(),
            optimize: sub_matches
                .get_one::<bool>("OPTIMIZE")
                .copied()
                .unwrap_or(false),
        }),
        _ => unreachable!(
            "Subcommands are mandatory. It should not be possible to reach this branch"
        ),
    }
}
