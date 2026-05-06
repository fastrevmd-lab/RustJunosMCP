//! `render_and_apply_j2_template` — Jinja2 render with optional commit.
//!
//! Vars input is parsed as JSON if it starts with `{` (after whitespace) or
//! YAML otherwise. Both must produce a top-level object.

use crate::error::JmcpError;
use minijinja::{Environment, UndefinedBehavior};
use serde_json::Value;

/// Parse `vars_content` as JSON if first non-whitespace char is `{`,
/// otherwise as YAML. Both branches must produce a `Value::Object`.
pub(crate) fn parse_vars(input: &str) -> Result<Value, JmcpError> {
    let trimmed = input.trim_start();
    let parsed = if trimmed.starts_with('{') {
        serde_json::from_str::<Value>(input)
            .map_err(|e| JmcpError::TemplateVars(format!("JSON parse failed: {e}")))?
    } else {
        serde_yml::from_str::<Value>(input)
            .map_err(|e| JmcpError::TemplateVars(format!("YAML parse failed: {e}")))?
    };
    if !parsed.is_object() {
        return Err(JmcpError::TemplateVars(
            "vars_content must deserialize to a top-level object/map".into(),
        ));
    }
    Ok(parsed)
}

/// Render `template_content` with `vars` (a JSON object). Strict-undefined:
/// missing variables surface as `JmcpError::TemplateRender`, not silently as "".
pub(crate) fn render(template_content: &str, vars: &Value) -> Result<String, JmcpError> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    let tmpl = env
        .template_from_str(template_content)
        .map_err(|e| JmcpError::TemplateSyntax(format!("{e}")))?;
    tmpl.render(vars)
        .map_err(|e| JmcpError::TemplateRender(format!("{e}")))
}

/// Auto-detect Junos config format from the rendered string.
/// Returns "xml" if the first non-whitespace char is `<`, "set" if any line
/// starts with `set ` or `delete `, otherwise "text".
pub(crate) fn detect_format(rendered: &str) -> &'static str {
    let trimmed = rendered.trim_start();
    if trimmed.starts_with('<') {
        return "xml";
    }
    for line in rendered.lines() {
        let line = line.trim_start();
        if line.starts_with("set ") || line.starts_with("delete ") {
            return "set";
        }
    }
    "text"
}

use crate::device_manager::DeviceManager;
use crate::policy::Policy;
use crate::tools::TemplateArgs;
use serde_json::json;
use std::sync::Arc;

/// Resolve the router-selector args to a single canonical Vec<String>.
/// Rejects both-supplied; rejects empty `router_names`; allows neither
/// (returns an empty list — apply path will be a no-op).
fn resolve_routers(args: &TemplateArgs) -> Result<Vec<String>, JmcpError> {
    match (&args.router_name, &args.router_names) {
        (Some(_), Some(_)) => Err(JmcpError::Validation(
            "specify exactly one of `router_name` or `router_names`".into(),
        )),
        (Some(one), None) => Ok(vec![one.clone()]),
        (None, Some(many)) if many.is_empty() => Err(JmcpError::Validation(
            "`router_names` cannot be empty".into(),
        )),
        (None, Some(many)) => Ok(many.clone()),
        (None, None) => Ok(Vec::new()),
    }
}

pub async fn handle(
    args: TemplateArgs,
    dm: Arc<DeviceManager>,
    policy: Arc<Policy>,
) -> Result<serde_json::Value, JmcpError> {
    let routers = resolve_routers(&args)?;

    // Pre-flight: verify every named router exists. Mirrors the batch tool.
    for r in &routers {
        let _ = dm.inventory().get(r)?;
    }

    let vars = parse_vars(&args.vars_content)?;
    let rendered = render(&args.template_content, &vars)?;
    let format = match args.config_format.as_deref() {
        Some(f) if f == "set" || f == "text" || f == "xml" => f.to_string(),
        Some(other) => return Err(JmcpError::BadFormat(other.to_string())),
        None => detect_format(&rendered).to_string(),
    };

    if !args.apply_config {
        let mut rows = Vec::with_capacity(routers.len().max(1));
        if routers.is_empty() {
            rows.push(json!({
                "router": null,
                "rendered_template": rendered,
                "config_format": format,
            }));
        } else {
            for r in routers {
                rows.push(json!({
                    "router": r,
                    "rendered_template": rendered,
                    "config_format": format,
                }));
            }
        }
        return Ok(json!({ "results": rows, "applied": false }));
    }

    // Apply path lands in Task 8; until then, refuse.
    let _ = &policy;
    Err(JmcpError::Validation(
        "apply_config=true is not yet wired (see Task 8)".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vars_sniff_routes_json() {
        let v = parse_vars(r#"{"name":"r1","port":22}"#).unwrap();
        assert_eq!(v["name"], "r1");
        assert_eq!(v["port"], 22);
    }

    #[test]
    fn vars_sniff_routes_yaml() {
        let v = parse_vars("name: r1\nport: 22\n").unwrap();
        assert_eq!(v["name"], "r1");
        assert_eq!(v["port"], 22);
    }

    #[test]
    fn vars_sniff_handles_leading_whitespace_for_json() {
        let v = parse_vars("   \n   {\"x\":1}").unwrap();
        assert_eq!(v["x"], 1);
    }

    #[test]
    fn vars_sniff_rejects_non_object_json_array() {
        let r = parse_vars("[1,2,3]");
        assert!(matches!(r, Err(JmcpError::TemplateVars(_))));
    }

    #[test]
    fn vars_sniff_rejects_non_object_yaml_scalar() {
        let r = parse_vars("just a string");
        assert!(matches!(r, Err(JmcpError::TemplateVars(_))));
    }

    #[test]
    fn vars_sniff_surfaces_yaml_parse_error() {
        // Stray colons + flow indentation will fail YAML parse.
        let r = parse_vars("key: : :\n  - bad: : :\n");
        assert!(matches!(r, Err(JmcpError::TemplateVars(s)) if s.contains("YAML")));
    }

    #[test]
    fn render_substitutes_simple_var() {
        let out = render(
            "set system host-name {{ name }}",
            &parse_vars(r#"{"name":"r1"}"#).unwrap(),
        )
        .unwrap();
        assert_eq!(out, "set system host-name r1");
    }

    #[test]
    fn render_strict_undefined_fails_with_var_name() {
        let r = render(
            "set system host-name {{ missing }}",
            &parse_vars("{}").unwrap(),
        );
        match r {
            Err(JmcpError::TemplateRender(s)) => assert!(s.contains("undefined")),
            other => panic!("expected TemplateRender, got {other:?}"),
        }
    }

    #[test]
    fn render_minijinja_filters_work() {
        let out = render(
            "{{ name | upper }}-{{ ports | length }}",
            &parse_vars(r#"{"name":"r1","ports":[1,2,3,4]}"#).unwrap(),
        )
        .unwrap();
        assert_eq!(out, "R1-4");
    }

    #[test]
    fn render_template_syntax_error_surfaces() {
        let r = render("{{ unterminated", &parse_vars("{}").unwrap());
        assert!(matches!(r, Err(JmcpError::TemplateSyntax(_))));
    }

    #[test]
    fn format_autodetect_xml_for_leading_lt() {
        assert_eq!(detect_format("<configuration>...</configuration>"), "xml");
        assert_eq!(detect_format("\n  <foo/>"), "xml");
    }

    #[test]
    fn format_autodetect_set_for_set_lines() {
        assert_eq!(detect_format("set system host-name r1"), "set");
        assert_eq!(detect_format("delete protocols bgp"), "set");
        // Mixed input, but `set ` line wins:
        assert_eq!(detect_format("set foo\n# comment\nbar"), "set");
    }

    #[test]
    fn format_autodetect_text_otherwise() {
        assert_eq!(detect_format("system {\n  host-name r1;\n}"), "text");
        assert_eq!(detect_format(""), "text");
    }

    use crate::device_manager::DeviceManager;
    use crate::inventory::Inventory;
    use crate::policy::Policy;
    use crate::tools::TemplateArgs;
    use std::io::Write;
    use std::sync::Arc;

    fn inv_with(json: &str) -> Arc<Inventory> {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(json.as_bytes()).unwrap();
        Arc::new(Inventory::load(f.path()).unwrap())
    }

    fn args_render_only(routers: Vec<&str>) -> TemplateArgs {
        TemplateArgs {
            template_content: "set system host-name {{ name }}".into(),
            vars_content: r#"{"name":"r1"}"#.into(),
            router_name: None,
            router_names: Some(routers.iter().map(|s| s.to_string()).collect()),
            apply_config: false,
            commit_comment: "test".into(),
            dry_run: false,
            config_format: None,
        }
    }

    #[tokio::test]
    async fn render_only_returns_rendered_string_per_router() {
        let inv = inv_with(
            r#"{"r1":{"ip":"127.0.0.1","username":"u","auth":{"type":"password","password":"x"}}}"#,
        );
        let dm = Arc::new(DeviceManager::new(inv.clone()));
        let pol = Arc::new(Policy::build(&inv).unwrap());
        let r = handle(args_render_only(vec!["r1"]), dm, pol).await.unwrap();
        let rows = r["results"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["router"], "r1");
        assert_eq!(rows[0]["rendered_template"], "set system host-name r1");
        assert!(rows[0].get("commit_id").is_none());
        assert!(rows[0].get("error").is_none());
    }

    #[tokio::test]
    async fn render_only_unknown_router_returns_error_row() {
        let inv = inv_with(
            r#"{"r1":{"ip":"127.0.0.1","username":"u","auth":{"type":"password","password":"x"}}}"#,
        );
        let dm = Arc::new(DeviceManager::new(inv.clone()));
        let pol = Arc::new(Policy::build(&inv).unwrap());
        let r = handle(args_render_only(vec!["nope"]), dm, pol).await;
        assert!(matches!(r, Err(JmcpError::UnknownRouter(_))));
    }

    #[tokio::test]
    async fn render_only_rejects_both_router_name_and_names() {
        let inv = inv_with(
            r#"{"r1":{"ip":"127.0.0.1","username":"u","auth":{"type":"password","password":"x"}}}"#,
        );
        let dm = Arc::new(DeviceManager::new(inv.clone()));
        let pol = Arc::new(Policy::build(&inv).unwrap());
        let mut a = args_render_only(vec!["r1"]);
        a.router_name = Some("r1".into());
        let r = handle(a, dm, pol).await;
        assert!(matches!(r, Err(JmcpError::Validation(_))));
    }
}
