//! mock-harness binary.
//!
//! Reads `--script <name>` and emits a scripted JSONL event sequence on
//! stdout, optionally pacing in real-time. Used by the orchestrator's
//! supervision pipeline (Step 5+) and by manual debugging from a shell.
//!
//! Args:
//!   --script <name>   required: claude_happy | codex_blocked | gemini_happy | gemini_blocked | gemini_failed | gemini_terminal
//!   --speed <factor>  default 1.0; 0.0 = instant; higher = faster
//!   --worker-id <id>  override the synthetic worker_id stamped on events
//!   --task-id <id>    override the synthetic task_id stamped on events

use mock_harness::{build_script, EmitOpts, Script};
use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_WORKER_ID: &str = "0190a7e0-2c3a-7a01-9f00-000000000001";
const DEFAULT_TASK_ID: &str = "0190a7e0-2c3a-7a01-9f00-000000000002";

struct CliArgs {
    script: String,
    speed: f64,
    worker_id: String,
    task_id: String,
}

fn parse_args(argv: Vec<String>) -> Result<CliArgs, String> {
    let mut script: Option<String> = None;
    let mut speed: f64 = 1.0;
    let mut worker_id = DEFAULT_WORKER_ID.to_string();
    let mut task_id = DEFAULT_TASK_ID.to_string();

    let mut iter = argv.into_iter();
    let _bin = iter.next();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--script" => {
                script = Some(iter.next().ok_or("--script needs a value")?);
            }
            "--speed" => {
                let v = iter.next().ok_or("--speed needs a value")?;
                speed = v
                    .parse()
                    .map_err(|_| format!("--speed: {v:?} is not a number"))?;
                if speed < 0.0 {
                    return Err("--speed must be >= 0".into());
                }
            }
            "--worker-id" => {
                worker_id = iter.next().ok_or("--worker-id needs a value")?;
            }
            "--task-id" => {
                task_id = iter.next().ok_or("--task-id needs a value")?;
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unrecognised argument: {other:?}")),
        }
    }

    Ok(CliArgs {
        script: script.ok_or_else(|| "missing --script <name>".to_string())?,
        speed,
        worker_id,
        task_id,
    })
}

fn print_help() {
    eprintln!("mock-harness — emit scripted JSONL events on stdout");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!(
        "    mock-harness --script <name> [--speed <factor>] [--worker-id <id>] [--task-id <id>]"
    );
    eprintln!();
    eprintln!("SCRIPTS:");
    eprintln!("    claude_happy    planning → executing → reviewing → done");
    eprintln!("    codex_blocked   executing → blocked → executing → done");
    eprintln!("    gemini_happy    planning → executing → reviewing → done (gemini-flavored)");
    eprintln!("    gemini_blocked  executing → blocked → executing → done (gemini-flavored)");
    eprintln!("    gemini_failed   executing → failed (retryable: true)");
    eprintln!("    gemini_terminal executing → failed (retryable: false)");
    eprintln!();
    eprintln!("FLAGS:");
    eprintln!("    --speed <f>     1.0 = real-time, 0.0 = instant, higher = faster");
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let args = match parse_args(argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            print_help();
            return ExitCode::from(2);
        }
    };

    let script = match Script::from_name(&args.script) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let start_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let opts = EmitOpts {
        worker_id: args.worker_id,
        task_id: args.task_id,
        start_unix_ms,
    };

    let timed = build_script(script, &opts);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for te in timed {
        if args.speed > 0.0 && te.delay_ms_before > 0 {
            let actual_ms = (te.delay_ms_before as f64 / args.speed).round() as u64;
            if actual_ms > 0 {
                std::thread::sleep(Duration::from_millis(actual_ms));
            }
        }
        let line = match serde_json::to_string(&te.event) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: failed to serialize event: {e}");
                return ExitCode::from(1);
            }
        };
        if writeln!(out, "{line}").is_err() {
            // Pipe closed (consumer hung up). Exit cleanly.
            return ExitCode::SUCCESS;
        }
        let _ = out.flush();
    }

    ExitCode::SUCCESS
}
