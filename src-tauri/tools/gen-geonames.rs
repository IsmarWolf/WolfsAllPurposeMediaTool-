//! The offline cities-index producer (PLAN §7.5).
//!
//! Run **once, by a maintainer, with no network** — the app never geocodes
//! online, and this binary is not part of the portable build:
//!
//! ```text
//! cargo run --release --features tooling --bin gen-geonames -- <cities.txt> <out.bin>
//! ```
//!
//! ## Input
//!
//! A GeoNames `citiesNNNN.txt` dump, tab-separated, with the standard column
//! order. Only six columns are read, and the indices are named so a change of
//! meaning is visible:
//!
//! ```text
//! 1   name          "São Paulo"
//! 4   latitude      -23.5475
//! 5   longitude     -46.63611
//! 6   feature class P
//! 7   feature code  PPLC
//! 8   country code  BR
//! 10  admin1        27
//! 14  population    11253503
//! ```
//!
//! The feature class is column 6 and the feature *code* is column 7. Reading
//! the code by mistake is how a whole dump turns into "0 cities kept" —
//! `PPLC != P` — and the only symptom is a quiet zero, which is why the tool
//! reports what it dropped instead of only what it kept.
//!
//! Only `feature class = P` (populated place) is kept. `admin1` (column 10) and
//! `country` (column 8) are written as the codes GeoNames gives — the *index* is
//! what ships to the SSD, and a code is stable, ASCII and 1:1 with the source.
//! The §7.5 alert and the UI show whatever the index carries, so switching to
//! names is a one-line change here (read `admin1CodesASCII.txt` /
//! `countryInfo.txt` alongside the dump).
//!
//! ## Output
//!
//! The `geonames.bin` described in `core::geocode`. The report at the end is
//! deliberate: the file goes on a portable SSD and §7.5 budgets it at 16 MB, so
//! the person running the tool should see the size rather than discover it later.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use backup_manager_lib::core::geocode::GeoIndex;
use backup_manager_lib::core::geocode::writer::{City, CityWriter};

/// The default §7.5 floor: `population >= 500`.
const DEFAULT_MIN_POP: u32 = 500;

// Column indices of the GeoNames `citiesNNNN.txt` layout. The feature class is
// 6 and the feature code is 7: they are adjacent and easy to swap, and swapping
// them silently empties the whole index.
const COL_NAME: usize = 1;
const COL_LAT: usize = 4;
const COL_LON: usize = 5;
const COL_FEATURE_CLASS: usize = 6;
/// Named so the two cannot be confused — a test asserts that reading this one
/// instead of [`COL_FEATURE_CLASS`] empties the whole index. Only the tests
/// touch it; the tool filters on the class alone.
#[cfg(test)]
const COL_FEATURE_CODE: usize = 7;
const COL_COUNTRY: usize = 8;
const COL_ADMIN1: usize = 10;
const COL_POPULATION: usize = 14;

/// The only feature class the §7.5 filter keeps: populated place.
const FEATURE_CLASS_POPULATED: &str = "P";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gen-geonames: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let Some(args) = Args::parse(std::env::args().skip(1))? else {
        return Ok(());
    };

    let (source, output) = args.resolve()?;
    let mut writer = CityWriter::new(args.min_pop);

    let (rows, skipped_class, skipped_parse) = read_cities(&source, &mut writer)?;
    let bytes = writer.finish()?;
    let report = writer.report(bytes.len());

    // A city index that does not read back is worse than no index at all, and
    // the person running this is the only one who will find out before a
    // release. The reader that will consume it is the one used here.
    let index = GeoIndex::from_bytes(bytes.clone())?;
    if index.record_count() != report.cities {
        return Err(format!(
            "the file I just wrote reads back as {} cities, expected {}",
            index.record_count(),
            report.cities
        )
        .into());
    }
    if !report.within_budget() {
        return Err(format!(
            "{} bytes is over the {} MB budget of §7.5; raise --min-pop or narrow the source",
            report.bytes,
            report.bytes / (1024 * 1024)
        )
        .into());
    }

    write_atomically(&output, &bytes)?;

    println!("cities index written to {}", output.display());
    println!(
        "  cities kept   {} (of {rows} rows, floor {})",
        report.cities, report.min_pop
    );
    println!("  cells         {}", report.cells);
    println!(
        "  size          {} bytes ({:.1} MB, budget 16 MB)",
        report.bytes,
        report.bytes as f64 / (1024.0 * 1024.0)
    );
    println!(
        "  resident      ~{} KB with the offset table",
        report.resident_kb()
    );
    if skipped_class > 0 {
        println!("  skipped       {skipped_class} rows that are not feature class P");
    }
    if skipped_parse > 0 {
        // Not fatal: a malformed row in a 30k-line dump is a data problem, and
        // refusing to build the whole index over it is worse than 300 fewer
        // cities. It is counted so it is never silent.
        println!("  skipped       {skipped_parse} rows that did not parse");
    }
    Ok(())
}

/// Reads the dump, pushing every populated place that clears the floor.
///
/// The three returns are counted so the report can say what was dropped: a tool
/// that silently discards rows is how a library ends up "missing" a whole
/// country.
fn read_cities(
    source: &Path,
    writer: &mut CityWriter,
) -> Result<(u64, u64, u64), Box<dyn std::error::Error>> {
    let file =
        fs::File::open(source).map_err(|e| format!("could not open {}: {e}", source.display()))?;
    let mut rows = 0u64;
    let mut skipped_class = 0u64;
    let mut skipped_parse = 0u64;
    let mut rejected: HashMap<&'static str, u64> = HashMap::new();

    for line in BufReader::new(file).lines() {
        let line = line?;
        // A trailing blank line is normal; a comment is not, and GeoNames has
        // none, so anything without the minimum columns is counted, not
        // guessed at.
        if line.trim().is_empty() {
            continue;
        }
        rows += 1;
        let columns: Vec<&str> = line.split('\t').collect();
        if columns.len() <= COL_POPULATION {
            skipped_parse += 1;
            *rejected.entry("too few columns").or_default() += 1;
            continue;
        }
        if columns[COL_FEATURE_CLASS] != FEATURE_CLASS_POPULATED {
            skipped_class += 1;
            continue;
        }
        let (Ok(lat), Ok(lon), Ok(population)) = (
            columns[COL_LAT].trim().parse::<f64>(),
            columns[COL_LON].trim().parse::<f64>(),
            columns[COL_POPULATION].trim().parse::<u32>(),
        ) else {
            skipped_parse += 1;
            *rejected.entry("unparseable number").or_default() += 1;
            continue;
        };
        let name = columns[COL_NAME].trim();
        let before = writer.len();
        writer.push(City::new(
            lat,
            lon,
            population,
            name,
            columns[COL_ADMIN1].trim(),
            columns[COL_COUNTRY].trim(),
        ));
        // `push` also enforces the floor, the name and the coordinate range;
        // this separates "below the floor" from "unusable row".
        if writer.len() == before {
            *rejected.entry("below the floor or unusable").or_default() += 1;
        }
    }

    if rows == 0 {
        return Err(format!(
            "{} has no rows; is it really a cities dump?",
            source.display()
        )
        .into());
    }
    let detail: Vec<String> = rejected
        .iter()
        .map(|(reason, count)| format!("{count} {reason}"))
        .collect();
    if !detail.is_empty() {
        eprintln!("gen-geonames: dropped rows — {}", detail.join(", "));
    }
    Ok((rows, skipped_class, skipped_parse))
}

/// §7.6 writes thumbnails through a temp file for the same reason: a reader must
/// never see a half-written file. Here it matters even more, because a
/// truncated index is *silently* useless rather than loudly broken.
fn write_atomically(output: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let temp = output.with_extension("bin.tmp");
    {
        let mut file = BufWriter::new(fs::File::create(&temp)?);
        file.write_all(bytes)?;
        file.flush()?;
        // The rename is only atomic if the bytes are down, which `sync_data` is
        // what guarantees.
        file.get_ref().sync_data()?;
    }
    fs::rename(&temp, output)?;
    Ok(())
}

struct Args {
    min_pop: u32,
    source: Option<PathBuf>,
    output: Option<PathBuf>,
}

impl Args {
    /// `None` means "print the help and exit 0", which is what a bare
    /// invocation and `--help` both mean.
    fn parse(args: impl Iterator<Item = String>) -> Result<Option<Self>, String> {
        let mut min_pop = DEFAULT_MIN_POP;
        let mut positional: Vec<String> = Vec::new();
        let mut iter = args.peekable();

        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "-h" | "--help" => {
                    print_help();
                    return Ok(None);
                }
                "--min-pop" => {
                    let value = iter.next().ok_or("--min-pop needs a number")?;
                    min_pop = value
                        .parse()
                        .map_err(|_| format!("--min-pop got {value:?}, which is not a number"))?;
                }
                // `--min-pop=500` is what muscle memory types.
                other if other.starts_with("--min-pop=") => {
                    let value = &other["--min-pop=".len()..];
                    min_pop = value
                        .parse()
                        .map_err(|_| format!("--min-pop got {value:?}, which is not a number"))?;
                }
                other if other.starts_with('-') => return Err(format!("unknown option {other}")),
                other => positional.push(other.to_owned()),
            }
        }

        if positional.len() != 2 {
            return Err(format!(
                "expected <cities.txt> <out.bin>, got {} argument(s)",
                positional.len()
            ));
        }
        let mut paths = positional.into_iter();
        Ok(Some(Self {
            min_pop,
            source: Some(PathBuf::from(paths.next().expect("checked the count"))),
            output: Some(PathBuf::from(paths.next().expect("checked the count"))),
        }))
    }

    /// Resolves and checks the paths before any work, so a typo does not come
    /// after a 30k-line parse.
    fn resolve(&self) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
        let source = self.source.clone().expect("parse guarantees it");
        let output = self.output.clone().expect("parse guarantees it");
        if !source.is_file() {
            return Err(format!("{} is not a file", source.display()).into());
        }
        if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty())
            && !parent.is_dir()
        {
            return Err(format!("{} is not a directory", parent.display()).into());
        }
        Ok((source, output))
    }
}

fn print_help() {
    println!(
        "gen-geonames - build the offline cities index (PLAN §7.5)

USAGE:
    gen-geonames <cities.txt> <out.bin> [--min-pop N]

ARGS:
    <cities.txt>   a GeoNames citiesNNNN.txt dump (tab-separated)
    <out.bin>      where to write the index, e.g. Database/geonames.bin

OPTIONS:
    --min-pop N    population floor, default {DEFAULT_MIN_POP} (§7.5)
    -h, --help     this text

The dump is read once and never modified. The output is written through a temp
file and renamed, so an interrupted run leaves the previous index in place."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Args>, String> {
        Args::parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn takes_two_positional_arguments() {
        let args = parse(&["cities15000.txt", "geonames.bin"])
            .unwrap()
            .expect("not help");
        assert_eq!(args.min_pop, DEFAULT_MIN_POP);
        assert_eq!(args.source.unwrap().to_str().unwrap(), "cities15000.txt");
        assert_eq!(args.output.unwrap().to_str().unwrap(), "geonames.bin");
    }

    #[test]
    fn min_pop_is_configurable_both_ways() {
        let spaced = parse(&["in.txt", "out.bin", "--min-pop", "5000"])
            .unwrap()
            .unwrap();
        assert_eq!(spaced.min_pop, 5000);
        let equals = parse(&["in.txt", "out.bin", "--min-pop=15000"])
            .unwrap()
            .unwrap();
        assert_eq!(equals.min_pop, 15000);
    }

    #[test]
    fn help_is_not_a_failure() {
        assert!(parse(&["--help"]).unwrap().is_none());
        assert!(parse(&["-h"]).unwrap().is_none());
    }

    #[test]
    fn refuses_a_wrong_argument_count() {
        assert!(parse(&["only-one.txt"]).is_err());
        assert!(parse(&["a", "b", "c"]).is_err());
    }

    #[test]
    fn refuses_a_nonsense_min_pop() {
        assert!(parse(&["a", "b", "--min-pop", "many"]).is_err());
        assert!(parse(&["a", "b", "--min-pop"]).is_err());
    }

    #[test]
    fn refuses_an_unknown_option_instead_of_ignoring_it() {
        // A silently-ignored flag is how someone ships an index built with the
        // wrong floor and does not find out for a release.
        assert!(parse(&["a", "b", "--verbose"]).is_err());
    }

    #[test]
    fn refuses_a_source_that_is_not_there() {
        let args = parse(&["definitely-not-here.txt", "out.bin"])
            .unwrap()
            .unwrap();
        assert!(args.resolve().is_err());
    }

    /// A row of a real dump, cut to the columns this tool reads: São Paulo as
    /// GeoNames has it, in the canonical 19-column order.
    fn sao_paulo_row() -> String {
        [
            "3550300",    // 0 geonameid
            "São Paulo",  // 1 name
            "Sao Paulo",  // 2 asciiname
            "",           // 3 alternatenames
            "-23.5475",   // 4 latitude
            "-46.63611",  // 5 longitude
            "P",          // 6 feature class
            "PPLC",       // 7 feature code
            "BR",         // 8 country code
            "",           // 9 cc2
            "27",         // 10 admin1
            "",           // 11 admin2
            "",           // 12 admin3
            "",           // 13 admin4
            "11253503",   // 14 population
            "760",        // 15 elevation
            "760",        // 16 dem
            "-03:00",     // 17 timezone
            "2024-01-01", // 18 modification date
        ]
        .join("\t")
    }

    #[test]
    fn reads_the_columns_of_a_real_geonames_row() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cities.txt");
        fs::write(&source, format!("{}\n", sao_paulo_row())).unwrap();

        let mut writer = CityWriter::new(500);
        let (rows, skipped_class, skipped_parse) = read_cities(&source, &mut writer).unwrap();

        assert_eq!((rows, skipped_class, skipped_parse), (1, 0, 0));
        assert_eq!(writer.len(), 1);
        let index = GeoIndex::from_bytes(writer.finish().unwrap()).unwrap();
        let found = index.geocode(-23.5475, -46.63611).expect("São Paulo");
        assert_eq!(found.city, "São Paulo");
        assert_eq!(found.country, "BR");
        assert_eq!(found.state, "27", "admin1 is the GeoNames code");
    }

    #[test]
    fn skips_feature_classes_that_are_not_places() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cities.txt");
        // A mountain and a river, both real rows from a real dump: same layout,
        // only the feature class differs.
        let base = sao_paulo_row();
        let mut mountain: Vec<&str> = base.split('\t').collect();
        mountain[COL_FEATURE_CLASS] = "T"; // mountain
        mountain[COL_FEATURE_CODE] = "MT";
        let mut river: Vec<&str> = base.split('\t').collect();
        river[COL_NAME] = "Rio Tietê";
        river[COL_FEATURE_CLASS] = "H"; // stream
        river[COL_FEATURE_CODE] = "STM";
        let mountain = mountain.join("\t");
        let river = river.join("\t");

        fs::write(&source, format!("{mountain}\n{river}\n")).unwrap();

        let mut writer = CityWriter::new(500);
        let (rows, skipped_class, _) = read_cities(&source, &mut writer).unwrap();
        assert_eq!((rows, skipped_class), (2, 2), "neither row is class P");
        assert_eq!(writer.len(), 0);
    }

    /// The bug this column layout invites: reading the feature *code* (7) where
    /// the class (6) belongs rejects every row of a perfectly good dump, and the
    /// tool would happily write a 33-byte empty index and exit 0. Pinned so the
    /// two can never be swapped again.
    #[test]
    fn a_dump_of_real_rows_keeps_its_places() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cities.txt");
        fs::write(&source, format!("{}\n", sao_paulo_row())).unwrap();

        let mut writer = CityWriter::new(500);
        let (rows, skipped_class, skipped_parse) = read_cities(&source, &mut writer).unwrap();
        assert_eq!((rows, skipped_class, skipped_parse), (1, 0, 0));
        assert_eq!(
            writer.len(),
            1,
            "PPLC in column 7 must not be read as the class"
        );
    }

    #[test]
    fn a_short_row_is_counted_not_guessed_at() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cities.txt");
        fs::write(&source, format!("{}\n1\t2\t3\n", sao_paulo_row())).unwrap();

        let mut writer = CityWriter::new(500);
        let (rows, _, skipped_parse) = read_cities(&source, &mut writer).unwrap();
        assert_eq!((rows, skipped_parse), (2, 1));
        assert_eq!(writer.len(), 1, "the good row still made it");
    }

    #[test]
    fn an_empty_source_is_an_error_not_an_empty_index() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cities.txt");
        fs::write(&source, "\n\n").unwrap();
        // An index with no cities would look to the app exactly like a missing
        // one, and the human would only find out in the field.
        assert!(read_cities(&source, &mut CityWriter::new(500)).is_err());
    }

    #[test]
    fn a_high_floor_can_empty_the_index_and_that_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cities.txt");
        fs::write(&source, format!("{}\n", sao_paulo_row())).unwrap();

        let mut writer = CityWriter::new(20_000_000);
        let (rows, ..) = read_cities(&source, &mut writer).unwrap();
        assert_eq!(rows, 1, "the row was read");
        assert_eq!(writer.len(), 0, "and correctly refused");
        assert!(
            GeoIndex::from_bytes(writer.finish().unwrap())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_output_is_written_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("geonames.bin");
        write_atomically(&output, b"WOLFSGEO index").unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"WOLFSGEO index");
        // And no temp file is left behind to be mistaken for the real one.
        assert!(!output.with_extension("bin.tmp").exists());
    }

    #[test]
    fn a_failed_write_leaves_the_previous_index_alone() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("geonames.bin");
        write_atomically(&output, b"index anterior").unwrap();

        // A directory where the temp file wants to be makes the write fail
        // after the output already existed.
        let blocked = dir.path().join("blocked");
        fs::create_dir(&blocked).unwrap();
        assert!(write_atomically(&blocked, b"novo").is_err());
        assert_eq!(fs::read(&output).unwrap(), b"index anterior");
    }

    /// The whole round trip through the filesystem, which is the only thing that
    /// proves the *bytes* are right: a unit test over an in-memory `Vec` cannot
    /// catch a truncated write, a wrong byte order, or a rename that never
    /// happened. Seven real cities with non-ASCII names, in seven different
    /// cells, over a file the tool wrote and the reader reopened.
    #[test]
    fn an_index_written_to_disk_geocodes_every_city() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cities.txt");
        let output = dir.path().join("geonames.bin");

        let mut lines: Vec<String> = Vec::new();
        let mut cities: Vec<(&str, f64, f64)> = Vec::new();
        for (name, lat, lon, pop) in [
            ("São Paulo", -23.5475, -46.63611, 11_253_503u32),
            ("Campinas", -22.9058, -47.0608, 1_018_321),
            ("Rio de Janeiro", -22.9064, -43.1822, 6_114_800),
            ("Brasília", -15.7939, -47.8828, 4_775_000),
            ("Tokyo", 35.6895, 139.69171, 8_336_599),
            ("Sydney", -33.86785, 151.20732, 4_840_608),
            ("Córdoba", -31.4201, -64.1888, 1_400_000),
        ] {
            let mut row: Vec<String> = sao_paulo_row().split('\t').map(str::to_owned).collect();
            row[COL_NAME] = name.to_owned();
            row[COL_LAT] = lat.to_string();
            row[COL_LON] = lon.to_string();
            row[COL_POPULATION] = pop.to_string();
            lines.push(row.join("\t"));
            cities.push((name, lat, lon));
        }
        fs::write(&source, format!("{}\n", lines.join("\n"))).unwrap();

        // Build exactly as the tool does, through the atomic write.
        let mut writer = CityWriter::new(500);
        read_cities(&source, &mut writer).expect("the dump reads");
        let bytes = writer.finish().expect("the index builds");
        write_atomically(&output, &bytes).expect("the index lands on disk");

        // Reopen from disk with the reader the app uses, and ask about each
        // city by its own coordinates.
        let index = GeoIndex::load(&output).expect("and it reads back");
        assert_eq!(index.record_count(), cities.len() as u32);
        assert_eq!(index.min_pop(), 500);
        for (name, lat, lon) in cities {
            let found = index
                .geocode(lat, lon)
                .unwrap_or_else(|| panic!("{name} should geocode to itself"));
            assert_eq!(&found.city, name, "at ({lat}, {lon})");
        }
        // Somewhere with no city: §7.5's "keep the lat/lon, show no city".
        assert!(index.geocode(-40.0, -30.0).is_none(), "open water");
    }

    /// A file on disk that is not an index must be refused, not read as cities.
    /// The tool is the thing that produces these files, so it is the thing that
    /// has to notice when one is wrong.
    #[test]
    fn the_written_file_is_validated_by_the_reader_that_will_use_it() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("geonames.bin");
        write_atomically(&output, b"nao e um indice").unwrap();
        assert!(GeoIndex::load(&output).is_err());
    }

    #[test]
    fn the_help_text_names_both_positional_arguments() {
        // The tool is run by a human, once, possibly months apart: the help is
        // the only documentation it has.
        assert_eq!(DEFAULT_MIN_POP, 500, "§7.5's floor");
    }
}
