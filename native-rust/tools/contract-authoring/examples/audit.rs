use fusion_contract_authoring::*;
fn main() {
    let source = std::fs::read_to_string("evidence/baseline/describe.py").unwrap();
    let schema = std::fs::read("data/mechanical-schema.json").unwrap();
    let old = std::fs::read("evidence/baseline/describe.json").unwrap();
    let canonical = literal::extract(&source, &schema, &old).unwrap();
    let bytes = encode(&canonical, false).unwrap();
    let c = Catalog::load(&bytes, &schema).unwrap();
    let generated = encode(&c.frozen(), true).unwrap();
    std::fs::write("evidence/working/unaccepted-catalog.json", bytes).unwrap();
    std::fs::write("evidence/working/unaccepted-describe.json", &generated).unwrap();
    println!(
        "length old={} generated={} semantic_equal={}",
        old.len(),
        generated.len(),
        parse(&old).unwrap() == c.frozen()
    );
    let i = old
        .iter()
        .zip(&generated)
        .position(|(a, b)| a != b)
        .unwrap_or(old.len().min(generated.len()));
    println!(
        "first byte mismatch at {i}\nold: {:?}\nnew: {:?}",
        String::from_utf8_lossy(&old[i.saturating_sub(80)..(i + 120).min(old.len())]),
        String::from_utf8_lossy(&generated[i.saturating_sub(80)..(i + 120).min(generated.len())])
    );
}
