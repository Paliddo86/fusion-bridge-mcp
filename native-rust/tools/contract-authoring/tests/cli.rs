use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
fn path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}
fn cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fusion-contract-authoring"))
}
fn tmp() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "contract-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&p).unwrap();
    p
}
#[test]
fn cli_complete_check_describe_and_atomic_freeze() {
    let source = path("data/catalog.json");
    let schema = path("data/mechanical-schema.json");
    let baseline = path("evidence/baseline/describe.json");
    let manifest = path("evidence/baseline/bridge.json");
    let check = cmd()
        .arg("check")
        .args([&source, &schema, &baseline])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let report: fusion_contract_authoring::Result<_> =
        fusion_contract_authoring::parse(&check.stdout);
    assert_eq!(report.unwrap()["completeContractByteEqual"], true);
    let describe = cmd()
        .arg("describe")
        .args([&source, &schema])
        .output()
        .unwrap();
    assert!(describe.status.success());
    assert_eq!(
        fusion_contract_authoring::parse(&describe.stdout).unwrap()["verbCount"],
        178
    );
    let root = tmp();
    let output = root.join("bundle");
    let run = || {
        cmd()
            .arg("freeze")
            .args([&source, &schema, &baseline, &manifest, &output])
            .output()
            .unwrap()
    };
    assert!(run().status.success());
    assert_eq!(
        fs::read(output.join("describe.json")).unwrap(),
        fs::read(&baseline).unwrap()
    );
    assert_eq!(
        fs::read(output.join("bridge.json")).unwrap(),
        fs::read(&manifest).unwrap()
    );
    assert!(!run().status.success());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn cli_mismatched_baseline_and_malformed_arguments_never_publish() {
    let root = tmp();
    let bad = root.join("bad.json");
    fs::write(&bad, b"{\"verbs\":[]}").unwrap();
    let out = root.join("out");
    let run = cmd()
        .arg("freeze")
        .arg(path("data/catalog.json"))
        .arg(path("data/mechanical-schema.json"))
        .arg(&bad)
        .arg(path("evidence/baseline/bridge.json"))
        .arg(&out)
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(!out.exists());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    assert!(!cmd().arg("describe").output().unwrap().status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_refreeze_deliberate_update_is_atomic_reusable_and_does_not_touch_inputs() {
    use fusion_contract_authoring::{encode, parse};
    let root = tmp();
    let source = root.join("catalog.json");
    let mut changed = parse(&fs::read(path("data/catalog.json")).unwrap()).unwrap();
    changed["rows"][0][1] = serde_json::json!("Explicitly authored revision");
    changed["rows"][1][0] = serde_json::json!("authored_replacement");
    let source_bytes = encode(&changed, false).unwrap();
    fs::write(&source, &source_bytes).unwrap();
    let out = root.join("bundle");
    let run = |date: &str| {
        cmd()
            .arg("refreeze")
            .arg(&source)
            .arg(path("data/mechanical-schema.json"))
            .arg(path("evidence/baseline/describe.json"))
            .arg(path("evidence/baseline/bridge.json"))
            .arg(date)
            .arg(&out)
            .output()
            .unwrap()
    };
    assert!(!run("2026-02-29").status.success());
    assert!(!out.exists());
    let result = run("2026-10-16");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let contract = fs::read(out.join("describe.json")).unwrap();
    assert_eq!(parse(&contract).unwrap()["frozen"], "2026-10-16");
    assert_eq!(
        parse(&fs::read(out.join("bridge.json")).unwrap()).unwrap()["verbs"][0],
        "authored_replacement"
    );
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    assert!(!run("2026-10-17").status.success());
    assert_eq!(fs::read(out.join("describe.json")).unwrap(), contract);
    assert!(cmd()
        .arg("check")
        .arg(out.join("catalog.json"))
        .arg(path("data/mechanical-schema.json"))
        .arg(out.join("describe.json"))
        .status()
        .unwrap()
        .success());
    let next = root.join("unchanged");
    assert!(cmd()
        .arg("refreeze")
        .arg(out.join("catalog.json"))
        .arg(path("data/mechanical-schema.json"))
        .arg(out.join("describe.json"))
        .arg(out.join("bridge.json"))
        .arg("2099-01-01")
        .arg(&next)
        .status()
        .unwrap()
        .success());
    for name in ["describe.json", "bridge.json", "catalog.json"] {
        assert_eq!(
            fs::read(out.join(name)).unwrap(),
            fs::read(next.join(name)).unwrap()
        );
    }
    let initial = root.join("initial");
    assert!(cmd()
        .arg("refreeze")
        .arg(&source)
        .arg(path("data/mechanical-schema.json"))
        .arg("-")
        .arg(path("evidence/baseline/bridge.json"))
        .arg("2026-10-16")
        .arg(&initial)
        .status()
        .unwrap()
        .success());
    assert_eq!(fs::read(initial.join("describe.json")).unwrap(), contract);
    fs::remove_dir_all(root).unwrap();
}
