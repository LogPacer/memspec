use std::process::Command;

#[test]
fn report_schemas_preserve_their_draft07_contract() {
    let output = Command::new(env!("CARGO_BIN_EXE_memspec"))
        .arg("schema")
        .output()
        .expect("run schema command");
    assert!(output.status.success());
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).expect("schema JSON");
    let reports = document["$defs"].as_object().expect("report schemas");
    assert!(!reports.is_empty());
    for (name, schema) in reports {
        assert_eq!(
            schema["$schema"], "http://json-schema.org/draft-07/schema#",
            "report {name} changed its schema dialect"
        );
    }
}
