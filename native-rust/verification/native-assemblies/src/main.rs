include!("support.rs");
include!("oracles.rs");
include!("envelopes.rs");
struct Campaign {
    runtime: Value,
    fixture: Value,
    address: SocketAddr,
    output: PathBuf,
    started: Instant,
    count: usize,
    checks: Vec<&'static str>,
    uncertain: bool,
    attempts: Vec<Value>,
    successful: Vec<Value>,
    inspections: BTreeMap<String, Value>,
}
include!("transport.rs");
include!("admission.rs");
include!("campaign.rs");
fn entry() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    ensure(
        args.len() == 3,
        "usage: fusion-native-assembly-verifier RUNTIME_JSON AUTHORITY_JSON NEW_RECEIPT_DIRECTORY",
    )?;
    let runtime_path = Path::new(&args[0]);
    let mut bytes = vec![];
    fs::File::open(runtime_path)
        .map_err(|e| e.to_string())?
        .take((MAX_RESPONSE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    ensure(bytes.len() <= MAX_RESPONSE, "runtime bound")?;
    let runtime: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let mut fixture = read_json(Path::new(&args[1]))?;
    validate_authority(&runtime, &fixture, &bytes)?;
    fixture["permittedRequests"] = json!({});
    let epoch = text(&runtime, "epoch")?;
    ensure(
        epoch.len() == 32
            && epoch
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "epoch",
    )?;
    let token = text(&runtime, "token")?;
    ensure(
        (32..=256).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_graphic()),
        "credential",
    )?;
    let address: SocketAddr = text(&runtime, "address")?.parse().map_err(|_| "address")?;
    ensure(
        address.ip().is_loopback() && address.port() != 0,
        "loopback required",
    )?;
    let path = PathBuf::from(&args[2]);
    ensure(path.is_absolute(), "absolute new output directory")?;
    fs::create_dir(&path).map_err(|e| e.to_string())?;
    let output = fs::canonicalize(path).map_err(|e| e.to_string())?;
    write_new(
        &output.join("campaign.json"),
        &redact(
            json!({"authority":fixture,"contract":CONTRACT,"maxRequests":MAX_REQUESTS,"deadlineSeconds":CAMPAIGN_LIMIT.as_secs(),"baseline":runtime["before"],"automaticRetry":false,"automaticCleanup":false,"scope":"independent protocol verifier; separate public SDK assembly truth validation pending"}),
            token,
        ),
    )?;
    let mut c = Campaign {
        runtime,
        fixture,
        address,
        output,
        started: Instant::now(),
        count: 0,
        checks: vec![],
        uncertain: false,
        attempts: vec![],
        successful: vec![],
        inspections: BTreeMap::new(),
    };
    let result = c.run();
    let receipt = redact(c.final_receipt(&result), text(&c.runtime, "token")?);
    write_new(&c.output.join("result.json"), &receipt)?;
    println!(
        "{}",
        serde_json::to_string(&receipt).map_err(|e| e.to_string())?
    );
    result.map(|_| ())
}
fn main() {
    if let Err(e) = entry() {
        eprintln!("assembly verification failed: {e}");
        std::process::exit(1)
    }
}
#[cfg(test)]
mod tests;
