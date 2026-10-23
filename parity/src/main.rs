//! Parity runner. Emacs stays in `parity.sh`; this binary projects and diffs.

mod corpus;
mod diff;
mod project;

use corpus::{check_priorities, config_from_sidecar, org_version, plan, read_pin};
use diff::{diff_trees, render};
use project::project;
use serde::Deserialize;
use serde_json::Value;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let cmd = args
        .next()
        .ok_or("usage: org-parity <plan|diff|priorities|record-version>")?;
    match cmd.as_str() {
        "plan" => cmd_plan(&mut args),
        "diff" => cmd_diff(&mut args),
        "priorities" => cmd_priorities(&mut args),
        "record-version" => cmd_record(&mut args),
        other => Err(format!("unknown command {other}")),
    }
}

fn cmd_plan(args: &mut impl Iterator<Item = String>) -> Result<(), String> {
    let repo = flag(args, "--repo")?;
    let out = flag(args, "--out")?;
    let fixtures = plan(&PathBuf::from(repo), &PathBuf::from(out))?;
    for fixture in fixtures {
        println!(
            "{}\t{}\t{}\t{}",
            fixture.kind,
            fixture.label,
            fixture.org.display(),
            fixture.sidecar.display()
        );
    }
    Ok(())
}

fn cmd_diff(args: &mut impl Iterator<Item = String>) -> Result<(), String> {
    let mut org = None;
    let mut sidecar = None;
    let mut emacs_path = None;
    let mut label = None;
    let mut pin = None;
    let mut expect = "equal".to_string();
    let mut coding = false;
    let mut verbose = false;
    let mut rest = args.peekable();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--org" => org = Some(rest.next().ok_or("--org needs a path")?),
            "--sidecar" => sidecar = Some(rest.next().ok_or("--sidecar needs a path")?),
            "--emacs" => emacs_path = Some(rest.next().ok_or("--emacs needs a path")?),
            "--label" => label = Some(rest.next().ok_or("--label needs a value")?),
            "--pin" => pin = Some(rest.next().ok_or("--pin needs a path")?),
            "--expect" => expect = rest.next().ok_or("--expect needs equal or differ")?,
            "--coding" => coding = true,
            "--verbose" => verbose = true,
            other => return Err(format!("unknown diff arg {other}")),
        }
    }
    let org = org.ok_or("--org is required")?;
    let sidecar = sidecar.ok_or("--sidecar is required")?;
    let emacs_path = emacs_path.ok_or("--emacs is required")?;
    let label = label.unwrap_or_else(|| org.clone());
    let source = fs::read_to_string(&org).map_err(|err| format!("{org}: {err}"))?;
    let config = config_from_sidecar(std::path::Path::new(&sidecar))?;
    let rust = project(&source, &config)?;
    let emacs_text =
        fs::read_to_string(&emacs_path).map_err(|err| format!("{emacs_path}: {err}"))?;
    // D-nesting-budget is deeper than serde_json's default recursion cap.
    let mut de = serde_json::Deserializer::from_str(emacs_text.trim());
    de.disable_recursion_limit();
    let emacs = Value::deserialize(&mut de)
        .map_err(|err| format!("{label}: emacs stdout is not one JSON value: {err}"))?;
    de.end()
        .map_err(|err| format!("{label}: emacs stdout is not one JSON value: {err}"))?;
    if let Some(pin) = pin {
        let want = read_pin(std::path::Path::new(&pin))?;
        let got = org_version(&emacs)?;
        if got != want {
            return Err(format!("{label}: org version {got} != {want}"));
        }
    }
    let diffs = diff_trees(&emacs, &rust);
    match expect.as_str() {
        "equal" => {
            if diffs.is_empty() {
                return Ok(());
            }
            eprint!("{}", render(&label, coding, &diffs, verbose));
            if verbose {
                eprintln!("emacs {emacs}");
                eprintln!("rust {rust}");
            }
            Err(format!("{label}: diverged"))
        }
        "differ" => {
            if diffs.is_empty() {
                Err(format!("{label}: stale exception, the trees agree"))
            } else {
                if verbose {
                    eprint!("{}", render(&label, false, &diffs, true));
                }
                Ok(())
            }
        }
        other => Err(format!("--expect {other} is not equal or differ")),
    }
}

fn cmd_priorities(args: &mut impl Iterator<Item = String>) -> Result<(), String> {
    let emacs_path = flag(args, "--emacs")?;
    let table = flag(args, "--table")?;
    let pin = flag(args, "--pin")?;
    let text = fs::read_to_string(&emacs_path).map_err(|err| format!("{emacs_path}: {err}"))?;
    let emacs: Value = serde_json::from_str(text.trim())
        .map_err(|err| format!("priorities emacs stdout is not JSON: {err}"))?;
    let want = read_pin(std::path::Path::new(&pin))?;
    check_priorities(std::path::Path::new(&table), &emacs, &want)
}

fn cmd_record(args: &mut impl Iterator<Item = String>) -> Result<(), String> {
    let emacs_path = flag(args, "--emacs")?;
    let out = flag(args, "--out")?;
    let text = fs::read_to_string(&emacs_path).map_err(|err| format!("{emacs_path}: {err}"))?;
    let emacs: Value = serde_json::from_str(text.trim())
        .map_err(|err| format!("record-version: stdout is not JSON: {err}"))?;
    let version = org_version(&emacs)?;
    fs::write(&out, format!("{version}\n")).map_err(|err| format!("{out}: {err}"))?;
    Ok(())
}

fn flag(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    match args.next() {
        Some(value) if value == name => args.next().ok_or_else(|| format!("{name} needs a value")),
        Some(other) => Err(format!("expected {name}, got {other}")),
        None => Err(format!("missing {name}")),
    }
}
