use serde_json::Value;

#[test]
fn title_classifier_matches_python() {
    let gold: Vec<Value> = serde_json::from_str(include_str!("golden/dialog_titles.json")).unwrap();
    for g in gold {
        let title = g["title"].as_str().unwrap();
        let got = fusion_core::dialogs::category_for_title(title).map(|(c, r)| serde_json::json!([c, r])).unwrap_or(Value::Null);
        assert_eq!(got, g["result"], "title {title:?}");
    }
}
