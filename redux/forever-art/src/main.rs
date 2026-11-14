//! `forever-art job` writes the wow.export CLI job that exports a manifest's sheets;
//! `forever-art build` turns the export into the addon's `Art/` TGAs and `Atlas.lua`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use forever_art::{build, job_json, tables, Build, Manifest};

const USAGE: &str = "\
usage:
  forever-art job   --extract <dir> --manifest <file> --install <wow dir> --log <file> --out <job.json> [--product wowf]
  forever-art build --extract <dir> --manifest <file> --log <file> --addon <dir>

  --extract   wow.export's output folder, holding UiTextureAtlas.csv and UiTextureAtlasMember.csv
  --log       the wow.export CLI job log (job: where it will go; build: read for FileDataID paths)";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("forever-art: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or(USAGE)?;
    let mut opts: HashMap<String, String> = HashMap::new();
    while let Some(flag) = args.next() {
        let key = flag
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected {flag:?}\n{USAGE}"))?;
        opts.insert(
            key.to_string(),
            args.next()
                .ok_or_else(|| format!("--{key} needs a value"))?,
        );
    }
    let opt = |k: &str| {
        opts.get(k)
            .cloned()
            .ok_or_else(|| format!("missing --{k}\n{USAGE}"))
    };
    let read =
        |p: &PathBuf| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));

    let extract = PathBuf::from(opt("extract")?);
    let manifest = Manifest::parse(&read(&PathBuf::from(opt("manifest")?))?)?;
    let atlases = tables::parse_atlases(&read(&extract.join("UiTextureAtlas.csv"))?)?;
    let members = tables::parse_members(&read(&extract.join("UiTextureAtlasMember.csv"))?)?;

    match command.as_str() {
        "job" => {
            let product = opts.get("product").map_or("wowf", String::as_str);
            // wow.export resolves paths against its own folder, so the job carries absolute ones.
            let extract = std::path::absolute(&extract).map_err(|e| e.to_string())?;
            let log = std::path::absolute(opt("log")?).map_err(|e| e.to_string())?;
            let text = job_json(
                &manifest,
                &members,
                &atlases,
                &opt("install")?,
                product,
                &extract.to_string_lossy(),
                &log.to_string_lossy(),
            );
            let out = opt("out")?;
            std::fs::write(&out, text).map_err(|e| format!("{out}: {e}"))?;
            println!("wrote {out}");
        }
        "build" => {
            let exported = tables::parse_export_log(&read(&PathBuf::from(opt("log")?))?);
            let addon = PathBuf::from(opt("addon")?);
            let addon_name = addon
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or("--addon must name a folder")?
                .to_string();
            let report = build(
                &Build {
                    extract: &extract,
                    addon: &addon,
                    addon_name: &addon_name,
                },
                &manifest,
                &members,
                &atlases,
                &exported,
            )?;
            println!(
                "{} atlas regions, {} textures -> {}",
                report.atlases.len(),
                report.textures.len(),
                addon.display()
            );
            if !report.missing_sheets.is_empty() {
                println!(
                    "sheets not exported yet (rerun `job` + wow.export): {:?}",
                    report.missing_sheets
                );
            }
            if !report.missing_files.is_empty() {
                println!("files not exported yet: {:?}", report.missing_files);
            }
        }
        _ => return Err(USAGE.to_string()),
    }
    Ok(())
}
