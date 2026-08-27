//! Measurement harness.
//!
//! The claims this crate makes are numbers — peak RSS, throughput, a refusal
//! that stays flat — and a number is only worth something if anyone can
//! reproduce it. So the harness is a shipped binary rather than a script that
//! lived long enough to produce a table once.
//!
//! ```text
//! xlsx-measure fixtures/out/large-600000.xlsx --max-bytes 1073741824 --max-rows 1000000
//! ```
//!
//! It prints one JSON object on stdout, and prints it whether the read
//! succeeded or was refused — a refusal's peak RSS is exactly what the hostile
//! archives are measured on.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use xlsx_core::{CellValue, ReadError, ReaderOptions, XlsxReader};

struct Args {
    path: PathBuf,
    sheet: Option<String>,
    batch: usize,
    max_decompressed_bytes: u64,
    max_rows: u64,
    list_only: bool,
    repeat: usize,
    all_sheets: bool,
}

fn usage() -> String {
    concat!(
        "usage: xlsx-measure <file.xlsx> [options]\n",
        "  --sheet <name>     sheet to read (default: the first one)\n",
        "  --batch <n>        rows per pull (default: 1000)\n",
        "  --max-bytes <n>    decompressed byte budget (default: 1 GiB)\n",
        "  --max-rows <n>     row budget (default: 2 000 000)\n",
        "  --list             list the sheets and stop\n",
        "  --repeat <n>       re-open and re-read n times (default: 1)\n",
        "  --all-sheets       read every sheet, not just one\n",
    )
    .to_string()
}

fn parse_args() -> Result<Args, String> {
    let mut argv = std::env::args().skip(1);

    let path = argv.next().ok_or_else(usage)?;
    let mut args = Args {
        path: PathBuf::from(path),
        sheet: None,
        batch: 1_000,
        max_decompressed_bytes: 1 << 30,
        max_rows: 2_000_000,
        list_only: false,
        repeat: 1,
        all_sheets: false,
    };

    while let Some(flag) = argv.next() {
        let mut value = || argv.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--sheet" => args.sheet = Some(value()?),
            "--batch" => args.batch = value()?.parse().map_err(|_| usage())?,
            "--max-bytes" => args.max_decompressed_bytes = value()?.parse().map_err(|_| usage())?,
            "--max-rows" => args.max_rows = value()?.parse().map_err(|_| usage())?,
            "--list" => args.list_only = true,
            "--repeat" => args.repeat = value()?.parse().map_err(|_| usage())?,
            "--all-sheets" => args.all_sheets = true,
            other => return Err(format!("unknown option {other}\n{}", usage())),
        }
    }

    Ok(args)
}

/// Peak resident set size, in kilobytes.
///
/// `VmHWM` is the kernel's own high-water mark, which is the only figure that
/// survives a peak that has already been released. Linux only; elsewhere the
/// memory assertions have nothing to stand on and say so.
fn peak_rss_kb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmHWM:") {
                return rest.split_whitespace().next()?.parse().ok();
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn report(fields: &[(&str, String)]) {
    let body = fields
        .iter()
        .map(|(key, value)| format!("\"{key}\":{value}"))
        .collect::<Vec<_>>()
        .join(",");
    println!("{{{body}}}");
}

fn escape(value: &str) -> String {
    format!("{:?}", value)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };

    let started = Instant::now();
    let summary = run(&args);
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let peak = peak_rss_kb()
        .map(|kb| kb.to_string())
        .unwrap_or("null".into());

    let mut fields = vec![
        ("rows", summary.rows.to_string()),
        ("cells", summary.cells.to_string()),
        ("sheets", summary.sheets.to_string()),
        ("decompressedBytes", summary.decompressed_bytes.to_string()),
        ("descriptorsLeaked", summary.descriptors_leaked.to_string()),
        ("elapsedMs", format!("{elapsed_ms:.1}")),
        ("peakRssKb", peak),
    ];

    // A refusal is a measurement, not a crash. It is reported alongside the peak
    // RSS and the descriptor count it went with — for a hostile archive, that
    // peak is the whole point of the run — and the process still exits zero.
    if let Some(error) = &summary.error {
        fields.push(("error", escape(error.code())));
        fields.push(("message", escape(&error.to_string())));
    }

    report(&fields);
    ExitCode::SUCCESS
}

struct Summary {
    rows: u64,
    cells: u64,
    sheets: usize,
    decompressed_bytes: u64,
    descriptors_leaked: i64,
    error: Option<ReadError>,
}

/// Descriptors this process holds. Counted in a process of its own, which is the
/// only way the figure means anything: `cargo test` runs its tests as threads of
/// one process, so a count taken there is everybody's count.
fn open_descriptors() -> i64 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir("/proc/self/fd")
            .map(|entries| entries.count() as i64)
            .unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

fn run(args: &Args) -> Summary {
    let options = ReaderOptions {
        max_decompressed_bytes: args.max_decompressed_bytes,
        max_rows: args.max_rows,
    };

    let mut summary = Summary {
        rows: 0,
        cells: 0,
        sheets: 0,
        decompressed_bytes: 0,
        descriptors_leaked: 0,
        error: None,
    };

    // One read before the baseline: the first open pulls in whatever the process
    // opens lazily, and that is not a leak.
    summary.error = read_once(args, options, &mut summary).err();
    let descriptors_before = open_descriptors();

    // A refusal does not stop the repeats. Reading a workbook that is going to
    // be refused, over and over, is exactly how a reader that holds on to the
    // archive when it gives up would show itself.
    for _ in 1..args.repeat.max(1) {
        if let Err(error) = read_once(args, options, &mut summary) {
            summary.error = Some(error);
        }
    }

    summary.descriptors_leaked = open_descriptors() - descriptors_before;

    summary
}

fn read_once(args: &Args, options: ReaderOptions, summary: &mut Summary) -> Result<(), ReadError> {
    summary.rows = 0;
    summary.cells = 0;

    // Listing takes its own path through the file — two small parts rather than
    // a walk of the archive — so measuring it through `open` would measure
    // something nobody runs.
    if args.list_only {
        let sheets = XlsxReader::list_sheets(&args.path, options.max_decompressed_bytes)?;
        summary.sheets = sheets.len();
        summary.decompressed_bytes = 0;
        return Ok(());
    }

    let mut reader = XlsxReader::open(&args.path, options)?;

    summary.sheets = reader.sheets().len();
    summary.decompressed_bytes = reader.decompressed_bytes();

    let targets: Vec<String> = if args.all_sheets {
        reader.sheets().iter().map(|s| s.name.clone()).collect()
    } else {
        args.sheet.clone().into_iter().collect()
    };

    if targets.is_empty() {
        return drain(&mut reader, args.batch, summary);
    }

    for name in targets {
        reader.select(&name)?;
        drain(&mut reader, args.batch, summary)?;
    }

    Ok(())
}

fn drain(reader: &mut XlsxReader, batch: usize, summary: &mut Summary) -> Result<(), ReadError> {
    while let Some(rows) = reader.next_batch(batch)? {
        summary.rows += rows.len() as u64;
        for row in &rows {
            summary.cells += row.iter().filter(|v| **v != CellValue::Empty).count() as u64;
        }
    }
    Ok(())
}
