//! Transform static imports to dynamic imports
//!
//! Converts ES6 static import statements to dynamic import() calls
//! so they work inside async function wrappers.

use regex::Regex;
use std::sync::OnceLock;

/// Get the import pattern regex
fn import_pattern() -> &'static Regex {
    static IMPORT_REGEX: OnceLock<Regex> = OnceLock::new();
    IMPORT_REGEX.get_or_init(|| {
        // Match various import forms:
        // import foo from 'bar'
        // import * as foo from 'bar'
        // import { a, b } from 'bar'
        // import foo, { a, b } from 'bar'
        // Note: Allow leading whitespace for indented imports
        Regex::new(r#"(?m)^\s*import\s+([^;]+?)\s+from\s+['"]([^'"]+)['"];?\s*$"#).unwrap()
    })
}

/// Transform static imports to dynamic imports
///
/// Converts ES6 import statements to await import() calls that work
/// inside async functions.
///
/// # Examples
///
/// ```
/// # use jsexec::import_transform::transform_imports;
/// let code = r#"
/// import _ from 'lodash';
/// import * as ts from 'typescript';
/// const result = _.sum([1,2,3]);
/// "#;
///
/// let transformed = transform_imports(code);
/// // Now contains: const _module = await import('lodash'); const _ = _module.default || _module;
/// ```
pub fn transform_imports(code: &str) -> String {
    let re = import_pattern();

    re.replace_all(code, |caps: &regex::Captures| {
        let import_spec = caps.get(1).unwrap().as_str().trim();
        let module_path = caps.get(2).unwrap().as_str();

        transform_import_statement(import_spec, module_path)
    }).to_string()
}

fn transform_import_statement(import_spec: &str, module_path: &str) -> String {
    // Handle different import forms

    // Case 1: import * as foo from 'bar'
    if let Some(caps) = Regex::new(r"^\*\s+as\s+(\w+)$").unwrap().captures(import_spec) {
        let name = &caps[1];
        return format!("const {} = await import('{}');", name, module_path);
    }

    // Case 2: import { a, b, c } from 'bar'
    if import_spec.starts_with('{') && import_spec.ends_with('}') {
        // Direct destructuring from await import() - no intermediate variable
        return format!(
            "const {} = await import('{}');",
            import_spec, module_path
        );
    }

    // Case 3: import foo from 'bar' (default import)
    // Case 4: import foo, { a, b } from 'bar' (default + named)
    if let Some(comma_pos) = import_spec.find(',') {
        // Default + named imports
        let default_name = import_spec[..comma_pos].trim();
        let named_part = import_spec[comma_pos + 1..].trim();

        // Use a unique variable name based on the default import name to avoid collisions
        let module_var = format!("{}__mod", default_name);

        return format!(
            "const {} = await import('{}'); const {} = {}.default || {}; const {} = {};",
            module_var, module_path,
            default_name, module_var, module_var,
            named_part, module_var
        );
    }

    // Simple default import - use import name as base for module variable
    let default_name = import_spec.trim();
    let module_var = format!("{}__mod", default_name);

    format!(
        "const {} = await import('{}'); const {} = {}.default || {};",
        module_var, module_path,
        default_name, module_var, module_var
    )
}

fn sanitize_module_name(module_path: &str) -> String {
    // Convert 'lodash' -> 'lodash'
    // Convert '@types/node' -> 'types_node'
    // Convert 'some-package' -> 'some_package'
    module_path
        .replace(['/', '@', '-'], "_")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transform_default_import() {
        let code = "import _ from 'lodash';";
        let result = transform_imports(code);
        assert!(result.contains("await import('lodash')"));
        assert!(result.contains("const _ = "));
    }

    #[test]
    fn test_transform_namespace_import() {
        let code = "import * as ts from 'typescript';";
        let result = transform_imports(code);
        assert_eq!(result, "const ts = await import('typescript');");
    }

    #[test]
    fn test_transform_named_import() {
        let code = "import { sum, map } from 'lodash';";
        let result = transform_imports(code);
        assert!(result.contains("await import('lodash')"));
        assert!(result.contains("{ sum, map }"));
    }

    #[test]
    fn test_transform_mixed_import() {
        let code = "import _, { sum } from 'lodash';";
        let result = transform_imports(code);
        assert!(result.contains("await import('lodash')"));
        assert!(result.contains("const _ = "));
        assert!(result.contains("{ sum }"));
    }

    #[test]
    fn test_transform_multiple_imports() {
        let code = r#"
import _ from 'lodash';
import * as ts from 'typescript';

const result = _.sum([1,2,3]);
"#;
        let result = transform_imports(code);
        assert!(result.contains("await import('lodash')"));
        assert!(result.contains("await import('typescript')"));
        assert!(result.contains("const result = _.sum"));
    }

    #[test]
    fn test_preserve_non_import_code() {
        let code = r#"
import _ from 'lodash';

const x = 42;
// This is a comment about imports
const importantVar = 'not an import';
"#;
        let result = transform_imports(code);
        assert!(result.contains("const x = 42"));
        assert!(result.contains("const importantVar"));
        assert!(result.contains("// This is a comment"));
    }

    #[test]
    fn test_scoped_package() {
        let code = "import ts from '@types/node';";
        let result = transform_imports(code);
        assert!(result.contains("await import('@types/node')"));
    }
}
