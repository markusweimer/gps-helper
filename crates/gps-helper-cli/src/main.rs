//! Command-line interface for gps-helper.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::builder::PossibleValuesParser;
use clap::{Args, Parser, Subcommand, ValueEnum};
use gps_helper::http::{HttpClient, HttpConfig};
use gps_helper::{AlignOptions, DEFAULT_CONTEXT, Error, Progress, TracePoint, backends, gpx};

/// Snap GPS traces to OSM roads and/or simplify them by keeping only points
/// near road changes.
#[derive(Parser)]
#[command(name = "gps-helper", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Snap a GPX trace to OSM roads.
    Align {
        #[command(flatten)]
        io: IoArgs,
        #[command(flatten)]
        backend: BackendArgs,
    },
    /// Simplify an aligned GPX trace (keep points near road changes).
    Simplify {
        #[command(flatten)]
        io: IoArgs,
        #[command(flatten)]
        context: ContextArg,
        /// Output format: a GPX track or a GPX route.
        #[arg(short, long, value_enum, default_value_t = Format::Track)]
        format: Format,
    },
    /// Align then simplify in a single run.
    Process {
        /// Input GPX file.
        input: PathBuf,
        /// Base name for outputs. Defaults to input name. Produces
        /// <base>.aligned.gpx, <base>.simplified.gpx, <base>.route.gpx.
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        backend: BackendArgs,
        #[command(flatten)]
        context: ContextArg,
    },
}

#[derive(Args)]
struct IoArgs {
    /// Input GPX file.
    input: PathBuf,
    /// Output GPX file.
    #[arg(short, long)]
    output: PathBuf,
}

#[derive(Args)]
struct ContextArg {
    /// Points to keep before & after each road change.
    #[arg(long, default_value_t = DEFAULT_CONTEXT)]
    context: usize,
}

#[derive(Args)]
struct BackendArgs {
    /// Map-matching backend.
    #[arg(long, default_value = "valhalla", value_parser = PossibleValuesParser::new(backends::available()))]
    backend: String,
    /// Override the backend base URL.
    #[arg(long)]
    backend_url: Option<String>,
    /// Max points per backend request.
    #[arg(long, default_value_t = 100)]
    chunk_size: usize,
    /// Overlapping points between chunks.
    #[arg(long, default_value_t = 5)]
    overlap: usize,
    /// Max requests per second.
    #[arg(long, default_value_t = 1.0)]
    rate_limit: f64,
    /// HTTP timeout in seconds.
    #[arg(long, default_value_t = 30.0, value_parser = parse_timeout)]
    timeout: f64,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Track,
    Route,
}

fn parse_timeout(text: &str) -> Result<f64, String> {
    let seconds: f64 = text.parse().map_err(|e| format!("{e}"))?;
    match Duration::try_from_secs_f64(seconds) {
        Ok(duration) if !duration.is_zero() => Ok(seconds),
        _ => Err("must be a positive number of seconds".to_owned()),
    }
}

/// A failed run: either a library error, or bad usage with its own exit code.
enum Failure {
    Library(Error),
    Usage(String),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Library(error)
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Library(error)) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
        Err(Failure::Usage(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<(), Failure> {
    match cli.command {
        Command::Align { io, backend } => {
            let points = load(&io.input)?;
            let aligned = align(&points, &backend)?;
            eprintln!("Writing {} ...", io.output.display());
            gpx::save_points(&io.output, &aligned)?;
            eprintln!("Done: {} -> {} points.", points.len(), aligned.len());
        }
        Command::Simplify {
            io,
            context,
            format,
        } => {
            let points = load(&io.input)?;
            if !gpx::has_any_way_id(&points) {
                return Err(Failure::Usage(
                    "input has no <gh:way_id> extensions; run `gps-helper align` first or use \
                     `gps-helper process`."
                        .to_owned(),
                ));
            }
            let result = match format {
                Format::Route => {
                    let result = gps_helper::simplify_to_route(&points);
                    eprintln!("Writing route {} ...", io.output.display());
                    gpx::save_route(&io.output, &result)?;
                    result
                }
                Format::Track => {
                    let result = gps_helper::simplify(&points, context.context);
                    eprintln!("Writing {} ...", io.output.display());
                    gpx::save_points(&io.output, &result)?;
                    result
                }
            };
            eprintln!(
                "Done: {} -> {} points ({} removed).",
                points.len(),
                result.len(),
                points.len() - result.len()
            );
        }
        Command::Process {
            input,
            output,
            backend,
            context,
        } => {
            let base = output.as_deref().unwrap_or(&input);
            let path_aligned = suffixed_path(base, "aligned");
            let path_simplified = suffixed_path(base, "simplified");
            let path_route = suffixed_path(base, "route");

            let points = load(&input)?;
            let aligned = align(&points, &backend)?;

            eprintln!("Writing {} ...", path_aligned.display());
            gpx::save_points(&path_aligned, &aligned)?;

            eprintln!("Simplifying ...");
            let simplified = gps_helper::simplify(&aligned, context.context);
            eprintln!("Writing {} ...", path_simplified.display());
            gpx::save_points(&path_simplified, &simplified)?;

            let route = gps_helper::simplify_to_route(&aligned);
            eprintln!("Writing {} ...", path_route.display());
            gpx::save_route(&path_route, &route)?;

            eprintln!(
                "Done: {} points -> {} aligned, {} simplified, {} route points.",
                points.len(),
                aligned.len(),
                simplified.len(),
                route.len()
            );
            eprintln!("  {}", path_aligned.display());
            eprintln!("  {}", path_simplified.display());
            eprintln!("  {}", path_route.display());
        }
    }
    Ok(())
}

fn load(path: &Path) -> Result<Vec<TracePoint>, Error> {
    eprintln!("Loading {} ...", path.display());
    let points = gpx::load_points(path)?;
    eprintln!("  {} points loaded.", points.len());
    Ok(points)
}

fn align(points: &[TracePoint], args: &BackendArgs) -> Result<Vec<TracePoint>, Error> {
    let mut http_config = HttpConfig::default();
    http_config.rate_limit = args.rate_limit;
    http_config.timeout = Duration::from_secs_f64(args.timeout);
    let http = HttpClient::new(http_config)?;
    let backend = backends::by_name(&args.backend, args.backend_url.as_deref(), http)?;

    let mut options = AlignOptions::default();
    options.chunk_size = args.chunk_size;
    options.overlap = args.overlap;

    let result =
        gps_helper::align_with_progress(points, backend.as_ref(), &options, print_progress);
    eprintln!(); // End the line that progress kept rewriting.
    result
}

fn print_progress(progress: Progress) {
    let percent = progress.done_points * 100 / progress.total_points.max(1);
    eprint!(
        "\r  Aligning: chunk {}/{} ({percent}% of points)",
        progress.chunk, progress.total_chunks
    );
}

/// Derives a suffixed output path: `ride.gpx` + `aligned` -> `ride.aligned.gpx`.
fn suffixed_path(base: &Path, suffix: &str) -> PathBuf {
    let mut name = base.file_stem().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    if let Some(extension) = base.extension() {
        name.push(".");
        name.push(extension);
    }
    base.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn suffixed_paths() {
        let cases = [
            ("ride.gpx", "ride.aligned.gpx"),
            ("out", "out.aligned"),
            ("dir.d/out", "dir.d/out.aligned"),
            ("dir/a.b.gpx", "dir/a.b.aligned.gpx"),
            (".hidden", ".hidden.aligned"),
        ];
        for (base, expected) in cases {
            assert_eq!(
                suffixed_path(Path::new(base), "aligned"),
                Path::new(expected),
                "{base}"
            );
        }
    }

    #[test]
    fn timeouts_must_be_usable() {
        assert_eq!(parse_timeout("2.5"), Ok(2.5));
        for bad in ["0", "-1", "NaN", "inf", "1e300", "soon"] {
            assert!(parse_timeout(bad).is_err(), "{bad}");
        }
    }
}
