//! Integration tests for jsexec
//!
//! These tests require workerd to be installed and available in PATH.
//! Run with: cargo test --test integration

use std::pin::pin;

use futures_util::StreamExt;
use jsexec::{JsExec, JsExecConfig, JsExecEvent, ModuleConfig};

/// Check if workerd is available
fn workerd_available() -> bool {
    std::process::Command::new("workerd")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Helper macro to skip tests if workerd is not available
macro_rules! require_workerd {
    () => {
        if !workerd_available() {
            eprintln!("Skipping test: workerd not available");
            return;
        }
    };
}

#[tokio::test]
async fn test_simple_eval() {
    require_workerd!();

    let jsexec = JsExec::new(JsExecConfig::default());
    let mut stream = pin!(jsexec.evaluate("1 + 1".to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                assert_eq!(*value, serde_json::json!(2));
                found_result = true;
            }
            JsExecEvent::Error { message, .. } => {
                panic!("Unexpected error: {}", message);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received a return value");
}

#[tokio::test]
async fn test_console_log() {
    require_workerd!();

    let jsexec = JsExec::new(JsExecConfig::default());
    // Use comma operator for multiple expressions in evaluate(), or use run() for statements
    let mut stream = pin!(jsexec.run(r#"console.log("hello world"); return 42;"#.to_string()));

    let mut console_events = vec![];
    let mut return_value = None;

    while let Some(event) = stream.next().await {
        match event {
            JsExecEvent::Console { level, args, .. } => {
                console_events.push((format!("{:?}", level), args));
            }
            JsExecEvent::Returned { value } => {
                return_value = Some(value);
            }
            JsExecEvent::Error { message, .. } => {
                panic!("Unexpected error: {}", message);
            }
            _ => {}
        }
    }

    assert!(!console_events.is_empty(), "Should have console output");
    assert_eq!(return_value, Some(serde_json::json!(42)));
}

#[tokio::test]
async fn test_syntax_error() {
    require_workerd!();

    let jsexec = JsExec::new(JsExecConfig::default());
    let mut stream = pin!(jsexec.evaluate("function(".to_string()));

    let mut found_error = false;
    while let Some(event) = stream.next().await {
        if let JsExecEvent::Error { name, .. } = event {
            assert_eq!(name, "SyntaxError");
            found_error = true;
        }
    }

    assert!(found_error, "Should have received a syntax error");
}

#[tokio::test]
async fn test_runtime_error() {
    require_workerd!();

    let jsexec = JsExec::new(JsExecConfig::default());
    // Use run() for throw statements since they can't be expressions
    let mut stream = pin!(jsexec.run("throw new Error('test error')".to_string()));

    let mut found_error = false;
    while let Some(event) = stream.next().await {
        if let JsExecEvent::Error { name, message, .. } = event {
            assert_eq!(name, "Error");
            assert!(message.contains("test error"));
            found_error = true;
        }
    }

    assert!(found_error, "Should have received a runtime error");
}

#[tokio::test]
async fn test_object_return() {
    require_workerd!();

    let jsexec = JsExec::new(JsExecConfig::default());
    let code = r#"({ name: "test", value: 123, nested: { a: 1, b: 2 } })"#;
    let mut stream = pin!(jsexec.evaluate(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                assert_eq!(value["name"], "test");
                assert_eq!(value["value"], 123);
                assert_eq!(value["nested"]["a"], 1);
                found_result = true;
            }
            JsExecEvent::Error { message, .. } => {
                panic!("Unexpected error: {}", message);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received object result");
}

#[tokio::test]
async fn test_array_return() {
    require_workerd!();

    let jsexec = JsExec::new(JsExecConfig::default());
    let mut stream = pin!(jsexec.evaluate("[1, 2, 3, 4, 5]".to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                let arr = value.as_array().expect("Should be array");
                assert_eq!(arr.len(), 5);
                found_result = true;
            }
            JsExecEvent::Error { message, .. } => {
                panic!("Unexpected error: {}", message);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received array result");
}

#[tokio::test]
async fn test_math_operations() {
    require_workerd!();

    let jsexec = JsExec::new(JsExecConfig::default());
    let code = r#"Math.sqrt(16) + Math.pow(2, 3)"#;
    let mut stream = pin!(jsexec.evaluate(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                // Compare as f64 since JS might return int or float
                assert_eq!(value.as_f64(), Some(12.0));
                found_result = true;
            }
            JsExecEvent::Error { message, .. } => {
                panic!("Unexpected error: {}", message);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received math result");
}

#[tokio::test]
async fn test_null_return() {
    require_workerd!();

    let jsexec = JsExec::new(JsExecConfig::default());
    let mut stream = pin!(jsexec.evaluate("null".to_string()));

    let mut found_null = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                assert!(value.is_null());
                found_null = true;
            }
            JsExecEvent::Error { message, .. } => {
                panic!("Unexpected error: {}", message);
            }
            _ => {}
        }
    }

    assert!(found_null, "Should have received null");
}

#[tokio::test]
async fn test_boolean_logic() {
    require_workerd!();

    let jsexec = JsExec::new(JsExecConfig::default());
    let mut stream = pin!(jsexec.evaluate("true && false || true".to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                assert_eq!(*value, serde_json::json!(true));
                found_result = true;
            }
            JsExecEvent::Error { message, .. } => {
                panic!("Unexpected error: {}", message);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received boolean result");
}

#[tokio::test]
async fn test_run_code_directly() {
    require_workerd!();

    let jsexec = JsExec::new(JsExecConfig::default());
    let code = r#"
        const x = 10;
        const y = 20;
        return x + y;
    "#;
    let mut stream = pin!(jsexec.run(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                assert_eq!(*value, serde_json::json!(30));
                found_result = true;
            }
            JsExecEvent::Error { message, .. } => {
                panic!("Unexpected error: {}", message);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received result from run()");
}

#[tokio::test]
async fn test_preloaded_module() {
    require_workerd!();

    // Create a simple utility module
    let utils_code = r#"
        export function add(a, b) { return a + b; }
        export function multiply(a, b) { return a * b; }
        export default { add, multiply };
    "#;

    let config = JsExecConfig {
        modules: vec![
            ModuleConfig::inline("utils", utils_code).with_global(true),
        ],
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    // Test accessing via ctx.modules
    let code = r#"
        const result = ctx.modules.utils.add(10, 20);
        return result;
    "#;
    let mut stream = pin!(jsexec.run(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                assert_eq!(*value, serde_json::json!(30));
                found_result = true;
            }
            JsExecEvent::Error { message, .. } => {
                panic!("Unexpected error: {}", message);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received result from module call");
}

#[tokio::test]
async fn test_module_on_global_this() {
    require_workerd!();

    // Create a math helpers module exposed on globalThis
    let math_code = r#"
        export function square(x) { return x * x; }
        export function cube(x) { return x * x * x; }
        export default { square, cube };
    "#;

    let config = JsExecConfig {
        modules: vec![
            ModuleConfig::inline("mathHelpers", math_code).with_global(true),
        ],
        ..Default::default()
    };

    let jsexec = JsExec::new(config);

    // Test accessing via globalThis (using the exposed name)
    let code = r#"
        // mathHelpers should be on globalThis
        return mathHelpers.square(5) + mathHelpers.cube(2);
    "#;
    let mut stream = pin!(jsexec.run(code.to_string()));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                // 5^2 + 2^3 = 25 + 8 = 33
                assert_eq!(*value, serde_json::json!(33));
                found_result = true;
            }
            JsExecEvent::Error { message, .. } => {
                panic!("Unexpected error: {}", message);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received result from globalThis module");
}

#[tokio::test]
async fn test_execute_with_modules() {
    require_workerd!();

    // Create a temp module file
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let module_path = temp_dir.path().join("helpers.js");

    let module_code = r#"
        export function greet(name) { return `Hello, ${name}!`; }
        export function double(x) { return x * 2; }
        export default { greet, double };
    "#;

    std::fs::write(&module_path, module_code).expect("Failed to write module file");

    let jsexec = JsExec::new(JsExecConfig::default());

    // Execute code that uses the module loaded by path
    let code = r#"
        const result = ctx.modules.helpers.greet("World");
        const doubled = ctx.modules.helpers.double(21);
        return { result, doubled };
    "#;

    let module_path_str = module_path.to_str().unwrap().to_string();
    let mut stream = pin!(jsexec.run_with_modules(code.to_string(), vec![module_path_str]));

    let mut found_result = false;
    while let Some(event) = stream.next().await {
        match &event {
            JsExecEvent::Returned { value } => {
                assert_eq!(value["result"], "Hello, World!");
                assert_eq!(value["doubled"], 42);
                found_result = true;
            }
            JsExecEvent::Error { message, .. } => {
                panic!("Unexpected error: {}", message);
            }
            _ => {}
        }
    }

    assert!(found_result, "Should have received result from path-loaded module");
}
