mod core;
mod office;

use std::{env, io, path::Path, process::ExitCode};

use core::VersionManager;
use serde_json::{json, Value};

const HELP: &str = "Usage: ovc --workspace <directory> <command> [arguments]

Commands:
  init
  status
  track <file>...
  tracked-files
  stage <file>
  unstage <file>
  commit-staged <message>
  commit <message>
  log [file]
  diff-working <file>
  diff-staged <file>
  diff-unstaged <file>
  diff-latest <file>
  diff-versions <file> <from-version> <to-version>
  checkout <file> <version> <output>

Paths passed to track and checkout are relative to the current directory
unless absolute. For version queries, use sourcePath from tracked-files.
Checkout overwrites an existing output file.
Successful commands print JSON to stdout.";

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || matches!(args.as_slice(), [flag] if flag == "--help" || flag == "-h") {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }

    match run(&args).and_then(write_result) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ovc: {error}");
            ExitCode::FAILURE
        }
    }
}

fn write_result(result: Value) -> Result<()> {
    use io::Write;

    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &result)?;
    writeln!(stdout)?;
    stdout.flush()?;
    Ok(())
}

fn run(args: &[String]) -> Result<Value> {
    let [flag, workspace, command, rest @ ..] = args else {
        return Err("expected --workspace <directory> <command> (see --help)".into());
    };
    if flag != "--workspace" || workspace.trim().is_empty() {
        return Err("expected --workspace <directory> (see --help)".into());
    }

    let manager = VersionManager::new(Path::new(workspace).join(".office-vcs"));
    let result = match (command.as_str(), rest) {
        ("init", []) => {
            manager.init()?;
            json!({})
        }
        ("status", []) => json!({ "status": manager.status()? }),
        ("track", [first, files @ ..]) => {
            let staged = std::iter::once(first)
                .chain(files)
                .map(|file| manager.add(file))
                .collect::<Result<Vec<_>>>()?;
            json!({ "staged": staged })
        }
        ("tracked-files", []) => json!({ "files": manager.tracked_files()? }),
        ("stage", [file]) => json!({ "staged": manager.stage(file)? }),
        ("unstage", [file]) => {
            manager.unstage(file)?;
            json!({})
        }
        ("commit-staged", [message]) => json!({ "records": manager.commit_staged(message.clone())? }),
        ("commit", [message]) => json!({ "records": manager.commit_tracked_changes(message.clone())? }),
        ("log", []) => json!({ "records": manager.log(None)? }),
        ("log", [file]) => json!({ "records": manager.log(Some(file))? }),
        ("diff-working", [file]) => json!({ "diff": manager.diff_working(file)? }),
        ("diff-staged", [file]) => json!({ "diff": manager.diff_staged(file)? }),
        ("diff-unstaged", [file]) => json!({ "diff": manager.diff_unstaged(file)? }),
        ("diff-latest", [file]) => json!({ "diff": manager.diff_latest(file)? }),
        ("diff-versions", [file, from, to]) => {
            json!({ "diff": manager.diff_versions(file, from, to)? })
        }
        ("checkout", [file, version, output]) => {
            json!({ "output": manager.checkout(file, version, output)? })
        }
        _ => return Err(format!("invalid command or arguments for {command:?} (see --help)").into()),
    };
    Ok(result)
}
