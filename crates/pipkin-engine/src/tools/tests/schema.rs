//! Argument validation against outputs captured from the reference (`golden-validate.json`:
//! pi-ai `validateToolArguments` over the durable tools' TypeBox schemas, after each tool's
//! `prepareArguments`).

use serde_json::Value;

use crate::tools::{builtin_tools, schema::validate};

#[test]
fn validation_matches_the_reference() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("golden-validate.json")).expect("golden json");
    assert!(cases.len() > 50);
    let tools = builtin_tools();
    for case in cases {
        let name = case["tool"].as_str().unwrap();
        let tool = tools.iter().find(|t| t.spec().name == name).unwrap();
        let prepared = tool.prepare_arguments(case["args"].clone());
        let actual = validate(name, &tool.spec().parameters, &prepared);
        match (&actual, case.get("ok"), case.get("err")) {
            (Ok(value), Some(expected), None) => {
                assert_eq!(value, expected, "case {case}")
            }
            (Err(message), None, Some(expected)) => {
                assert_eq!(message, expected.as_str().unwrap(), "case {case}")
            }
            _ => panic!("case {case}: got {actual:?}"),
        }
    }
}

#[test]
fn tool_specs_match_the_reference() {
    let specs: Vec<_> = builtin_tools().iter().map(|t| t.spec()).collect();
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["read", "write", "edit", "bash"]);
    // Key order is the reference's (TypeBox) order; `serde_json` keeps it (`preserve_order`).
    assert_eq!(
        serde_json::to_string(&specs[0].parameters).unwrap(),
        r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string","description":"Path to the file to read (relative or absolute)"},"offset":{"type":"number","description":"Line number to start reading from (1-indexed)"},"limit":{"type":"number","description":"Maximum number of lines to read"}}}"#
    );
    assert_eq!(
        specs[3].description,
        "Execute a bash command in the current working directory. Returns combined stdout and stderr. Output is truncated to last 2000 lines or 50KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds."
    );
    for spec in &specs {
        assert!(spec.parameters.is_object(), "{}", spec.name);
    }
}
