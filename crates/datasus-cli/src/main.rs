use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use arrow_array::RecordBatchReader;
use clap::{Args, Parser, Subcommand, ValueEnum};
use datasus_arrow::{BatchReader, DEFAULT_BATCH_SIZE};
use datasus_core::{ColType, Encoding, ReadOptions};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Read DATASUS .dbc/.dbf files and convert them to Parquet, CSV or DBF.
#[derive(Parser)]
#[command(name = "datasus", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert .dbc/.dbf files to Parquet or CSV
    Convert(Convert),
    /// Show the fields of a .dbc/.dbf file and the types they would be read as
    Schema(SchemaArgs),
    /// Decompress .dbc files into plain .dbf files
    Decompress(Decompress),
}

#[derive(Clone, Copy, ValueEnum, PartialEq)]
enum Format {
    Parquet,
    Csv,
}

#[derive(Clone, Copy, ValueEnum)]
enum Codec {
    Zstd,
    Snappy,
    None,
}

#[derive(Args)]
struct ReadArgs {
    /// Columns to keep, comma-separated (case-insensitive)
    #[arg(short, long, value_delimiter = ',')]
    select: Option<Vec<String>>,
    /// Read at most this many rows (deleted records do not count)
    #[arg(short, long)]
    n_max: Option<u64>,
    /// Keep leading/trailing whitespace in character fields
    #[arg(long)]
    no_trim: bool,
    /// Encoding of character fields: latin1 or utf-8
    #[arg(long, default_value = "latin1", value_parser = parse_encoding)]
    encoding: Encoding,
    /// Type numeric fields from the header only (no decimals = integer)
    #[arg(long)]
    no_guess_types: bool,
    /// Force a column type: NAME=character|integer|double|logical|date
    #[arg(long = "col-type", value_name = "NAME=TYPE", value_parser = parse_col_type)]
    col_types: Vec<(String, ColType)>,
    /// Read DBF date fields (type D) as dates instead of text
    #[arg(long)]
    parse_dates: bool,
}

impl ReadArgs {
    fn options(&self) -> ReadOptions {
        ReadOptions {
            select: self.select.clone(),
            n_max: self.n_max,
            trim_ws: !self.no_trim,
            encoding: self.encoding,
            guess_types: !self.no_guess_types,
            col_types: self.col_types.clone(),
            parse_dates: self.parse_dates,
        }
    }
}

#[derive(Args)]
struct Convert {
    /// Input files
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Output file (single input only)
    #[arg(short, long, conflicts_with = "out_dir")]
    output: Option<PathBuf>,
    /// Directory for the outputs (default: next to each input)
    #[arg(short = 'd', long)]
    out_dir: Option<PathBuf>,
    /// Output format (default: from the --output extension, else parquet)
    #[arg(short, long)]
    format: Option<Format>,
    #[command(flatten)]
    read: ReadArgs,
    /// Keep the original column names instead of lowercasing them
    #[arg(long)]
    keep_names: bool,
    /// Parquet compression
    #[arg(long, default_value = "zstd")]
    compression: Codec,
    /// Rows per batch (Parquet row groups are at most this size)
    #[arg(long, default_value_t = DEFAULT_BATCH_SIZE)]
    batch_size: usize,
    /// Overwrite existing outputs
    #[arg(long)]
    force: bool,
    /// Only print errors
    #[arg(short, long)]
    quiet: bool,
}

#[derive(Args)]
struct SchemaArgs {
    input: PathBuf,
    #[command(flatten)]
    read: ReadArgs,
}

#[derive(Args)]
struct Decompress {
    /// Input files
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Output file (single input only)
    #[arg(short, long, conflicts_with = "out_dir")]
    output: Option<PathBuf>,
    /// Directory for the outputs (default: next to each input)
    #[arg(short = 'd', long)]
    out_dir: Option<PathBuf>,
    /// Overwrite existing outputs
    #[arg(long)]
    force: bool,
    /// Only print errors
    #[arg(short, long)]
    quiet: bool,
}

fn parse_encoding(s: &str) -> std::result::Result<Encoding, String> {
    Encoding::from_name(s).ok_or_else(|| format!("unsupported encoding '{s}' (use latin1 or utf-8)"))
}

fn parse_col_type(s: &str) -> std::result::Result<(String, ColType), String> {
    let (name, ty) = s.split_once('=').ok_or("expected NAME=TYPE")?;
    let ty = ColType::from_name(&ty.to_ascii_lowercase()).ok_or_else(|| {
        format!("unknown type '{ty}' (use character, integer, double, logical or date)")
    })?;
    Ok((name.to_string(), ty))
}

/// Where the output for `input` goes: `--output`, or the input's file name
/// with a new extension, inside `--out-dir` when given.
fn output_path(
    input: &Path,
    output: &Option<PathBuf>,
    out_dir: &Option<PathBuf>,
    ext: &str,
) -> Result<PathBuf> {
    if let Some(o) = output {
        return Ok(o.clone());
    }
    let name = input.file_name().ok_or("input has no file name")?;
    let name = Path::new(name).with_extension(ext);
    Ok(match out_dir {
        Some(d) => d.join(name),
        None => input.with_file_name(name),
    })
}

/// Write through a `.part` file that is renamed on success, so an
/// interrupted run never leaves a truncated output behind.
fn write_atomic(out: &Path, force: bool, write: impl FnOnce(File) -> Result<()>) -> Result<()> {
    if out.exists() && !force {
        return Err(format!("{} already exists (use --force to overwrite)", out.display()).into());
    }
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let mut part = out.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let result = File::create(&part).map_err(Into::into).and_then(write);
    match result {
        Ok(()) => Ok(fs::rename(&part, out)?),
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(e)
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn convert_one(input: &Path, args: &Convert) -> Result<()> {
    let format = args.format.unwrap_or_else(|| {
        match args.output.as_deref().and_then(Path::extension).and_then(|e| e.to_str()) {
            Some(e) if e.eq_ignore_ascii_case("csv") => Format::Csv,
            _ => Format::Parquet,
        }
    });
    let ext = if format == Format::Csv { "csv" } else { "parquet" };
    let out = output_path(input, &args.output, &args.out_dir, ext)?;
    if same_file(input, &out) {
        return Err("output would overwrite the input".into());
    }

    let started = Instant::now();
    let table = datasus_core::open(input, &args.read.options())?;
    for w in table.warnings() {
        eprintln!("warning: {}: {w}", input.display());
    }
    if !table.unmatched_select().is_empty() {
        eprintln!(
            "warning: {}: --select matched no column: {}",
            input.display(),
            table.unmatched_select().join(", ")
        );
    }
    let (rows, cols) = (table.n_rows(), table.n_cols());
    let reader = BatchReader::new(table, !args.keep_names, args.batch_size);
    let schema = reader.schema();

    write_atomic(&out, args.force, |file| {
        match format {
            Format::Parquet => {
                let compression = match args.compression {
                    Codec::Zstd => Compression::ZSTD(ZstdLevel::default()),
                    Codec::Snappy => Compression::SNAPPY,
                    Codec::None => Compression::UNCOMPRESSED,
                };
                let props = WriterProperties::builder().set_compression(compression).build();
                let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
                for batch in reader {
                    writer.write(&batch?)?;
                }
                writer.close()?;
            }
            Format::Csv => {
                let mut file = BufWriter::new(file);
                // Written by hand so an empty table still gets a header.
                let header: Vec<String> =
                    schema.fields().iter().map(|f| csv_quote(f.name())).collect();
                writeln!(file, "{}", header.join(","))?;
                let mut writer =
                    arrow_csv::WriterBuilder::new().with_header(false).build(&mut file);
                for batch in reader {
                    writer.write(&batch?)?;
                }
                drop(writer);
                file.flush()?;
            }
        }
        Ok(())
    })?;

    if !args.quiet {
        eprintln!(
            "{} -> {} ({rows} rows x {cols} columns, {:.2}s)",
            input.display(),
            out.display(),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

fn csv_quote(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn schema(args: &SchemaArgs) -> Result<()> {
    let table = datasus_core::open(&args.input, &args.read.options())?;
    for w in table.warnings() {
        eprintln!("warning: {w}");
    }
    println!("file:      {}", args.input.display());
    println!("container: {:?}", table.data().container());
    println!(
        "records:   {} declared, {} to read",
        table.declared_records(),
        table.n_rows()
    );
    println!("fields:    {} ({} selected)", table.fields().len(), table.n_cols());
    println!();
    let width = table.fields().iter().map(|f| f.name.chars().count()).max().unwrap_or(4).max(4);
    println!("{:<width$}  dbf  len  dec  type", "name");
    for f in table.fields() {
        println!(
            "{:<width$}  {:<3}  {:>3}  {:>3}  {}",
            f.name,
            f.dbf_type as char,
            f.len,
            f.dec,
            f.col_type.map_or("-", ColType::name)
        );
    }
    Ok(())
}

fn decompress_one(input: &Path, args: &Decompress) -> Result<()> {
    let out = output_path(input, &args.output, &args.out_dir, "dbf")?;
    if same_file(input, &out) {
        return Err("output would overwrite the input (already a .dbf?)".into());
    }
    let data = datasus_core::decode(fs::read(input)?)?;
    write_atomic(&out, args.force, |mut file| {
        file.write_all(data.header())?;
        file.write_all(data.records())?;
        Ok(())
    })?;
    if !args.quiet {
        eprintln!(
            "{} -> {} ({} bytes)",
            input.display(),
            out.display(),
            data.header().len() + data.records().len()
        );
    }
    Ok(())
}

/// Run `f` on every input; report failures and keep going.
fn for_each(inputs: &[PathBuf], single_output: bool, f: impl Fn(&Path) -> Result<()>) -> ExitCode {
    if single_output && inputs.len() > 1 {
        eprintln!("error: --output takes a single input; use --out-dir for several files");
        return ExitCode::from(2);
    }
    let mut failed = 0;
    for input in inputs {
        if let Err(e) = f(input) {
            eprintln!("error: {}: {e}", input.display());
            failed += 1;
        }
    }
    if failed > 0 {
        if inputs.len() > 1 {
            eprintln!("{failed} of {} files failed", inputs.len());
        }
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Convert(args) => {
            for_each(&args.inputs, args.output.is_some(), |i| convert_one(i, &args))
        }
        Command::Decompress(args) => {
            for_each(&args.inputs, args.output.is_some(), |i| decompress_one(i, &args))
        }
        Command::Schema(args) => match schema(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {}: {e}", args.input.display());
                ExitCode::FAILURE
            }
        },
    }
}
