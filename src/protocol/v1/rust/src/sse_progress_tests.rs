use super::*;

#[test]
fn shared_progress_fixture() {
    #[derive(Deserialize)]
    struct Case {
        name: String,
        data: String,
        class: String,
        output: bool,
    }
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../../fixtures/sse_progress.json")).unwrap();
    for case in cases {
        let got = classify(case.data.as_bytes());
        assert_eq!(got.as_str(), case.class, "{}", case.name);
        assert_eq!(got.is_output(), case.output, "{}", case.name);
    }
}
