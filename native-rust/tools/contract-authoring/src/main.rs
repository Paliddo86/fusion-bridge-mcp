use fusion_contract_authoring::*;
use std::{env, io::Write, path::Path};
fn main() {
    if let Err(error) = run() {
        eprintln!("contract-authoring: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, legacy, schema, baseline, out] if cmd == "extract" => {
            let bytes = read(Path::new(legacy))?;
            let text = std::str::from_utf8(&bytes).map_err(|_| "legacy source is not UTF8")?;
            let schema = read(Path::new(schema))?;
            let baseline = read(Path::new(baseline))?;
            let canonical = literal::extract(text, &schema, &baseline)?;
            let encoded = encode(&canonical, false)?;
            Catalog::load(&encoded, &schema)?.check(&baseline)?;
            publish(Path::new(out), &[("catalog.json".into(), encoded)])?;
            println!("extracted and checked canonical table");
            Ok(())
        }
        [cmd, source, schema] if cmd == "describe" => {
            let catalog = Catalog::load(&read(Path::new(source))?, &read(Path::new(schema))?)?;
            std::io::stdout()
                .write_all(&encode(&catalog.describe(), false)?)
                .map_err(|e| e.to_string())
        }
        [cmd, source, schema, baseline] if cmd == "check" => {
            let catalog = Catalog::load(&read(Path::new(source))?, &read(Path::new(schema))?)?;
            println!("{}", catalog.check(&read(Path::new(baseline))?)?);
            Ok(())
        }
        [cmd, source, schema, baseline, manifest, out] if cmd == "freeze" => {
            let catalog = Catalog::load(&read(Path::new(source))?, &read(Path::new(schema))?)?;
            let files = catalog.freeze(&read(Path::new(baseline))?, &read(Path::new(manifest))?)?;
            publish(Path::new(out), &files)?;
            println!("published checked immutable contract/manifest bundle");
            Ok(())
        }
        [cmd, source, schema, prior, manifest, date, out] if cmd == "refreeze" => {
            let catalog = Catalog::load(&read(Path::new(source))?, &read(Path::new(schema))?)?;
            let prior = if prior == "-" {
                None
            } else {
                Some(read(Path::new(prior))?)
            };
            let files = catalog.refreeze(prior.as_deref(), &read(Path::new(manifest))?, date)?;
            publish(Path::new(out), &files)?;
            println!("published explicitly refrozen contract/manifest/canonical bundle");
            Ok(())
        }
        _ => Err(concat!(
            "usage: extract LEGACY SCHEMA BASELINE NEW_DIRECTORY | ",
            "describe CANONICAL SCHEMA | check CANONICAL SCHEMA BASELINE | ",
            "freeze CANONICAL SCHEMA BASELINE MANIFEST NEW_DIRECTORY | ",
            "refreeze CANONICAL SCHEMA PRIOR_CONTRACT|- MANIFEST YYYY-MM-DD NEW_DIRECTORY"
        )
        .into()),
    }
}
