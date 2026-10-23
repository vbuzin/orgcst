//! Corpus layout, sidecars, and the priorities oracle.

use orgcst::{EntityDef, ParseConfig, TodoInterpretation, TodoSequence};
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CODING_SOURCES: &[&str] = &[
    "\r\n",
    "crlf\r\nlines\r\n\r\nwith blanks\r\n",
    "\u{feff}* H\n",
    "\u{feff}\n* H\n",
];

const LICENSES: &[&str] = &[
    "MIT",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "Apache-2.0",
    "CC0-1.0",
    "Unlicense",
];

#[derive(Debug)]
pub struct Fixture {
    pub kind: &'static str,
    pub label: String,
    pub org: PathBuf,
    pub sidecar: PathBuf,
}

#[derive(Debug, Deserialize)]
struct Sidecar {
    #[serde(default)]
    inline_task_min_level: Option<u16>,
    #[serde(default)]
    list_alphabetical: Option<bool>,
    #[serde(default)]
    todo: Option<Vec<TodoSidecar>>,
    #[serde(default)]
    link_types: Option<Vec<String>>,
    #[serde(default)]
    user_entities: Option<Vec<EntitySidecar>>,
}

#[derive(Debug, Deserialize)]
struct TodoSidecar {
    interpretation: String,
    active: Vec<String>,
    done: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EntitySidecar {
    name: String,
    latex: String,
    latex_mathp: bool,
    html: String,
    ascii: String,
    latin1: String,
    utf8: String,
}

#[derive(Debug, Deserialize)]
struct Exception {
    id: String,
    fixture: String,
}

#[derive(Debug, Deserialize)]
struct PriorityRow {
    line: String,
    emacs: [u8; 3],
    rust: Option<[u8; 3]>,
}

pub fn config_from_sidecar(path: &Path) -> Result<ParseConfig, String> {
    let text = fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let side: Sidecar =
        serde_json::from_str(&text).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut config = ParseConfig::default();
    if let Some(level) = side.inline_task_min_level {
        config = config.with_inline_task_min_level(level);
    }
    if let Some(allow) = side.list_alphabetical {
        config = config.with_list_alphabetical(allow);
    }
    if let Some(types) = side.link_types {
        config = config.with_link_types(types);
    }
    if let Some(todo) = side.todo {
        let sequences = todo.into_iter().map(|seq| {
            let interpretation = if seq.interpretation == "type" {
                TodoInterpretation::Type
            } else {
                TodoInterpretation::Sequence
            };
            let done_from = seq.active.len();
            let mut names = seq.active;
            names.extend(seq.done);
            TodoSequence::new(interpretation, names, done_from)
        });
        config = config.with_todo_sequences(sequences);
    }
    if let Some(entities) = side.user_entities {
        let defs = entities.into_iter().map(|entity| {
            EntityDef::new(
                entity.name,
                entity.latex,
                entity.latex_mathp,
                entity.html,
                entity.ascii,
                entity.latin1,
                entity.utf8,
            )
        });
        config = config.with_user_entities(defs);
    }
    Ok(config)
}

pub fn plan(repo: &Path, out_dir: &Path) -> Result<Vec<Fixture>, String> {
    fs::create_dir_all(out_dir).map_err(|err| err.to_string())?;
    check_external(repo)?;
    let exceptions = read_exceptions(repo)?;
    check_diverge_dirs(repo, &exceptions)?;
    let default_side = out_dir.join("default.sidecar.json");
    fs::write(
        &default_side,
        "{\n  \"inline_task_min_level\": null,\n  \"list_alphabetical\": null,\n  \"todo\": null,\n  \"link_types\": null,\n  \"user_entities\": null\n}\n",
    )
    .map_err(|err| err.to_string())?;

    let mut fixtures = Vec::new();
    let mut later = Vec::new();
    let taxonomy = read_json(&repo.join("parity/corpus/taxonomy.json"))?;
    let rows = taxonomy.as_array().ok_or("taxonomy.json is not an array")?;
    for (index, row) in rows.iter().enumerate() {
        let src = row
            .get("src")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("taxonomy.json[{index}] has no src"))?;
        let org = out_dir.join(format!("tax-{index}.org"));
        let sidecar = out_dir.join(format!("tax-{index}.sidecar.json"));
        fs::write(&org, src).map_err(|err| err.to_string())?;
        fs::write(
            &sidecar,
            serde_json::to_string_pretty(row).map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
        let fixture = Fixture {
            kind: if CODING_SOURCES.contains(&src) {
                "coding"
            } else {
                "equal"
            },
            label: format!("parity/corpus/taxonomy.json[{index}]"),
            org,
            sidecar,
        };
        if fixture.kind == "coding" {
            fixtures.push(fixture);
        } else {
            later.push(fixture);
        }
    }
    fixtures.append(&mut later);
    push_org_dir(
        &mut fixtures,
        repo,
        &repo.join("parity/corpus/differential"),
        &default_side,
        "equal",
    )?;
    push_org_dir(
        &mut fixtures,
        repo,
        &repo.join("parity/corpus/realistic"),
        &default_side,
        "equal",
    )?;
    for exception in &exceptions {
        let dir = repo.join("parity/corpus/diverge").join(&exception.fixture);
        let org = dir.join("input.org");
        if !org.is_file() {
            return Err(format!("missing {}", org.display()));
        }
        let reason = dir.join("reason.txt");
        let text =
            fs::read_to_string(&reason).map_err(|err| format!("{}: {err}", reason.display()))?;
        let first = text.lines().next().unwrap_or("");
        if first != exception.id {
            return Err(format!(
                "{} first line is {first:?}, expected {}",
                reason.display(),
                exception.id
            ));
        }
        let own = dir.join("sidecar.json");
        let sidecar = if own.is_file() {
            own
        } else {
            default_side.clone()
        };
        fixtures.push(Fixture {
            kind: "differ",
            label: format!("parity/corpus/diverge/{}", exception.id),
            org,
            sidecar,
        });
    }
    Ok(fixtures)
}

fn push_org_dir(
    fixtures: &mut Vec<Fixture>,
    repo: &Path,
    dir: &Path,
    default_side: &Path,
    kind: &'static str,
) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("missing {}", dir.display()));
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("org") {
            files.push(path);
        }
    }
    files.sort();
    for org in files {
        let mut sidecar = org.clone();
        sidecar.set_extension("sidecar.json");
        let sidecar = if sidecar.is_file() {
            sidecar
        } else {
            default_side.to_path_buf()
        };
        let label = org
            .strip_prefix(repo)
            .unwrap_or(org.as_path())
            .display()
            .to_string();
        fixtures.push(Fixture {
            kind,
            label,
            org,
            sidecar,
        });
    }
    Ok(())
}

fn read_exceptions(repo: &Path) -> Result<Vec<Exception>, String> {
    let path = repo.join("parity/exceptions.json");
    let value = read_json(&path)?;
    serde_json::from_value(value).map_err(|err| format!("{}: {err}", path.display()))
}

fn check_diverge_dirs(repo: &Path, exceptions: &[Exception]) -> Result<(), String> {
    let dir = repo.join("parity/corpus/diverge");
    let mut names = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))? {
        let entry = entry.map_err(|err| err.to_string())?;
        if entry.file_type().map_err(|err| err.to_string())?.is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    let mut expected: Vec<String> = exceptions.iter().map(|row| row.fixture.clone()).collect();
    expected.sort();
    if names != expected {
        return Err(format!(
            "diverge directories {names:?} do not match exceptions {expected:?}"
        ));
    }
    Ok(())
}

fn check_external(repo: &Path) -> Result<(), String> {
    let dir = repo.join("parity/corpus/external");
    if !dir.exists() {
        return Ok(());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        if entry.file_name() == "MANIFEST" {
            continue;
        }
        if entry.file_type().map_err(|err| err.to_string())?.is_file() {
            files.push(entry.path());
        }
    }
    if files.is_empty() {
        return Ok(());
    }
    let manifest_path = dir.join("MANIFEST");
    let manifest = read_json(&manifest_path)?;
    let rows = manifest
        .as_array()
        .ok_or("external MANIFEST is not an array")?;
    for file in &files {
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("external file name is not utf-8")?;
        let row = rows
            .iter()
            .find(|row| row.get("path").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| format!("external/{name} has no MANIFEST row"))?;
        let spdx = row
            .get("spdx")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("external/{name} missing spdx"))?;
        if !LICENSES.contains(&spdx) {
            return Err(format!("external/{name} SPDX {spdx} is not allowed"));
        }
        for key in ["url", "retrieved", "sha256"] {
            if row
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .is_empty()
            {
                return Err(format!("external/{name} missing {key}"));
            }
        }
        let want = row.get("sha256").and_then(Value::as_str).unwrap_or("");
        let got = sha256(file)?;
        if !got.eq_ignore_ascii_case(want) {
            return Err(format!("external/{name} sha256 {got} != {want}"));
        }
    }
    Ok(())
}

fn sha256(path: &Path) -> Result<String, String> {
    if let Ok(output) = Command::new("sha256sum").arg(path).output()
        && output.status.success()
    {
        return first_word(&output.stdout);
    }
    let output = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .map_err(|err| format!("sha256: {err}"))?;
    if !output.status.success() {
        return Err(format!("sha256 failed for {}", path.display()));
    }
    first_word(&output.stdout)
}

fn first_word(bytes: &[u8]) -> Result<String, String> {
    let text = String::from_utf8_lossy(bytes);
    text.split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| "sha256 produced no digest".to_string())
}

pub fn check_priorities(table_path: &Path, emacs: &Value, pin: &str) -> Result<(), String> {
    let version = emacs
        .get("org_version")
        .and_then(Value::as_str)
        .ok_or("priorities output has no org_version")?;
    if version != pin {
        return Err(format!("org version {version} != pin {pin}"));
    }
    let rows = read_json(table_path)?;
    let rows: Vec<PriorityRow> =
        serde_json::from_value(rows).map_err(|err| format!("priorities.json: {err}"))?;
    let got = emacs
        .get("rows")
        .and_then(Value::as_array)
        .ok_or("priorities output has no rows")?;
    if got.len() != rows.len() {
        return Err(format!(
            "emacs returned {} priority rows, table has {}",
            got.len(),
            rows.len()
        ));
    }
    let mut errors = Vec::new();
    for (index, (row, emacs_row)) in rows.iter().zip(got).enumerate() {
        let emacs_got =
            triple(emacs_row).ok_or_else(|| format!("row {index} is not three integers"))?;
        if emacs_got != row.emacs {
            errors.push(format!(
                "line {:?}: emacs {emacs_got:?} != column {:?}",
                row.line, row.emacs
            ));
        }
        let rust_got = rust_priority(&row.line);
        if rust_got != row.rust {
            errors.push(format!(
                "line {:?}: rust {rust_got:?} != column {:?}",
                row.line, row.rust
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn rust_priority(line: &str) -> Option<[u8; 3]> {
    let src = format!("#+PRIORITIES: {line}\n");
    let doc = orgcst::parse(&src);
    doc.priority_range().map(|range| {
        [
            range.highest.rank(),
            range.lowest.rank(),
            range.default.rank(),
        ]
    })
}

fn triple(value: &Value) -> Option<[u8; 3]> {
    let array = value.as_array()?;
    if array.len() != 3 {
        return None;
    }
    Some([
        u8::try_from(array[0].as_u64()?).ok()?,
        u8::try_from(array[1].as_u64()?).ok()?,
        u8::try_from(array[2].as_u64()?).ok()?,
    ])
}

pub fn org_version(emacs: &Value) -> Result<String, String> {
    emacs
        .get("org_version")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "emacs JSON has no org_version".to_string())
}

pub fn read_pin(path: &Path) -> Result<String, String> {
    let text = fs::read_to_string(path).map_err(|err| {
        format!(
            "{}: {err}\nRe-run parity/parity.sh --record-version",
            path.display()
        )
    })?;
    Ok(text.trim().to_string())
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    serde_json::from_str(&text).map_err(|err| format!("{}: {err}", path.display()))
}
