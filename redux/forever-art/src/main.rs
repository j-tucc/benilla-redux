//! `forever-art job` writes the wow.export CLI job that exports a manifest's sheets;
//! `forever-art build` turns the export into the addon's `Art/` TGAs and `Atlas.lua`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use forever_art::{build, job_json, tables, Build, Manifest, Tables};

const USAGE: &str = "\
usage:
  forever-art job   --extract <dir> --manifest <files> --install <wow dir> --log <file> --out <job.json> [--product wowf]
  forever-art build --extract <dir> --manifest <files> --log <files> --addon <dir>

  --extract   wow.export's output folder, holding the UiTextureAtlas* tables as CSV
  --manifest  one manifest, or several comma-separated (merged)
  --log       the wow.export CLI job log (job: where it will go; build: one or several comma-separated,
              read for FileDataID paths)";

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
    let mut manifest = Manifest::default();
    for path in opt("manifest")?.split(',') {
        manifest.merge(Manifest::parse(&read(&PathBuf::from(path))?)?);
    }
    let table = |name: &str| read(&extract.join(name));
    let t = Tables {
        atlases: tables::parse_atlases(&table("UiTextureAtlas.csv")?)?,
        members: tables::parse_members(&table("UiTextureAtlasMember.csv")?)?,
        elements: tables::parse_elements(&table("UiTextureAtlasElement.csv")?)?,
        slices: tables::parse_slices(&table("UiTextureAtlasElementSliceData.csv")?)?,
    };

    match command.as_str() {
        "job" => {
            let product = opts.get("product").map_or("wowf", String::as_str);
            // wow.export resolves paths against its own folder, so the job carries absolute ones.
            let extract = std::path::absolute(&extract).map_err(|e| e.to_string())?;
            let log = std::path::absolute(opt("log")?).map_err(|e| e.to_string())?;
            let text = job_json(
                &manifest,
                &t,
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
            let mut exported = HashMap::new();
            for path in opt("log")?.split(',') {
                exported.extend(tables::parse_export_log(&read(&PathBuf::from(path))?));
            }
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
                &t,
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
